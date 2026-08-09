//! The per-window write gate (docs/decisions/0009 §5): every structured
//! source write — canvas command or MCP tool — passes through here. The
//! gate serializes writes, enforces the `source_hash` optimistic-concurrency
//! guard, and records before/after byte snapshots per touched file: the raw
//! material of the gesture undo stack (surfaced in a later phase).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One file's bytes around a gated write. `before` is `None` when the file
/// did not exist; `after` is `None` when the write removed it.
#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

/// One gesture = one record, even when it touched several files — undo
/// reverts the whole unit.
#[derive(Debug, Clone)]
pub struct GestureRecord {
    /// Short human label ("move", "connect_pins", …) for the undo UI.
    pub label: String,
    pub files: Vec<FileSnapshot>,
}

#[derive(Debug)]
pub enum WriteError {
    /// The guard hash no longer matches the file — the board changed since
    /// the gesture was computed. Rejected, never misapplied; the caller
    /// re-offers after the rebuild.
    Stale,
    Failed(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The exact string save_positions has always returned on a hash
            // mismatch — existing callers match on it.
            WriteError::Stale => write!(f, "content modified"),
            WriteError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Undo entries kept per window. A session convenience, not history — git
/// is the durable trajectory.
const GATE_DEPTH: usize = 100;

#[derive(Default)]
struct GateInner {
    undo: Vec<GestureRecord>,
    redo: Vec<GestureRecord>,
}

#[derive(Clone, Default)]
pub struct WriteGate {
    records: Arc<Mutex<GateInner>>,
}

impl WriteGate {
    /// Apply one gesture's write under the gate: serialize against other
    /// gated writes, check `guard` (a file and the SHA-256 hex its bytes
    /// must still have), snapshot `touches` before and after `write` runs.
    ///
    /// `write` runs while the gate is held — keep it to the file writes
    /// themselves.
    pub fn apply(
        &self,
        label: &str,
        touches: &[PathBuf],
        guard: Option<(&Path, &str)>,
        write: impl FnOnce() -> anyhow::Result<()>,
    ) -> Result<(), WriteError> {
        let mut inner = self.records.lock().expect("write gate poisoned");

        if let Some((file, expected)) = guard {
            let current = zen_build::content_hash(file)
                .map_err(|e| WriteError::Failed(format!("{e:#}")))?;
            if current != expected {
                return Err(WriteError::Stale);
            }
        }

        let before: Vec<Option<Vec<u8>>> =
            touches.iter().map(|p| std::fs::read(p).ok()).collect();
        write().map_err(|e| WriteError::Failed(format!("{e:#}")))?;
        let files = touches
            .iter()
            .zip(before)
            .map(|(path, before)| FileSnapshot {
                path: path.clone(),
                before,
                after: std::fs::read(path).ok(),
            })
            .collect();

        inner.undo.push(GestureRecord {
            label: label.to_string(),
            files,
        });
        // A fresh gesture forks history; the redo line is gone.
        inner.redo.clear();
        if inner.undo.len() > GATE_DEPTH {
            let drop_count = inner.undo.len() - GATE_DEPTH;
            inner.undo.drain(..drop_count);
        }
        Ok(())
    }

    /// Undo the newest gesture. Every touched file must still hold exactly
    /// the bytes the gesture left (invalidate, never clobber — an agent or
    /// editor write since makes the entry unusable and it is dropped).
    /// Returns the gesture's label.
    pub fn undo(&self) -> Result<String, WriteError> {
        let mut inner = self.records.lock().expect("write gate poisoned");
        let Some(record) = inner.undo.pop() else {
            return Err(WriteError::Failed("nothing to undo".into()));
        };
        Self::restore(&record, true)?;
        let label = record.label.clone();
        inner.redo.push(record);
        Ok(label)
    }

    /// Redo the newest undone gesture. Same invalidation rule against the
    /// `before` bytes.
    pub fn redo(&self) -> Result<String, WriteError> {
        let mut inner = self.records.lock().expect("write gate poisoned");
        let Some(record) = inner.redo.pop() else {
            return Err(WriteError::Failed("nothing to redo".into()));
        };
        Self::restore(&record, false)?;
        let label = record.label.clone();
        inner.undo.push(record);
        Ok(label)
    }

    /// Verify each file matches the expected side, then write the other.
    /// `backward` = undo (expect `after`, restore `before`).
    fn restore(record: &GestureRecord, backward: bool) -> Result<(), WriteError> {
        for f in &record.files {
            let expected = if backward { &f.after } else { &f.before };
            let current = std::fs::read(&f.path).ok();
            if &current != expected {
                return Err(WriteError::Failed(format!(
                    "{} changed since this gesture — {} discarded",
                    f.path.display(),
                    if backward { "undo" } else { "redo" },
                )));
            }
        }
        for f in &record.files {
            let target = if backward { &f.before } else { &f.after };
            match target {
                Some(bytes) => std::fs::write(&f.path, bytes)
                    .map_err(|e| WriteError::Failed(format!("restoring {}: {e}", f.path.display())))?,
                None => {
                    let _ = std::fs::remove_file(&f.path);
                }
            }
        }
        Ok(())
    }

    /// The recorded gestures, newest last (undo-stack raw material).
    pub fn records(&self) -> Vec<GestureRecord> {
        self.records.lock().expect("write gate poisoned").undo.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, content: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("etch-gate-{}-{name}", std::process::id()));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn stale_guard_rejects_without_writing() {
        let f = temp_file("stale.zen", "original");
        let r = WriteGate::default().apply(
            "test",
            &[f.clone()],
            Some((&f, "not-the-hash")),
            || {
                std::fs::write(&f, "clobbered").unwrap();
                Ok(())
            },
        );
        assert!(matches!(r, Err(WriteError::Stale)));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn undo_redo_round_trips_and_invalidates() {
        let f = temp_file("undo.zen", "v1");
        let gate = WriteGate::default();
        gate.apply("edit", &[f.clone()], None, || {
            std::fs::write(&f, "v2")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(gate.undo().unwrap(), "edit");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v1");
        assert_eq!(gate.redo().unwrap(), "edit");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v2");

        // An outside write invalidates: refuse, drop the entry, never clobber.
        gate.undo().unwrap();
        gate.redo().unwrap();
        std::fs::write(&f, "agent-edit").unwrap();
        let err = gate.undo().unwrap_err();
        assert!(err.to_string().contains("changed since"), "{err}");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "agent-edit");
        assert!(gate.undo().unwrap_err().to_string().contains("nothing to undo"));

        // A fresh gesture clears the redo line.
        gate.apply("edit2", &[f.clone()], None, || {
            std::fs::write(&f, "v3")?;
            Ok(())
        })
        .unwrap();
        gate.undo().unwrap();
        gate.apply("edit3", &[f.clone()], None, || {
            std::fs::write(&f, "v4")?;
            Ok(())
        })
        .unwrap();
        assert!(gate.redo().unwrap_err().to_string().contains("nothing to redo"));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn matching_guard_writes_and_snapshots() {
        let f = temp_file("ok.zen", "before");
        let hash = zen_build::content_hash(&f).unwrap();
        let gate = WriteGate::default();
        gate.apply("move", &[f.clone()], Some((&f, &hash)), || {
            std::fs::write(&f, "after")?;
            Ok(())
        })
        .unwrap();
        let records = gate.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].label, "move");
        assert_eq!(records[0].files[0].before.as_deref(), Some(b"before".as_slice()));
        assert_eq!(records[0].files[0].after.as_deref(), Some(b"after".as_slice()));
        let _ = std::fs::remove_file(&f);
    }
}
