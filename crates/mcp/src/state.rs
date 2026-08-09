//! Shared canvas/build state. The desktop app writes; MCP tools read.
//! Rebuild requests flow back to the desktop over a channel so the build
//! pipeline stays single-owner.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use zen_build::{BuildOutput, BuildSummary};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Selection {
    /// Instance paths (e.g. `root.SENSE_DIV.R1.R`) and/or net names.
    pub paths: Vec<String>,
    /// Optional free-text note the user attached to the selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Default)]
pub struct CanvasState {
    pub workspace_root: Option<PathBuf>,
    /// The .zen file currently open on the canvas.
    pub source: Option<PathBuf>,
    pub build: Option<BuildOutput>,
    pub selection: Selection,
    /// The etchable project this board belongs to, when opened as one.
    pub project: Option<zen_build::ProjectDoc>,
    /// SHA-256 of `source` when `build` was recorded — the staleness token
    /// `set_positions` checks so it never merges layout data from an old
    /// build with a newer file.
    pub source_hash: Option<String>,
    /// The materialized stdlib dir, known after the first workspace open.
    pub stdlib_dir: Option<PathBuf>,
    /// Monotonic build counter (bumped on every completed build).
    pub build_seq: u64,
}

/// Ask the desktop to rebuild now; reply arrives on the oneshot.
pub type RebuildRequest = oneshot::Sender<Result<BuildSummary, String>>;

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<CanvasState>>,
    rebuild_tx: mpsc::Sender<RebuildRequest>,
}

impl SharedState {
    pub fn new(rebuild_tx: mpsc::Sender<RebuildRequest>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(CanvasState::default())),
            rebuild_tx,
        }
    }

    pub fn read<R>(&self, f: impl FnOnce(&CanvasState) -> R) -> R {
        f(&self.inner.read().expect("canvas state poisoned"))
    }

    pub fn write<R>(&self, f: impl FnOnce(&mut CanvasState) -> R) -> R {
        f(&mut self.inner.write().expect("canvas state poisoned"))
    }

    pub fn set_build(&self, output: BuildOutput) {
        self.write(|s| {
            s.source_hash = s
                .source
                .as_deref()
                .and_then(|p| zen_build::content_hash(p).ok());
            s.build = Some(output);
            s.build_seq += 1;
        });
    }

    pub fn set_selection(&self, selection: Selection) {
        self.write(|s| s.selection = selection);
    }

    pub async fn request_rebuild(&self) -> Result<BuildSummary, String> {
        let (tx, rx) = oneshot::channel();
        self.rebuild_tx
            .send(tx)
            .await
            .map_err(|_| "rebuild channel closed (no board open?)".to_string())?;
        rx.await
            .map_err(|_| "rebuild request dropped".to_string())?
    }
}
