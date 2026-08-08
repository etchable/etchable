//! Deterministic schematic layout. All math in world coordinates (y grows
//! downward, like the old canvas); `circuit_json` flips y once at emission.
//!
//! Placement is connectivity-aware per module: siblings sharing a local
//! signal net get a directed edge (output-ish pin -> input-ish pin), columns
//! come from longest-path layering, and row order from a two-sweep
//! barycenter pass — so signal chains read left to right. Authored
//! `# pcb:sch` positions win only when every component has one (a partial
//! set would interleave two coordinate systems; the save-all write-back
//! relies on this rule).

use std::collections::BTreeMap;

use crate::circuit_json::{classify, resolve_glyph, Orient, ResolvedGlyph};
use crate::model::{InstanceDoc, InstanceKind, PinDoc, SchematicDoc};
use crate::route::ROUTE_MAX_PORTS;

pub(crate) const PIN_SPACING: f64 = 0.2;
const CHIP_MIN_W: f64 = 1.2;
const CHIP_MIN_H: f64 = 0.6;
pub(crate) const MODULE_PAD: f64 = 0.5;
pub(crate) const MODULE_TITLE_H: f64 = 0.3;
/// Column spacing must clear two net-label flags extending toward each
/// other (~1 unit each for typical net names).
const GAP: f64 = 2.4;
/// Stacked rows only need to clear wire stubs, not facing labels.
const ROW_GAP: f64 = 1.6;
/// Authored `# pcb:sch` coordinates are in the pcb layout tool's mm-ish
/// space; dividing by 25.4 lands boards in the same magnitude as computed
/// layout.
const AUTHORED_DIVISOR: f64 = 25.4;

// ---------------------------------------------------------------------------
// Shared ordering / pin-splitting helpers
// ---------------------------------------------------------------------------

/// Natural, case-insensitive ordering ("P2" < "P10"), mirroring the previous
/// TS canvas so layouts stay familiar.
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < ab.len() && j < bb.len() {
        let (ca, cb) = (ab[i], bb[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            let na: u64 = a[si..i].parse().unwrap_or(u64::MAX);
            let nb: u64 = b[sj..j].parse().unwrap_or(u64::MAX);
            match na.cmp(&nb) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            }
        } else {
            match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                other => return other,
            }
        }
    }
    ab.len().cmp(&bb.len()).then_with(|| a.cmp(b))
}

pub(crate) const LEFTY: [&str; 7] = ["1", "A", "P1", "+", "IN", "VIN", "L"];
pub(crate) const RIGHTY: [&str; 6] = ["2", "K", "P2", "-", "OUT", "VOUT"];

/// Split pins between the left and right edge (port of the TS canvas
/// splitPins): known input-ish names lean left, output-ish right, the rest
/// natural-sorted and halved.
pub(crate) fn split_pins(pins: &[PinDoc]) -> (Vec<PinDoc>, Vec<PinDoc>) {
    if pins.len() == 2 {
        let score = |p: &PinDoc| {
            if LEFTY.contains(&p.name.as_str()) {
                0
            } else if RIGHTY.contains(&p.name.as_str()) {
                2
            } else {
                1
            }
        };
        let mut sorted = pins.to_vec();
        sorted.sort_by(|a, b| score(a).cmp(&score(b)).then_with(|| natural_cmp(&a.name, &b.name)));
        let right = sorted.split_off(1);
        return (sorted, right);
    }
    let mut sorted = pins.to_vec();
    sorted.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    let right = sorted.split_off(sorted.len().div_ceil(2));
    (sorted, right)
}

pub(crate) fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Layout data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone)]
pub(crate) struct PinLayout {
    pub(crate) name: String,
    pub(crate) net: Option<String>,
    pub(crate) number: u32,
    /// World position of the port.
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) side: Side,
}

#[derive(Debug, Clone)]
pub(crate) struct CompLayout {
    pub(crate) center: (f64, f64),
    pub(crate) size: (f64, f64),
    /// Resolved schematic-symbols name; `None` = box-with-pins chip.
    pub(crate) symbol_name: Option<String>,
    pub(crate) pins: Vec<PinLayout>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

pub(crate) struct Layout {
    pub(crate) comps: BTreeMap<String, CompLayout>,
    /// Module path -> world bounding box (root excluded).
    pub(crate) modules: BTreeMap<String, Rect>,
}

/// A component with its glyph-vs-chip decision already made.
enum ResolvedComp {
    Glyph(ResolvedGlyph, Vec<PinDoc>),
    Chip { left: Vec<PinDoc>, right: Vec<PinDoc> },
}

struct SizedNode {
    path: String,
    w: f64,
    h: f64,
    kids: Vec<SizedNode>,
    offsets: Vec<(f64, f64)>,
    comp: Option<ResolvedComp>,
}

fn chip_size(left: &[PinDoc], right: &[PinDoc]) -> (f64, f64) {
    let max_side = left.len().max(right.len()).max(1) as f64;
    let label = |pins: &[PinDoc]| pins.iter().map(|p| p.name.len()).max().unwrap_or(0);
    (
        CHIP_MIN_W.max(0.8 + 0.1 * (label(left) + label(right)) as f64),
        CHIP_MIN_H.max(max_side * PIN_SPACING + 0.4),
    )
}

/// Glyph-or-chip resolution for one component at one orientation.
fn resolve_comp(inst: &InstanceDoc, orient: Orient) -> ResolvedComp {
    if let Some(choice) = classify(inst).symbol {
        if let Some(glyph) = resolve_glyph(inst, choice, orient) {
            return ResolvedComp::Glyph(glyph, inst.pins.clone());
        }
    }
    let (left, right) = split_pins(&inst.pins);
    ResolvedComp::Chip { left, right }
}

fn drawable_children<'a>(
    sch: &'a SchematicDoc,
    inst: &'a InstanceDoc,
) -> Vec<(&'a str, &'a InstanceDoc)> {
    let mut out: Vec<(&str, &InstanceDoc)> = inst
        .children
        .iter()
        .filter_map(|(name, child_path)| {
            sch.instances.get(child_path).and_then(|c| {
                matches!(c.kind, InstanceKind::Component | InstanceKind::Module)
                    .then_some((name.as_str(), c))
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let ga = (a.1.kind != InstanceKind::Component) as u8;
        let gb = (b.1.kind != InstanceKind::Component) as u8;
        ga.cmp(&gb).then_with(|| natural_cmp(a.0, b.0))
    });
    out
}

// ---------------------------------------------------------------------------
// Connectivity-aware packing
// ---------------------------------------------------------------------------

/// Output-ish pin names imply signal flows *out* of the touching sibling.
fn is_output_pin(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    RIGHTY.contains(&name)
        || matches!(lower.as_str(), "out" | "o" | "output" | "d" | "drain" | "c" | "collector")
}

fn is_input_pin(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    LEFTY.contains(&name)
        || matches!(lower.as_str(), "in" | "input" | "g" | "gate" | "b" | "base")
}

/// Column and row assignment for one module's children: longest-path
/// layering over directed connectivity, then two barycenter sweeps.
/// Returns (offsets aligned with `kids`, inner width, inner height).
fn pack_by_connectivity(
    sch: &SchematicDoc,
    kids: &[SizedNode],
) -> (Vec<(f64, f64)>, f64, f64) {
    let n = kids.len();
    if n == 0 {
        return (Vec::new(), 0.0, 0.0);
    }

    // Owner lookup: component path -> kid index (by path prefix).
    let owner = |path: &str| {
        kids.iter().position(|k| {
            path == k.path || path.starts_with(&format!("{}.", k.path))
        })
    };

    // Directed edges between siblings sharing a local signal net. Each
    // unordered pair is scored once: output-ish pins push flow out of their
    // kid, input-ish pins pull it in; ties fall back to natural child order.
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for net in sch.nets.values() {
        if net.kind == "Power" || net.kind == "Ground" || net.ports.len() > ROUTE_MAX_PORTS {
            continue;
        }
        // Aggregate per kid: does any pin of this net drive out / feed in?
        let mut touch: BTreeMap<usize, (bool, bool)> = BTreeMap::new();
        for port in &net.ports {
            if let Some(k) = owner(&port.component) {
                let entry = touch.entry(k).or_insert((false, false));
                entry.0 |= is_output_pin(&port.pin);
                entry.1 |= is_input_pin(&port.pin);
            }
        }
        let kids_touched: Vec<(usize, (bool, bool))> =
            touch.into_iter().collect();
        for (i, &(a, (a_out, a_in))) in kids_touched.iter().enumerate() {
            for &(b, (b_out, b_in)) in kids_touched.iter().skip(i + 1) {
                let forward = (a_out as i8) + (b_in as i8);
                let backward = (b_out as i8) + (a_in as i8);
                if forward >= backward {
                    edges.push((a, b));
                } else {
                    edges.push((b, a));
                }
            }
        }
    }
    edges.sort();
    edges.dedup();

    // Break cycles: DFS in natural order, drop back edges.
    let mut adj = vec![Vec::new(); n];
    for &(a, b) in &edges {
        adj[a].push(b);
    }
    let mut state = vec![0u8; n]; // 0 unvisited, 1 on stack, 2 done
    let mut kept: Vec<(usize, usize)> = Vec::new();
    fn dfs(
        v: usize,
        adj: &[Vec<usize>],
        state: &mut [u8],
        kept: &mut Vec<(usize, usize)>,
    ) {
        state[v] = 1;
        for &w in &adj[v] {
            match state[w] {
                0 => {
                    kept.push((v, w));
                    dfs(w, adj, state, kept);
                }
                2 => kept.push((v, w)),
                _ => {} // back edge: drop
            }
        }
        state[v] = 2;
    }
    for v in 0..n {
        if state[v] == 0 {
            dfs(v, &adj, &mut state, &mut kept);
        }
    }

    // Longest-path column assignment (kept edges are acyclic).
    let mut col = vec![0usize; n];
    let mut changed = true;
    while changed {
        changed = false;
        for &(a, b) in &kept {
            if col[b] < col[a] + 1 {
                col[b] = col[a] + 1;
                changed = true;
            }
        }
    }

    let n_cols = col.iter().max().map_or(1, |m| m + 1);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); n_cols];
    for (i, &c) in col.iter().enumerate() {
        columns[c].push(i);
    }

    // Row ordering: two barycenter sweeps over neighbor row positions.
    let mut row = vec![0.0f64; n];
    for column in &columns {
        for (r, &i) in column.iter().enumerate() {
            row[i] = r as f64;
        }
    }
    let neighbors_of = |i: usize, dir_prev: bool| -> Vec<usize> {
        kept.iter()
            .filter_map(|&(a, b)| {
                if dir_prev && b == i {
                    Some(a)
                } else if !dir_prev && a == i {
                    Some(b)
                } else {
                    None
                }
            })
            .collect()
    };
    for dir_prev in [true, false] {
        let order: Box<dyn Iterator<Item = &Vec<usize>>> = if dir_prev {
            Box::new(columns.iter())
        } else {
            Box::new(columns.iter().rev())
        };
        let mut new_rows: Vec<(usize, Vec<usize>)> = Vec::new();
        for column in order {
            let mut scored: Vec<(f64, usize)> = column
                .iter()
                .map(|&i| {
                    let ns = neighbors_of(i, dir_prev);
                    let bary = if ns.is_empty() {
                        row[i]
                    } else {
                        ns.iter().map(|&x| row[x]).sum::<f64>() / ns.len() as f64
                    };
                    (bary, i)
                })
                .collect();
            scored.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| natural_cmp(&kids[a.1].path, &kids[b.1].path))
            });
            for (r, &(_, i)) in scored.iter().enumerate() {
                row[i] = r as f64;
            }
            new_rows.push((col[scored[0].1], scored.iter().map(|&(_, i)| i).collect()));
        }
        for (c, ordered) in new_rows {
            columns[c] = ordered;
        }
    }

    // Placement: columns left -> right, rows stacked, columns v-centered.
    let col_heights: Vec<f64> = columns
        .iter()
        .map(|column| {
            column.iter().map(|&i| kids[i].h).sum::<f64>()
                + ROW_GAP * column.len().saturating_sub(1) as f64
        })
        .collect();
    let inner_h = col_heights.iter().cloned().fold(0.0, f64::max);

    let mut offsets = vec![(0.0, 0.0); n];
    let mut col_x = 0.0;
    for (column, col_h) in columns.iter().zip(&col_heights) {
        let col_w = column.iter().map(|&i| kids[i].w).fold(0.0, f64::max);
        let mut y = (inner_h - col_h) / 2.0;
        for &i in column {
            offsets[i] = (col_x + (col_w - kids[i].w) / 2.0, y);
            y += kids[i].h + ROW_GAP;
        }
        col_x += col_w + GAP;
    }
    (offsets, col_x - GAP, inner_h)
}

// ---------------------------------------------------------------------------
// Sizing, placement, pin geometry
// ---------------------------------------------------------------------------

fn size_node(sch: &SchematicDoc, inst: &InstanceDoc) -> SizedNode {
    if inst.kind == InstanceKind::Component {
        let comp = resolve_comp(inst, Orient::Right);
        let (w, h) = match &comp {
            ResolvedComp::Glyph(glyph, _) => (glyph.geom.w, glyph.geom.h),
            ResolvedComp::Chip { left, right } => chip_size(left, right),
        };
        return SizedNode {
            path: inst.path.clone(),
            w,
            h,
            kids: Vec::new(),
            offsets: Vec::new(),
            comp: Some(comp),
        };
    }

    let kids: Vec<SizedNode> = drawable_children(sch, inst)
        .into_iter()
        .map(|(_, c)| size_node(sch, c))
        .collect();

    let (offsets, inner_w, inner_h) = pack_by_connectivity(sch, &kids);

    let title_w = 0.12 * last_segment(&inst.path).len() as f64 + 0.5;
    SizedNode {
        path: inst.path.clone(),
        w: (inner_w + 2.0 * MODULE_PAD).max(title_w).max(1.2),
        h: MODULE_TITLE_H + MODULE_PAD + inner_h.max(0.3) + MODULE_PAD,
        kids,
        offsets,
        comp: None,
    }
}

fn place(node: &SizedNode, x: f64, y: f64, is_root: bool, out: &mut Layout) {
    if let Some(comp) = &node.comp {
        let center = (x + node.w / 2.0, y + node.h / 2.0);
        let layout = match comp {
            ResolvedComp::Glyph(glyph, pins) => glyph_layout_at(center, glyph, pins),
            ResolvedComp::Chip { left, right } => {
                chip_layout_at(center, (node.w, node.h), left, right)
            }
        };
        out.comps.insert(node.path.clone(), layout);
        return;
    }
    if !is_root {
        out.modules.insert(
            node.path.clone(),
            Rect {
                x,
                y,
                w: node.w,
                h: node.h,
            },
        );
    }
    let (ox, oy) = if is_root {
        (x, y)
    } else {
        (x + MODULE_PAD, y + MODULE_TITLE_H + MODULE_PAD)
    };
    for (kid, off) in node.kids.iter().zip(&node.offsets) {
        place(kid, ox + off.0, oy + off.1, false, out);
    }
}

/// Pin world positions for a glyph component: the symbol's native port
/// offsets, verbatim. Symbol coordinates ARE schematic coordinates (y-up) —
/// verified empirically against circuit-to-svg's angle-matcher — so the
/// world-space (y-down) offset negates dy.
fn glyph_layout_at(center: (f64, f64), glyph: &ResolvedGlyph, pins_src: &[PinDoc]) -> CompLayout {
    let geom = glyph.geom;
    let mut pins = Vec::with_capacity(geom.ports.len());
    for (port_idx, port) in geom.ports.iter().enumerate() {
        let pin = &pins_src[glyph.pin_for_port[port_idx]];
        let (ox, oy) = (port.dx, -port.dy);
        let side = if ox.abs() >= oy.abs() {
            if ox <= 0.0 {
                Side::Left
            } else {
                Side::Right
            }
        } else if oy < 0.0 {
            Side::Top
        } else {
            Side::Bottom
        };
        pins.push(PinLayout {
            name: pin.name.clone(),
            net: pin.net.clone(),
            number: port_idx as u32 + 1,
            x: center.0 + ox,
            y: center.1 + oy,
            side,
        });
    }
    CompLayout {
        center,
        size: (geom.w, geom.h),
        symbol_name: Some(glyph.name.clone()),
        pins,
    }
}

/// Pin world positions for a box chip: even spacing down each edge.
fn chip_layout_at(
    center: (f64, f64),
    size: (f64, f64),
    left: &[PinDoc],
    right: &[PinDoc],
) -> CompLayout {
    let mut pins = Vec::with_capacity(left.len() + right.len());
    let (x0, y0) = (center.0 - size.0 / 2.0, center.1 - size.1 / 2.0);
    let mut number = 1u32;
    for (side, list, edge_x) in [
        (Side::Left, left, x0),
        (Side::Right, right, x0 + size.0),
    ] {
        let n = list.len();
        for (i, pin) in list.iter().enumerate() {
            pins.push(PinLayout {
                name: pin.name.clone(),
                net: pin.net.clone(),
                number,
                x: edge_x,
                y: y0 + size.1 * (i + 1) as f64 / (n + 1) as f64,
                side,
            });
            number += 1;
        }
    }
    CompLayout {
        center,
        size,
        symbol_name: None,
        pins,
    }
}

pub(crate) fn compute_layout(sch: &SchematicDoc) -> Layout {
    let mut out = Layout {
        comps: BTreeMap::new(),
        modules: BTreeMap::new(),
    };
    let Some(root) = sch.instances.get("root") else {
        return out;
    };

    let components: Vec<&InstanceDoc> = sch
        .instances
        .values()
        .filter(|i| i.kind == InstanceKind::Component)
        .collect();

    // Authored positions win only when they cover every component; a partial
    // set would interleave two coordinate systems. (The drag-to-persist loop
    // relies on this: its first save writes every position at once.)
    let all_authored = !components.is_empty() && components.iter().all(|c| c.position.is_some());
    if all_authored {
        for inst in &components {
            let pos = inst.position.as_ref().expect("checked above");
            let center = (pos.x / AUTHORED_DIVISOR, pos.y / AUTHORED_DIVISOR);
            let layout = match resolve_comp(inst, Orient::from_rotation(pos.rotation)) {
                ResolvedComp::Glyph(glyph, pins) => glyph_layout_at(center, &glyph, &pins),
                ResolvedComp::Chip { left, right } => {
                    chip_layout_at(center, chip_size(&left, &right), &left, &right)
                }
            };
            out.comps.insert(inst.path.clone(), layout);
        }
        // Module boxes from descendant component bounds.
        for inst in sch.instances.values() {
            if inst.kind != InstanceKind::Module || inst.path == "root" {
                continue;
            }
            let prefix = format!("{}.", inst.path);
            let mut bounds: Option<Rect> = None;
            for (path, c) in &out.comps {
                if !path.starts_with(&prefix) {
                    continue;
                }
                let r = Rect {
                    x: c.center.0 - c.size.0 / 2.0,
                    y: c.center.1 - c.size.1 / 2.0,
                    w: c.size.0,
                    h: c.size.1,
                };
                bounds = Some(match bounds {
                    None => r,
                    Some(b) => {
                        let x = b.x.min(r.x);
                        let y = b.y.min(r.y);
                        Rect {
                            x,
                            y,
                            w: (b.x + b.w).max(r.x + r.w) - x,
                            h: (b.y + b.h).max(r.y + r.h) - y,
                        }
                    }
                });
            }
            if let Some(b) = bounds {
                out.modules.insert(
                    inst.path.clone(),
                    Rect {
                        x: b.x - MODULE_PAD,
                        y: b.y - MODULE_TITLE_H - MODULE_PAD,
                        w: b.w + 2.0 * MODULE_PAD,
                        h: b.h + MODULE_TITLE_H + 2.0 * MODULE_PAD,
                    },
                );
            }
        }
        return out;
    }

    let sized = size_node(sch, root);
    place(&sized, 0.0, 0.0, true, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn npn(pins: [&str; 3]) -> InstanceDoc {
        InstanceDoc {
            path: "root.Q1".into(),
            kind: InstanceKind::Component,
            type_name: "Npn".into(),
            source_file: None,
            refdes: Some("Q1".into()),
            attributes: [("type".to_string(), json!("npn"))].into_iter().collect(),
            children: BTreeMap::new(),
            pins: pins
                .iter()
                .map(|n| crate::model::PinDoc {
                    name: (*n).into(),
                    net: None,
                })
                .collect(),
            position: None,
        }
    }

    #[test]
    fn labeled_symbols_map_pins_and_fall_back_safely() {
        // Semantic pin names map onto the transistor glyph; the base pin is a
        // vertical port.
        let inst = npn(["C", "B", "E"]);
        let comp = resolve_comp(&inst, Orient::Right);
        match &comp {
            ResolvedComp::Glyph(glyph, _) => {
                assert_eq!(glyph.name, "npn_bipolar_transistor_right");
                assert_eq!(glyph.pin_for_port.len(), 3);
            }
            ResolvedComp::Chip { .. } => panic!("expected glyph for C/B/E npn"),
        }
        let layout = match comp {
            ResolvedComp::Glyph(g, pins) => glyph_layout_at((0.0, 0.0), &g, &pins),
            _ => unreachable!(),
        };
        let base = layout.pins.iter().find(|p| p.name == "B").unwrap();
        assert_eq!(base.side, Side::Bottom, "base port hangs below the glyph");

        // Unmappable pin names never render a wrong glyph.
        let bad = npn(["X", "Y", "Z"]);
        assert!(matches!(
            resolve_comp(&bad, Orient::Right),
            ResolvedComp::Chip { .. }
        ));
    }

    fn resistor(path: &str, nets: [Option<&str>; 2]) -> InstanceDoc {
        InstanceDoc {
            path: path.into(),
            kind: InstanceKind::Component,
            type_name: "Resistor".into(),
            source_file: None,
            refdes: None,
            attributes: [
                ("type".to_string(), json!("resistor")),
                ("value".to_string(), json!("1k")),
            ]
            .into_iter()
            .collect(),
            children: BTreeMap::new(),
            pins: vec![
                crate::model::PinDoc {
                    name: "P1".into(),
                    net: nets[0].map(String::from),
                },
                crate::model::PinDoc {
                    name: "P2".into(),
                    net: nets[1].map(String::from),
                },
            ],
            position: None,
        }
    }

    fn chain_doc() -> SchematicDoc {
        let mut instances = BTreeMap::new();
        instances.insert(
            "root".to_string(),
            InstanceDoc {
                path: "root".into(),
                kind: InstanceKind::Module,
                type_name: "top".into(),
                source_file: None,
                refdes: None,
                attributes: BTreeMap::new(),
                children: ["RA", "RB", "RC"]
                    .iter()
                    .map(|n| (n.to_string(), format!("root.{n}")))
                    .collect(),
                pins: vec![],
                position: None,
            },
        );
        instances.insert("root.RA".into(), resistor("root.RA", [None, Some("N1")]));
        instances.insert(
            "root.RB".into(),
            resistor("root.RB", [Some("N1"), Some("N2")]),
        );
        instances.insert("root.RC".into(), resistor("root.RC", [Some("N2"), None]));
        let nets = [("N1", "root.RA:P2", "root.RB:P1"), ("N2", "root.RB:P2", "root.RC:P1")]
            .into_iter()
            .map(|(name, a, b)| {
                let port = |s: &str| {
                    let (c, p) = s.split_once(':').unwrap();
                    crate::model::PortRef {
                        component: c.into(),
                        pin: p.into(),
                    }
                };
                (
                    name.to_string(),
                    crate::model::NetDoc {
                        name: name.into(),
                        kind: "Net".into(),
                        ports: vec![port(a), port(b)],
                    },
                )
            })
            .collect();
        SchematicDoc {
            root_module: "top".into(),
            instances,
            nets,
            by_refdes: BTreeMap::new(),
        }
    }

    #[test]
    fn signal_chains_flow_left_to_right() {
        let layout = compute_layout(&chain_doc());
        let x = |p: &str| layout.comps[p].center.0;
        assert!(
            x("root.RA") < x("root.RB") && x("root.RB") < x("root.RC"),
            "chain must read left to right: {} {} {}",
            x("root.RA"),
            x("root.RB"),
            x("root.RC"),
        );
    }

    #[test]
    fn computed_layout_never_overlaps() {
        let layout = compute_layout(&chain_doc());
        let rects: Vec<(&String, Rect)> = layout
            .comps
            .iter()
            .map(|(p, c)| {
                (
                    p,
                    Rect {
                        x: c.center.0 - c.size.0 / 2.0,
                        y: c.center.1 - c.size.1 / 2.0,
                        w: c.size.0,
                        h: c.size.1,
                    },
                )
            })
            .collect();
        for (i, (pa, a)) in rects.iter().enumerate() {
            for (pb, b) in rects.iter().skip(i + 1) {
                let overlap = a.x < b.x + b.w
                    && b.x < a.x + a.w
                    && a.y < b.y + b.h
                    && b.y < a.y + a.h;
                assert!(!overlap, "{pa} overlaps {pb}");
            }
        }
    }
}
