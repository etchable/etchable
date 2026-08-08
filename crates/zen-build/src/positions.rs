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
