//! Tauri commands — the webview's API surface.

use std::path::PathBuf;

use tauri::{AppHandle, State};
use zen_build::BuildSummary;

use crate::state::{BuildRequest, BuildView, SharedAppState, UiStateSnapshot};
use crate::{agent, builder};

type CmdResult<T> = Result<T, String>;

/// Open a .zen board: set it active, start the watcher, kick a build.
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

    state.canvas.write(|s| {
        s.source = Some(path.clone());
        s.selection = Default::default();
    });

    let summary = state.request_build_and_wait().await?;

    // Watch from the resolved workspace root (set by the builder on open).
    if let Some(root) = state.canvas.read(|s| s.workspace_root.clone()) {
        builder::start_watcher(&state, &root).map_err(|e| e.to_string())?;
    }

    let _ = app;
    Ok(summary)
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
    }))
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
            reply: None,
        })
        .await
        .map_err(|_| "builder stopped".to_string())
}
