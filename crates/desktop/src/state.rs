use std::path::PathBuf;
use std::sync::Arc;

use agent_host::AgentSession;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use zen_build::BuildSummary;

/// A build request for the single builder task. Watcher fires `reply: None`;
/// MCP `build` and `select_board` want the summary back.
pub struct BuildRequest {
    /// Re-run workspace discovery + dependency resolution first
    /// (needed when pcb.toml changed).
    pub reload: bool,
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

/// Snapshot handed to the UI on demand (late mounts, reloads).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStateSnapshot {
    pub workspace_root: Option<String>,
    pub source: Option<String>,
    pub selection: mcp::Selection,
    pub agent_running: bool,
    pub build: Option<zen_build::BuildOutput>,
}
