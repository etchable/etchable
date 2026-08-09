//! Tauri commands — the webview's API surface.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use tokio::sync::mpsc;
use zen_build::BuildSummary;

use crate::state::{AppState, BuildRequest, BuildView, Registry, SharedAppState, UiStateSnapshot};
use crate::{agent, builder};

type CmdResult<T> = Result<T, String>;

/// Bring one project window forward and tuck the dashboard away.
fn show_project_window(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
    }
    if let Some(w) = app.get_webview_window("dashboard") {
        let _ = w.hide();
    }
}

/// Bring the dashboard window forward (project windows stay).
#[tauri::command]
pub fn show_dashboard(app: AppHandle) -> CmdResult<()> {
    let w = app
        .get_webview_window("dashboard")
        .ok_or("no dashboard window")?;
    w.show().map_err(|e| e.to_string())?;
    w.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Stand up a fresh project instance: its own builder task and MCP server,
/// registered under a fresh `app-N` label. The window itself is created
/// later, after the first build succeeds.
async fn create_instance(app: &AppHandle, registry: &Registry) -> Result<SharedAppState, String> {
    let label = registry.next_label();
    let (build_tx, build_rx) = mpsc::channel::<BuildRequest>(16);
    let (rebuild_tx, rebuild_rx) = mpsc::channel::<mcp::RebuildRequest>(16);

    let state: SharedAppState = Arc::new(AppState {
        window_label: label.clone(),
        canvas: mcp::SharedState::new(rebuild_tx),
        build_tx,
        agent: tokio::sync::Mutex::new(None),
        mcp_config_path: std::sync::OnceLock::new(),
        mcp_server: std::sync::OnceLock::new(),
        store: registry.store().cloned(),
        pending_title: std::sync::Mutex::new(None),
        pending_resumed_from: std::sync::Mutex::new(None),
        stdlib_source: std::sync::OnceLock::new(),
        watcher: std::sync::Mutex::new(None),
        pending_permissions: std::sync::Mutex::new(Vec::new()),
    });
    if let Some(stdlib) = registry.stdlib_source.get() {
        let _ = state.stdlib_source.set(stdlib.clone());
    }

    builder::spawn_builder(app.clone(), state.clone(), build_rx, rebuild_rx);

    // Per-instance MCP server + config file — the agent for this window
    // must see this window's canvas, not a global one.
    let (addr, server) = mcp::serve(state.canvas.clone())
        .await
        .map_err(|e| format!("mcp server failed to start: {e:#}"))?;
    tracing::info!("mcp server for {label} on http://{addr}/mcp");
    let _ = state.mcp_server.set(server);
    let config_dir = registry
        .config_dir
        .get()
        .cloned()
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&config_dir).map_err(|e| format!("cannot create config dir: {e}"))?;
    // Pid-prefixed: two app processes must never clobber each other's
    // configs (labels restart at app-1 in every process).
    let config_path = config_dir.join(format!("mcp-config-{}-{label}.json", std::process::id()));
    std::fs::write(&config_path, mcp::mcp_config_json(addr).to_string())
        .map_err(|e| format!("cannot write mcp config: {e}"))?;
    let _ = state.mcp_config_path.set(config_path);

    registry.insert(state.clone());
    Ok(state)
}

/// Undo `create_instance`: abort the MCP server, kill the agent, drop the
/// watcher and builder (their channels close with the state). Called when a
/// project window is destroyed, and when an open flow fails half-way.
pub fn teardown_instance(registry: &Registry, label: &str) {
    let Some(state) = registry.remove(label) else {
        return;
    };
    if let Some(server) = state.mcp_server.get() {
        server.abort();
    }
    if let Some(path) = state.mcp_config_path.get() {
        let _ = std::fs::remove_file(path);
    }
    tauri::async_runtime::spawn(async move {
        let mut guard = state.agent.lock().await;
        if let Some(session) = guard.take() {
            let _ = session.kill().await;
        }
    });
}

/// Shared tail of every open flow: spin up a project instance for the board,
/// build, watch, then open a new project window for it (replacing the
/// dashboard on screen). A board that's already open focuses its existing
/// window instead — no duplicate watchers or agents on one file.
pub async fn open_board_file(
    app: &AppHandle,
    registry: &Registry,
    entry: PathBuf,
    project: Option<zen_build::ProjectDoc>,
) -> Result<BuildSummary, String> {
    // Record real projects in recents before anything can fail — a broken
    // build is still a project the user wants back. Bare-.zen opens
    // (project: None) are the dev path and would pollute recents.
    if let (Some(store), Some(doc)) = (registry.store(), &project) {
        if let Err(e) = store
            .record_project_opened(
                &doc.root.display().to_string(),
                &doc.name,
                doc.board.as_deref(),
            )
            .await
        {
            tracing::warn!("recents recording failed: {e:#}");
        }
    }

    if let Some(existing) = registry.find_by_source(&entry) {
        show_project_window(app, &existing.window_label);
        // Re-opening doubles as a refresh.
        return existing.request_build_and_wait().await;
    }

    let state = create_instance(app, registry).await?;
    let label = state.window_label.clone();
    state.canvas.write(|s| {
        s.source = Some(entry);
        s.selection = Default::default();
        s.project = project;
    });

    let opened: Result<BuildSummary, String> = async {
        let summary = state.request_build_and_wait().await?;
        // Watch from the resolved workspace root (set by the builder on open).
        if let Some(root) = state.canvas.read(|s| s.workspace_root.clone()) {
            builder::start_watcher(&state, &root).map_err(|e| e.to_string())?;
        }
        Ok(summary)
    }
    .await;

    let summary = match opened {
        Ok(s) => s,
        Err(e) => {
            teardown_instance(registry, &label);
            return Err(e);
        }
    };

    if let Err(e) = create_project_window(app, &label) {
        teardown_instance(registry, &label);
        return Err(e);
    }
    show_project_window(app, &label);
    Ok(summary)
}

/// A project window, cloned from the dashboard's chrome (overlay titlebar,
/// floating traffic lights) but pointed at the workbench page.
fn create_project_window(app: &AppHandle, label: &str) -> Result<(), String> {
    let config = tauri::utils::config::WindowConfig {
        label: label.to_string(),
        url: tauri::WebviewUrl::App("app.html".into()),
        title: "etchable".into(),
        width: 1400.0,
        height: 900.0,
        hidden_title: true,
        ..Default::default()
    };
    let mut builder = tauri::WebviewWindowBuilder::from_config(app, &config)
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .traffic_light_position(tauri::LogicalPosition::new(15.0, 24.0));
    }
    builder
        .build()
        .map_err(|e| format!("cannot create project window: {e}"))?;
    Ok(())
}

/// Open a bare .zen board (advanced/dev path — no project attached).
#[tauri::command]
pub async fn select_board(
    app: AppHandle,
    registry: State<'_, Registry>,
    path: String,
) -> CmdResult<BuildSummary> {
    let path = PathBuf::from(path);
    if !path.exists() || path.extension().is_none_or(|e| e != "zen") {
        return Err(format!("not a .zen file: {}", path.display()));
    }
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    open_board_file(&app, &registry, path, None).await
}

/// Open an etchable project (the primary flow): requires `etchable.toml`,
/// resolves the board entry, loads part data. Accepts either the project
/// directory or the `etchable.toml` file itself (what the file picker
/// selects).
#[tauri::command]
pub async fn open_project(
    app: AppHandle,
    registry: State<'_, Registry>,
    path: String,
) -> CmdResult<BuildSummary> {
    let mut dir = PathBuf::from(path);
    if dir.is_file() {
        if dir.file_name().is_none_or(|f| f != zen_build::ETCH_MANIFEST) {
            return Err(format!(
                "not an etchable project manifest (expected {}): {}",
                zen_build::ETCH_MANIFEST,
                dir.display()
            ));
        }
        let Some(parent) = dir.parent() else {
            return Err(format!("manifest has no parent directory: {}", dir.display()));
        };
        dir = parent.to_path_buf();
    }
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    let doc = {
        let dir = dir.clone();
        tauri::async_runtime::spawn_blocking(move || zen_build::load_project(&dir))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?
    };
    let Some(board) = &doc.board else {
        let why = doc
            .problems
            .iter()
            .find(|p| p.contains("board entry"))
            .cloned()
            .unwrap_or_else(|| "cannot determine the board entry".into());
        return Err(why);
    };
    let entry = doc.root.join(board);
    if !entry.is_file() || entry.extension().is_none_or(|e| e != "zen") {
        return Err(format!("board entry is not a .zen file: {}", entry.display()));
    }
    open_board_file(&app, &registry, entry, Some(doc)).await
}

/// Scaffold a fresh project and open it.
#[tauri::command]
pub async fn create_project(
    app: AppHandle,
    registry: State<'_, Registry>,
    parent: String,
    name: String,
) -> CmdResult<BuildSummary> {
    let parent = PathBuf::from(parent);
    let result = {
        let (parent, name) = (parent.clone(), name.clone());
        tauri::async_runtime::spawn_blocking(move || {
            zen_build::scaffold_project_detailed(&parent, &name)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?
    };
    if !result.git_initialized {
        tracing::warn!("scaffolded {} without a git repo", result.root.display());
    }
    open_project(app, registry, result.root.display().to_string()).await
}

/// The instance behind the calling window — every per-project command
/// resolves through this, so each window only ever sees its own project.
fn instance(registry: &Registry, window: &tauri::WebviewWindow) -> CmdResult<SharedAppState> {
    registry
        .get(window.label())
        .ok_or_else(|| format!("no project open in window {}", window.label()))
}

/// Snapshot for (re)mounting UIs.
#[tauri::command]
pub async fn get_state(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<UiStateSnapshot> {
    let state = instance(&registry, &window)?;
    let agent_running = state.agent.lock().await.is_some();
    let pending_permissions = state
        .pending_permissions
        .lock()
        .expect("pending permissions lock")
        .clone();
    Ok(state.canvas.read(|s| UiStateSnapshot {
        workspace_root: s.workspace_root.as_ref().map(|p| p.display().to_string()),
        source: s.source.as_ref().map(|p| p.display().to_string()),
        selection: s.selection.clone(),
        agent_running,
        build: s.build.as_ref().map(BuildView::from),
        project: s.project.as_ref().map(crate::state::ProjectView::from),
        pending_permissions: pending_permissions.clone(),
    }))
}

#[derive(serde::Deserialize)]
pub struct PositionIn {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default)]
    pub mirror: Option<String>,
}

/// Persist authored positions into the open board file as a trailing
/// `# pcb:sch` block. Save-all by design: the layout's all-or-nothing
/// authored rule expects every component's position in one write. The fs
/// watcher picks the write up and rebuilds — that rebuild IS the edit loop's
/// confirmation. `base_hash` is the `source_hash` of the build the edit was
/// made against; a mismatch means someone (likely the agent) changed the
/// file since, and the stale edit is rejected.
#[tauri::command]
pub fn save_positions(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    positions: std::collections::BTreeMap<String, PositionIn>,
    base_hash: String,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    let Some(source) = state.canvas.read(|s| s.source.clone()) else {
        return Err("no board open".into());
    };
    let current = zen_build::content_hash(&source).map_err(|e| e.to_string())?;
    if current != base_hash {
        return Err("content modified".into());
    }
    let map: std::collections::BTreeMap<String, zen_build::PositionDoc> = positions
        .into_iter()
        .map(|(path, p)| {
            let key = path
                .strip_prefix("root.")
                .ok_or_else(|| format!("not an instance path: {path}"))?
                .to_string();
            Ok((
                key,
                zen_build::PositionDoc {
                    x: p.x,
                    y: p.y,
                    rotation: p.rotation,
                    mirror: p.mirror,
                },
            ))
        })
        .collect::<Result<_, String>>()?;
    if map.is_empty() {
        return Err("no positions to save".into());
    }
    zen_build::write_positions(&source, &map).map_err(|e| e.to_string())
}

/// Canvas selection changed. Paths are instance paths and/or net names.
#[tauri::command]
pub fn set_selection(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    paths: Vec<String>,
    note: Option<String>,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    state.canvas.set_selection(mcp::Selection { paths, note });
    Ok(())
}

/// Send a user turn to the agent (spawning it on first use). The current
/// canvas selection rides along as a structured context block.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    text: String,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    // If this message is about to spawn a session, it becomes the session's
    // title — parked here because the session id only exists once the init
    // event arrives (recorded in agent::pump_events).
    if state.agent.lock().await.is_none() {
        *state.pending_title.lock().expect("pending_title") = Some(text.clone());
    }
    agent::ensure_session(&app, &state, None)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let context = agent::selection_context(&state);
    let guard = state.agent.lock().await;
    let session = guard.as_ref().ok_or("agent not running")?;
    session
        .send_user_message(&text, context.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Answer an inline permission prompt.
#[tauri::command]
pub async fn respond_permission(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    request_id: String,
    allow: bool,
    message: Option<String>,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    let guard = state.agent.lock().await;
    let session = guard.as_ref().ok_or("agent not running")?;
    session
        .respond_permission(&request_id, allow, message.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))?;
    state
        .pending_permissions
        .lock()
        .expect("pending permissions lock")
        .retain(|p| p.request_id != request_id);
    Ok(())
}

/// Best-effort interrupt of the in-flight turn.
#[tauri::command]
pub async fn interrupt_agent(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    let guard = state.agent.lock().await;
    let session = guard.as_ref().ok_or("agent not running")?;
    session.interrupt().await.map_err(|e| format!("{e:#}"))
}

/// Kill the current session; the next send_message starts a fresh one.
#[tauri::command]
pub async fn new_session(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    let mut guard = state.agent.lock().await;
    if let Some(session) = guard.take() {
        let _ = session.kill().await;
    }
    state
        .pending_permissions
        .lock()
        .expect("pending permissions lock")
        .clear();
    Ok(())
}

/// Resume a previous CLI session by id (rope-style branching comes later).
#[tauri::command]
pub async fn resume_session(
    app: AppHandle,
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    session_id: String,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    {
        let mut guard = state.agent.lock().await;
        if let Some(session) = guard.take() {
            let _ = session.kill().await;
        }
    }
    // `--resume` forks a NEW session id; this links the fork to its
    // ancestor so the old row is hidden from listings.
    *state
        .pending_resumed_from
        .lock()
        .expect("pending_resumed_from") = Some(session_id.clone());
    agent::ensure_session(&app, &state, Some(session_id))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Force a rebuild (the UI's refresh button).
#[tauri::command]
pub async fn rebuild(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<BuildSummary> {
    let state = instance(&registry, &window)?;
    state.request_build_and_wait().await
}

/// Reload the workspace too (manifest edits outside the watcher's view).
#[tauri::command]
pub async fn reload_workspace(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    state
        .build_tx
        .send(BuildRequest {
            reload: true,
            reload_project: true,
            build: true,
            reply: None,
        })
        .await
        .map_err(|_| "builder stopped".to_string())
}

// --- local storage (docs/decisions/0005) -----------------------------------
// App-global: these resolve through the Registry's store, not a window
// instance — the dashboard has no instance. All degrade to empty/no-op
// when the store failed to open.

/// Recently opened projects, newest first (the dashboard's list).
#[tauri::command]
pub async fn list_recent_projects(
    registry: State<'_, Registry>,
) -> CmdResult<Vec<store::RecentProject>> {
    match registry.store() {
        Some(s) => s.recent_projects(20).await.map_err(|e| format!("{e:#}")),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
pub async fn remove_recent_project(
    registry: State<'_, Registry>,
    root: String,
) -> CmdResult<()> {
    match registry.store() {
        Some(s) => s
            .remove_recent_project(&root)
            .await
            .map_err(|e| format!("{e:#}")),
        None => Ok(()),
    }
}

/// Resumable agent sessions for the given workspace, newest first
/// (superseded resume-ancestors hidden).
#[tauri::command]
pub async fn list_sessions(
    registry: State<'_, Registry>,
    workspace_root: String,
) -> CmdResult<Vec<store::SessionSummary>> {
    match registry.store() {
        Some(s) => s
            .sessions_for(&workspace_root, 20)
            .await
            .map_err(|e| format!("{e:#}")),
        None => Ok(Vec::new()),
    }
}

/// The whole prefs table (tiny; one invoke hydrates a window).
#[tauri::command]
pub async fn get_prefs(
    registry: State<'_, Registry>,
) -> CmdResult<std::collections::BTreeMap<String, serde_json::Value>> {
    match registry.store() {
        Some(s) => s.get_prefs().await.map_err(|e| format!("{e:#}")),
        None => Ok(Default::default()),
    }
}

#[tauri::command]
pub async fn set_pref(
    registry: State<'_, Registry>,
    key: String,
    value: serde_json::Value,
) -> CmdResult<()> {
    match registry.store() {
        Some(s) => s.set_pref(&key, &value).await.map_err(|e| format!("{e:#}")),
        None => Ok(()),
    }
}
