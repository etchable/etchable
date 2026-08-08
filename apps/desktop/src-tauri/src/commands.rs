//! Tauri commands — the webview's API surface.

use std::path::PathBuf;

use tauri::{AppHandle, State};
use zen_build::BuildSummary;

use crate::state::{BuildRequest, BuildView, SharedAppState, UiStateSnapshot};
use crate::{agent, builder};

type CmdResult<T> = Result<T, String>;

/// Shared tail of every open flow: point the canvas at a board file, build,
/// then watch from the resolved workspace root.
pub async fn open_board_file(
    state: &SharedAppState,
    entry: PathBuf,
    project: Option<zen_build::ProjectDoc>,
) -> Result<BuildSummary, String> {
    state.canvas.write(|s| {
        s.source = Some(entry);
        s.selection = Default::default();
        s.project = project;
    });

    let summary = state.request_build_and_wait().await?;

    // Watch from the resolved workspace root (set by the builder on open).
    if let Some(root) = state.canvas.read(|s| s.workspace_root.clone()) {
        builder::start_watcher(state, &root).map_err(|e| e.to_string())?;
    }
    Ok(summary)
}

/// Open a bare .zen board (advanced/dev path — no project attached).
#[tauri::command]
pub async fn select_board(
    app: AppHandle,
    state: State<'_, SharedAppState>,
    path: String,
) -> CmdResult<BuildSummary> {
    let path = PathBuf::from(path);
    if !path.exists() || path.extension().is_none_or(|e| e != "zen") {
        return Err(format!("not a .zen file: {}", path.display()));
    }
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let _ = app;
    open_board_file(&state, path, None).await
}

/// Open an etchable project directory (the primary flow): requires
/// `etch.toml`, resolves the board entry, loads part data.
#[tauri::command]
pub async fn open_project(
    state: State<'_, SharedAppState>,
    path: String,
) -> CmdResult<BuildSummary> {
    let dir = PathBuf::from(path);
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
    open_board_file(&state, entry, Some(doc)).await
}

/// Scaffold a fresh project and open it.
#[tauri::command]
pub async fn create_project(
    state: State<'_, SharedAppState>,
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
    open_project(state, result.root.display().to_string()).await
}

/// Snapshot for (re)mounting UIs.
#[tauri::command]
pub async fn get_state(state: State<'_, SharedAppState>) -> CmdResult<UiStateSnapshot> {
    let agent_running = state.agent.lock().await.is_some();
    Ok(state.canvas.read(|s| UiStateSnapshot {
        workspace_root: s.workspace_root.as_ref().map(|p| p.display().to_string()),
        source: s.source.as_ref().map(|p| p.display().to_string()),
        selection: s.selection.clone(),
        agent_running,
        build: s.build.as_ref().map(BuildView::from),
        project: s.project.as_ref().map(crate::state::ProjectView::from),
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
    state: State<'_, SharedAppState>,
    positions: std::collections::BTreeMap<String, PositionIn>,
    base_hash: String,
) -> CmdResult<()> {
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
    state: State<'_, SharedAppState>,
    paths: Vec<String>,
    note: Option<String>,
) -> CmdResult<()> {
    state.canvas.set_selection(mcp::Selection { paths, note });
    Ok(())
}

/// Send a user turn to the agent (spawning it on first use). The current
/// canvas selection rides along as a structured context block.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, SharedAppState>,
    text: String,
) -> CmdResult<()> {
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
    state: State<'_, SharedAppState>,
    request_id: String,
    allow: bool,
    message: Option<String>,
) -> CmdResult<()> {
    let guard = state.agent.lock().await;
    let session = guard.as_ref().ok_or("agent not running")?;
    session
        .respond_permission(&request_id, allow, message.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Best-effort interrupt of the in-flight turn.
#[tauri::command]
pub async fn interrupt_agent(state: State<'_, SharedAppState>) -> CmdResult<()> {
    let guard = state.agent.lock().await;
    let session = guard.as_ref().ok_or("agent not running")?;
    session.interrupt().await.map_err(|e| format!("{e:#}"))
}

/// Kill the current session; the next send_message starts a fresh one.
#[tauri::command]
pub async fn new_session(state: State<'_, SharedAppState>) -> CmdResult<()> {
    let mut guard = state.agent.lock().await;
    if let Some(session) = guard.take() {
        let _ = session.kill().await;
    }
    Ok(())
}

/// Resume a previous CLI session by id (rope-style branching comes later).
#[tauri::command]
pub async fn resume_session(
    app: AppHandle,
    state: State<'_, SharedAppState>,
    session_id: String,
) -> CmdResult<()> {
    {
        let mut guard = state.agent.lock().await;
        if let Some(session) = guard.take() {
            let _ = session.kill().await;
        }
    }
    agent::ensure_session(&app, &state, Some(session_id))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Force a rebuild (the UI's refresh button).
#[tauri::command]
pub async fn rebuild(state: State<'_, SharedAppState>) -> CmdResult<BuildSummary> {
    state.request_build_and_wait().await
}

/// Reload dependencies too (pcb.toml edits outside the watcher's view).
#[tauri::command]
pub async fn reload_workspace(state: State<'_, SharedAppState>) -> CmdResult<()> {
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
