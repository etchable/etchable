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
    /// (needed when etchable.toml changed).
    pub reload: bool,
    /// Re-read etchable.toml + component cards and emit `project-changed`.
    pub reload_project: bool,
    /// Run the zen build (false = project-only refresh, no build events).
    pub build: bool,
    pub reply: Option<oneshot::Sender<Result<BuildSummary, String>>>,
}

/// One open project: everything scoped to a single `app-N` window. Each
/// project window owns its own canvas state, builder task, fs watcher,
/// MCP server, and agent session — windows are independent documents.
pub struct AppState {
    /// The `app-N` window this instance renders into; all events emit
    /// to exactly this window.
    pub window_label: String,
    /// Shared with this instance's MCP server; holds build output + selection.
    pub canvas: mcp::SharedState,
    /// Owned by the builder task via this queue.
    pub build_tx: mpsc::Sender<BuildRequest>,
    pub agent: tokio::sync::Mutex<Option<AgentSession>>,
    /// Written once the MCP listener is up.
    pub mcp_config_path: std::sync::OnceLock<PathBuf>,
    /// Aborted on teardown so closed windows don't leak servers.
    pub mcp_server: std::sync::OnceLock<tokio::task::JoinHandle<()>>,
    /// `~/.etchable/state` sqlite, cloned from the registry at instance
    /// creation. `None` = persistence unavailable (open failed at startup,
    /// logged once) — every caller degrades gracefully.
    pub store: Option<store::Store>,
    /// First user message of a not-yet-spawned session; taken by the init
    /// recording in `agent::pump_events` (the session id doesn't exist at
    /// send time, so recording can't happen there).
    pub pending_title: std::sync::Mutex<Option<String>>,
    /// Set by `resume_session`; links the forked session to its ancestor.
    pub pending_resumed_from: std::sync::Mutex<Option<String>>,
    /// Bundled stdlib source (the app's Resources/stdlib), copied from the
    /// registry at instance creation. Unset under `tauri dev` — upstream's
    /// exe-ancestor discovery finds the repo's lib/std there.
    pub stdlib_source: std::sync::OnceLock<PathBuf>,
    /// Keeps the fs watcher alive; replaced when a new board is opened.
    pub watcher: std::sync::Mutex<Option<notify::RecommendedWatcher>>,
    /// Unanswered `can_use_tool` requests. The CLI blocks its turn on these,
    /// so they must outlive the webview: a reload re-materializes the cards
    /// from here (a lost prompt would wedge the session forever).
    pub pending_permissions: std::sync::Mutex<Vec<PendingPermission>>,
    /// First message for the agent, set by the dashboard's "Sketch it" flow
    /// and consumed once by the app window's chat after it mounts.
    pub initial_prompt: std::sync::Mutex<Option<String>>,
    /// Session to `--resume` when the agent next spawns. Resuming loads the
    /// history immediately but deliberately does NOT start the CLI — the
    /// first send does, picking this up.
    pub resume_target: std::sync::Mutex<Option<String>>,
}

pub type SharedAppState = Arc<AppState>;

/// A permission prompt the agent is blocked on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPermission {
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

/// App-global: the map from window label to project instance, plus the
/// startup facts every instance copies.
#[derive(Default)]
pub struct Registry {
    instances: std::sync::Mutex<std::collections::HashMap<String, SharedAppState>>,
    next_id: std::sync::atomic::AtomicU64,
    /// Bundled stdlib (Resources/stdlib), discovered once at startup.
    pub stdlib_source: std::sync::OnceLock<PathBuf>,
    /// Where per-instance mcp-config files are written.
    pub config_dir: std::sync::OnceLock<PathBuf>,
    /// The `~/.etchable/state` sqlite, opened once at startup (None =
    /// running without persistence). Instances clone it — the connection
    /// pool is shared.
    pub store: std::sync::OnceLock<Option<store::Store>>,
    /// Set on ExitRequested so window teardown stops resurrecting the
    /// dashboard mid-quit.
    pub exiting: std::sync::atomic::AtomicBool,
}

impl Registry {
    /// The app-global store, if persistence is available.
    pub fn store(&self) -> Option<&store::Store> {
        self.store.get().and_then(|o| o.as_ref())
    }

    pub fn next_label(&self) -> String {
        let n = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("app-{}", n + 1)
    }

    pub fn insert(&self, state: SharedAppState) {
        self.instances
            .lock()
            .expect("registry lock")
            .insert(state.window_label.clone(), state);
    }

    pub fn get(&self, label: &str) -> Option<SharedAppState> {
        self.instances
            .lock()
            .expect("registry lock")
            .get(label)
            .cloned()
    }

    pub fn remove(&self, label: &str) -> Option<SharedAppState> {
        self.instances.lock().expect("registry lock").remove(label)
    }

    pub fn is_empty(&self) -> bool {
        self.instances.lock().expect("registry lock").is_empty()
    }

    /// An instance already showing this board file, if any (dedup on open).
    pub fn find_by_source(&self, source: &std::path::Path) -> Option<SharedAppState> {
        self.instances
            .lock()
            .expect("registry lock")
            .values()
            .find(|s| s.canvas.read(|c| c.source.as_deref() == Some(source)))
            .cloned()
    }
}

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
    pub pending_permissions: Vec<PendingPermission>,
}

/// Versioned `build-finished` payload / snapshot build state. The UI rejects
/// mismatched versions loudly — bump when the shape changes.
pub const BUILD_PAYLOAD_VERSION: u32 = 4;

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
    /// Which instances/nets structured writers may target (decision 0009);
    /// the canvas greys out edit affordances from this before any gesture.
    pub editability: Option<zen_build::EditabilityDoc>,
}

impl BuildView {
    /// `source_abs` must be the ABSOLUTE board path (canvas state), not
    /// `out.source` (workspace-relative): hashing the relative path only
    /// resolves when the process cwd happens to be the workspace root,
    /// and a `None` hash silently disables drag-to-move persistence.
    pub fn new(out: &zen_build::BuildOutput, source_abs: Option<&std::path::Path>) -> Self {
        let cj = zen_build::to_circuit_json(out);
        let source_hash = source_abs.and_then(|p| zen_build::content_hash(p).ok());
        Self {
            version: BUILD_PAYLOAD_VERSION,
            source: out.source.clone(),
            schematic: out.schematic.clone(),
            diagnostics: out.diagnostics.clone(),
            circuit_json: cj.elements,
            id_map: cj.id_map,
            source_hash,
            editability: out.editability.clone(),
        }
    }
}

#[cfg(test)]
mod build_view_tests {
    use super::*;

    #[test]
    fn source_hash_comes_from_the_absolute_path() {
        let dir = std::env::temp_dir().join(format!("etch-bv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let board = dir.join("board.zen");
        std::fs::write(&board, "# board\n").unwrap();

        let out = zen_build::BuildOutput {
            // Workspace-relative, resolvable only from the workspace root —
            // hashing this instead of the absolute path silently disables
            // drag-to-move persistence.
            source: "board.zen".into(),
            schematic: None,
            diagnostics: vec![],
            editability: None,
        };
        let view = BuildView::new(&out, Some(&board));
        assert_eq!(
            view.source_hash.as_deref(),
            zen_build::content_hash(&board).ok().as_deref()
        );
        assert!(view.source_hash.is_some());

        let none = BuildView::new(&out, None);
        assert_eq!(none.source_hash, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
