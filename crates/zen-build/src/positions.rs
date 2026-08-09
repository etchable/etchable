//! `# pcb:sch` position write-back — the only writer of position comment
//! blocks. Wraps `pcb_sch::position` (merge semantics: foreign keys like
//! `sym:` net-symbol positions survive) behind plain types so nothing
//! `pcb_*` crosses the crate boundary.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;

use crate::model::PositionDoc;

/// Upsert authored positions into `zen_file`'s trailing `# pcb:sch` block.
/// Keys are dotted instance paths relative to the file's root module
/// (e.g. `SENSE_DIV.R1.R`).
pub fn write_positions(
    zen_file: &Path,
    positions: &BTreeMap<String, PositionDoc>,
) -> anyhow::Result<()> {
    let map: BTreeMap<String, pcb_sch::position::Position> = positions
        .iter()
        .map(|(key, pos)| {
            let mirror = match pos.mirror.as_deref() {
                Some("x") => Some(pcb_sch::position::MirrorAxis::X),
                Some("y") => Some(pcb_sch::position::MirrorAxis::Y),
                _ => None,
            };
            (
                key.clone(),
                pcb_sch::position::Position {
                    x: pos.x,
                    y: pos.y,
                    rotation: pos.rotation,
                    mirror,
                },
            )
        })
        .collect();
    pcb_sch::position::replace_pcb_sch_comments(zen_file, &map)
        .with_context(|| format!("writing positions into {}", zen_file.display()))
}

/// Hex-encoded SHA-256 of the file's exact bytes — the optimistic-concurrency
/// token for `write_positions` callers (mirrors pcb-zen's LSP savePositions).
pub fn content_hash(path: &Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// A requested component move, in schematic space — the same y-up units
/// `get_circuit_json` reports component centers in.
#[derive(Debug, Clone, Copy)]
pub struct MovedPosition {
    pub x: f64,
    pub y: f64,
    /// Degrees; `None` keeps the component's current rotation.
    pub rotation: Option<f64>,
}

/// Compose the full save-all map `write_positions` needs from partial moves.
///
/// The layout honors authored positions only when EVERY component has one
/// (the all-or-nothing rule), so moving a subset means writing all: moved
/// components take their new schematic-space centers, everything else keeps
/// its current spot — the authored position verbatim when one exists,
/// otherwise the derived layout center. Rotation and mirror survive for
/// unmoved components.
///
/// Spaces: `# pcb:sch` is layout-world × 25.4 (both y-down); schematic
/// space is layout-world with y negated. Keys in the returned map are
/// root-stripped (the `# pcb:sch` convention); `moves` keys are full
/// `root.`-prefixed component instance paths.
pub fn merge_positions(
    sch: &crate::model::SchematicDoc,
    moves: &BTreeMap<String, MovedPosition>,
) -> anyhow::Result<BTreeMap<String, PositionDoc>> {
    use crate::layout::AUTHORED_DIVISOR;

    for path in moves.keys() {
        let Some(inst) = sch.instance(path) else {
            anyhow::bail!("no such instance: {path}");
        };
        if inst.kind != crate::model::InstanceKind::Component {
            anyhow::bail!(
                "{path} is a {:?}, not a component — positions are per component; \
                 move its component descendants instead",
                inst.kind
            );
        }
    }

    let layout = crate::layout::compute_layout(sch);
    let mut out = BTreeMap::new();
    for (path, inst) in &sch.instances {
        if inst.kind != crate::model::InstanceKind::Component {
            continue;
        }
        let key = path
            .strip_prefix("root.")
            .ok_or_else(|| anyhow::anyhow!("component path outside root: {path}"))?
            .to_string();
        let existing = inst.position.as_ref();
        let doc = if let Some(m) = moves.get(path) {
            PositionDoc {
                x: m.x * AUTHORED_DIVISOR,
                y: -m.y * AUTHORED_DIVISOR,
                rotation: m
                    .rotation
                    .or_else(|| existing.map(|p| p.rotation))
                    .unwrap_or(0.0),
                mirror: existing.and_then(|p| p.mirror.clone()),
            }
        } else if let Some(p) = existing {
            p.clone()
        } else {
            let Some(cl) = layout.comps.get(path) else {
                anyhow::bail!("no layout for component {path}");
            };
            PositionDoc {
                x: cl.center.0 * AUTHORED_DIVISOR,
                y: cl.center.1 * AUTHORED_DIVISOR,
                rotation: 0.0,
                mirror: None,
            }
        };
        out.insert(key, doc);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{InstanceDoc, InstanceKind, PinDoc, SchematicDoc};

    fn two_resistor_board() -> SchematicDoc {
        let resistor = |path: &str, refdes: &str| InstanceDoc {
            path: path.into(),
            kind: InstanceKind::Component,
            type_name: "R".into(),
            source_file: None,
            refdes: Some(refdes.into()),
            attributes: BTreeMap::new(),
            children: BTreeMap::new(),
            pins: vec![
                PinDoc {
                    name: "P1".into(),
                    net: None,
                },
                PinDoc {
                    name: "P2".into(),
                    net: None,
                },
            ],
            position: None,
        };
        let mut instances = BTreeMap::new();
        instances.insert(
            "root".into(),
            InstanceDoc {
                path: "root".into(),
                kind: InstanceKind::Module,
                type_name: "<root>".into(),
                source_file: None,
                refdes: None,
                attributes: BTreeMap::new(),
                children: BTreeMap::from([
                    ("A".to_string(), "root.A".to_string()),
                    ("B".to_string(), "root.B".to_string()),
                ]),
                pins: vec![],
                position: None,
            },
        );
        instances.insert("root.A".into(), resistor("root.A", "R1"));
        instances.insert("root.B".into(), resistor("root.B", "R2"));
        SchematicDoc {
            root_module: "<root>".into(),
            instances,
            nets: BTreeMap::new(),
            by_refdes: BTreeMap::from([
                ("R1".to_string(), "root.A".to_string()),
                ("R2".to_string(), "root.B".to_string()),
            ]),
        }
    }

    #[test]
    fn partial_move_fills_every_component() {
        let sch = two_resistor_board();
        let moves = BTreeMap::from([(
            "root.A".to_string(),
            MovedPosition {
                x: 1.0,
                y: -0.5,
                rotation: None,
            },
        )]);
        let full = merge_positions(&sch, &moves).unwrap();

        // Save-all: both components covered, keys root-stripped.
        assert_eq!(full.len(), 2);
        let a = &full["A"];
        assert!((a.x - 25.4).abs() < 1e-9);
        // Schematic y-up -> pcb:sch y-down.
        assert!((a.y - 12.7).abs() < 1e-9);
        assert_eq!(a.rotation, 0.0);

        // The unmoved component sits at its derived layout center.
        let b = &full["B"];
        let layout = crate::layout::compute_layout(&sch);
        let center = layout.comps["root.B"].center;
        assert!((b.x - center.0 * 25.4).abs() < 1e-9);
        assert!((b.y - center.1 * 25.4).abs() < 1e-9);
    }

    #[test]
    fn moving_a_module_is_rejected() {
        let sch = two_resistor_board();
        let moves = BTreeMap::from([(
            "root".to_string(),
            MovedPosition {
                x: 0.0,
                y: 0.0,
                rotation: None,
            },
        )]);
        let e = merge_positions(&sch, &moves).unwrap_err();
        assert!(e.to_string().contains("not a component"), "{e}");
    }
}
