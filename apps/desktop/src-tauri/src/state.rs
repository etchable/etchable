use std::path::PathBuf;
use std::sync::Arc;

use agent_host::AgentSession;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use zen_build::BuildSummary;

/// A request for the single builder task — orthogonal flags, one queue, so
/// builds and project refreshes never race. Watcher fires `reply: None`;
/// MCP `build` and the open commands want the summary back (`reply` is only
/// meaningful with `build: true`).
pub struct BuildRequest {
    /// Re-run workspace discovery + dependency resolution first
    /// (needed when pcb.toml changed).
    pub reload: bool,
    /// Re-read etch.toml + component cards and emit `project-changed`.
    pub reload_project: bool,
    /// Run the zen build (false = project-only refresh, no build events).
    pub build: bool,
    pub reply: Option<oneshot::Sender<Result<BuildSummary, String>>>,
}

pub struct AppState {
    /// Shared with the MCP server; holds build output + selection.
    pub canvas: mcp::SharedState,
    /// Owned by the builder task via this queue.
    pub build_tx: mpsc::Sender<BuildRequest>,
    pub agent: tokio::sync::Mutex<Option<AgentSession>>,
    /// Written once the MCP listener is up.
    pub mcp_config_path: std::sync::OnceLock<PathBuf>,
    /// Keeps the fs watcher alive; replaced when a new board is opened.
    pub watcher: std::sync::Mutex<Option<notify::RecommendedWatcher>>,
}

pub type SharedAppState = Arc<AppState>;

/// Project summary for the UI (`project-changed` payload + snapshot field).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub name: String,
    pub root: String,
    pub board: Option<String>,
    pub problems: Vec<String>,
}

impl From<&zen_build::ProjectDoc> for ProjectView {
    fn from(doc: &zen_build::ProjectDoc) -> Self {
        Self {
            name: doc.name.clone(),
            root: doc.root.display().to_string(),
            board: doc.board.clone(),
            problems: doc.problems.clone(),
        }
    }
}

/// Snapshot handed to the UI on demand (late mounts, reloads).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStateSnapshot {
    pub workspace_root: Option<String>,
    pub source: Option<String>,
    pub selection: mcp::Selection,
    pub agent_running: bool,
    pub build: Option<BuildView>,
    pub project: Option<ProjectView>,
}

/// Versioned `build-finished` payload / snapshot build state. The UI rejects
/// mismatched versions loudly — bump when the shape changes.
pub const BUILD_PAYLOAD_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize)]
pub struct BuildView {
    pub version: u32,
    pub source: String,
    pub schematic: Option<zen_build::SchematicDoc>,
    pub diagnostics: Vec<zen_build::Diag>,
    /// Circuit JSON element array (the canvas view-model).
    pub circuit_json: Vec<serde_json::Value>,
    /// Circuit JSON id -> instance path (or net name).
    pub id_map: std::collections::BTreeMap<String, String>,
    /// SHA-256 of the board source at build time — the optimistic-concurrency
    /// token `save_positions` requires.
    pub source_hash: Option<String>,
}

impl From<&zen_build::BuildOutput> for BuildView {
    fn from(out: &zen_build::BuildOutput) -> Self {
        let cj = zen_build::to_circuit_json(out);
        let source_hash = zen_build::content_hash(std::path::Path::new(&out.source)).ok();
        Self {
            version: BUILD_PAYLOAD_VERSION,
            source: out.source.clone(),
            schematic: out.schematic.clone(),
            diagnostics: out.diagnostics.clone(),
            circuit_json: cj.elements,
            id_map: cj.id_map,
            source_hash,
        }
    }
}
