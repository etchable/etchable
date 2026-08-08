//! The single builder task + fs watcher. All rebuilds funnel through one
//! queue so builds never race; results land in the shared canvas state and
//! fan out to the webview as events.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};
use zen_build::BuildSummary;

use crate::state::{AppState, BuildRequest, BuildView, SharedAppState};

pub const BUILD_STARTED: &str = "build-started";
pub const BUILD_FINISHED: &str = "build-finished";

const DEBOUNCE: Duration = Duration::from_millis(150);

type Reply = Option<oneshot::Sender<Result<BuildSummary, String>>>;

/// Spawn the builder loop. `rebuild_rx` carries MCP-initiated requests;
/// `build_rx` carries watcher/command requests. One loop services both.
pub fn spawn_builder(
    app: AppHandle,
    state: SharedAppState,
    mut build_rx: mpsc::Receiver<BuildRequest>,
    mut rebuild_rx: mpsc::Receiver<mcp::RebuildRequest>,
) {
    tauri::async_runtime::spawn(async move {
        let mut workspace: Option<zen_build::Workspace> = None;
        let mut source: Option<PathBuf> = None;

        loop {
            let BuildRequest { reload, reply } = tokio::select! {
                req = build_rx.recv() => match req {
                    Some(r) => r,
                    None => break,
                },
                req = rebuild_rx.recv() => match req {
                    Some(reply) => BuildRequest { reload: false, reply: Some(reply) },
                    None => break,
                },
            };

            // The canvas state names the board; (re)open lazily so
            // `select_board` only has to update state and poke us.
            let (want_root, want_source) = state
                .canvas
                .read(|s| (s.workspace_root.clone(), s.source.clone()));
            let Some(want_source) = want_source else {
                if let Some(reply) = reply {
                    let _ = reply.send(Err("no board selected".into()));
                }
                continue;
            };

            let need_open = reload
                || workspace.is_none()
                || source.as_deref() != Some(want_source.as_path())
                || workspace.as_ref().map(|w| w.root().to_path_buf()) != want_root;

            let _ = app.emit(BUILD_STARTED, json!({"source": want_source}));

            if need_open {
                let path = want_source.clone();
                let opened =
                    tokio::task::spawn_blocking(move || zen_build::Workspace::open(&path, false))
                        .await;
                match opened {
                    Ok(Ok(ws)) => {
                        state
                            .canvas
                            .write(|s| s.workspace_root = Some(ws.root().to_path_buf()));
                        workspace = Some(ws);
                        source = Some(want_source.clone());
                    }
                    Ok(Err(e)) => {
                        finish_with_failure(&app, &state, reply, format!("{e:#}"));
                        continue;
                    }
                    Err(e) => {
                        finish_with_failure(&app, &state, reply, format!("builder panicked: {e}"));
                        continue;
                    }
                }
            }

            let ws = workspace.take().expect("workspace opened above");
            let src = want_source.clone();
            let result = tokio::task::spawn_blocking(move || {
                let out = ws.build_file(&src, &Default::default());
                (ws, out)
            })
            .await;

            match result {
                Ok((ws, Ok(output))) => {
                    workspace = Some(ws);
                    let summary = BuildSummary::from_output(&output);
                    let view = BuildView::from(&output);
                    state.canvas.set_build(output);
                    let _ = app.emit(BUILD_FINISHED, &view);
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(summary));
                    }
                }
                Ok((ws, Err(e))) => {
                    workspace = Some(ws);
                    finish_with_failure(&app, &state, reply, format!("{e:#}"));
                }
                Err(e) => {
                    finish_with_failure(&app, &state, reply, format!("builder panicked: {e}"));
                }
            }
        }
    });
}

/// Surface infrastructure failures as a synthetic error diagnostic so the UI
/// has one channel for "the build is unhappy"; keep the last good schematic.
fn finish_with_failure(app: &AppHandle, state: &SharedAppState, reply: Reply, msg: String) {
    tracing::warn!("build failed: {msg}");
    let source = state
        .canvas
        .read(|s| s.source.clone())
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let output = zen_build::BuildOutput {
        source,
        schematic: state
            .canvas
            .read(|s| s.build.as_ref().and_then(|b| b.schematic.clone())),
        diagnostics: vec![zen_build::Diag {
            severity: zen_build::Severity::Error,
            message: msg.clone(),
            kind: Some("build.infrastructure".into()),
            file: None,
            line: None,
            col: None,
            end_line: None,
            end_col: None,
            suppressed: false,
            stack: vec![],
        }],
    };
    let view = BuildView::from(&output);
    state.canvas.set_build(output);
    let _ = app.emit(BUILD_FINISHED, &view);
    if let Some(reply) = reply {
        let _ = reply.send(Err(msg));
    }
}

/// Watch the workspace root for `*.zen` / `pcb.toml` changes; debounce and
/// enqueue rebuilds. Replaces any previous watcher.
pub fn start_watcher(state: &SharedAppState, root: &Path) -> anyhow::Result<()> {
    use notify::{RecursiveMode, Watcher};

    let (fs_tx, fs_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(fs_tx)?;
    watcher.watch(root, RecursiveMode::Recursive)?;

    let build_tx = state.build_tx.clone();
    std::thread::spawn(move || {
        loop {
            let first = match fs_rx.recv() {
                Ok(ev) => ev,
                Err(_) => break,
            };
            let mut relevant = classify(&first);
            // Debounce: swallow everything arriving within the quiet window.
            loop {
                match fs_rx.recv_timeout(DEBOUNCE) {
                    Ok(ev) => relevant = relevant.merge(classify(&ev)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            match relevant {
                Relevance::None => continue,
                Relevance::Sources => {
                    let _ = build_tx.blocking_send(BuildRequest {
                        reload: false,
                        reply: None,
                    });
                }
                Relevance::Manifest => {
                    let _ = build_tx.blocking_send(BuildRequest {
                        reload: true,
                        reply: None,
                    });
                }
            }
        }
    });

    *state.watcher.lock().expect("watcher lock") = Some(watcher);
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Relevance {
    None,
    Sources,
    Manifest,
}

impl Relevance {
    fn merge(self, other: Relevance) -> Relevance {
        match (self, other) {
            (Relevance::Manifest, _) | (_, Relevance::Manifest) => Relevance::Manifest,
            (Relevance::Sources, _) | (_, Relevance::Sources) => Relevance::Sources,
            _ => Relevance::None,
        }
    }
}

fn classify(event: &notify::Result<notify::Event>) -> Relevance {
    let Ok(event) = event else {
        return Relevance::None;
    };
    let mut relevance = Relevance::None;
    for path in &event.paths {
        // Ignore anything inside hidden dirs (.pcb cache, .git).
        if path.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|s| s.starts_with('.') && s.len() > 1)
        }) {
            continue;
        }
        if path.file_name().is_some_and(|f| f == "pcb.toml") {
            relevance = relevance.merge(Relevance::Manifest);
        } else if path.extension().is_some_and(|e| e == "zen") {
            relevance = relevance.merge(Relevance::Sources);
        }
    }
    relevance
}

impl AppState {
    pub async fn request_build_and_wait(&self) -> Result<BuildSummary, String> {
        let (tx, rx) = oneshot::channel();
        self.build_tx
            .send(BuildRequest {
                reload: false,
                reply: Some(tx),
            })
            .await
            .map_err(|_| "builder stopped".to_string())?;
        rx.await.map_err(|_| "build reply dropped".to_string())?
    }
}
