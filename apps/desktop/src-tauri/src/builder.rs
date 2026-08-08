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
pub const PROJECT_CHANGED: &str = "project-changed";

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
            let BuildRequest {
                reload,
                reload_project,
                build,
                reply,
            } = tokio::select! {
                req = build_rx.recv() => match req {
                    Some(r) => r,
                    None => break,
                },
                req = rebuild_rx.recv() => match req {
                    Some(reply) => BuildRequest {
                        reload: false,
                        reload_project: false,
                        build: true,
                        reply: Some(reply),
                    },
                    None => break,
                },
            };

            // Project refresh runs first (and alone when `build` is false):
            // re-read etch.toml + cards, retarget the source if pcb.toml
            // renamed the entry, emit project-changed. No build events fire
            // for project-only refreshes — the canvas doesn't flash.
            if reload_project {
                refresh_project(&app, &state).await;
            }
            if !build {
                debug_assert!(reply.is_none(), "reply without build");
                continue;
            }

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
                let stdlib_source = state.stdlib_source.get().cloned();
                let opened =
                    tokio::task::spawn_blocking(move || open_workspace(&path, stdlib_source))
                        .await;
                match opened {
                    Ok(Ok(ws)) => {
                        state.canvas.write(|s| {
                            s.workspace_root = Some(ws.root().to_path_buf());
                            s.stdlib_dir = Some(ws.stdlib_dir());
                        });
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

/// Open offline first — etchable projects declare no remote dependencies, so
/// this is the normal (and packaged-app) mode. If the offline open fails AND
/// pcb.toml actually declares `[dependencies]`, retry once online.
fn open_workspace(
    path: &Path,
    stdlib_source: Option<PathBuf>,
) -> anyhow::Result<zen_build::Workspace> {
    let opts = zen_build::OpenOptions {
        offline: true,
        stdlib_source,
    };
    match zen_build::Workspace::open_with(path, &opts) {
        Ok(ws) => Ok(ws),
        Err(e) if manifest_declares_dependencies(path) => {
            tracing::warn!("offline open failed ({e:#}); retrying online for declared deps");
            zen_build::Workspace::open_with(
                path,
                &zen_build::OpenOptions {
                    offline: false,
                    ..opts
                },
            )
        }
        Err(e) => Err(e),
    }
}

/// Walk up from the .zen file to the nearest pcb.toml and check for a
/// non-trivial `[dependencies]` table. Textual on purpose: this only gates
/// whether a failed offline open earns one online retry.
fn manifest_declares_dependencies(path: &Path) -> bool {
    let mut dir = if path.is_file() { path.parent() } else { Some(path) };
    while let Some(d) = dir {
        let manifest = d.join("pcb.toml");
        if manifest.is_file() {
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                return false;
            };
            let mut in_deps = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_deps = line == "[dependencies]";
                } else if in_deps && !line.is_empty() && !line.starts_with('#') {
                    return true;
                }
            }
            return false;
        }
        dir = d.parent();
    }
    false
}

/// Re-read the project manifests. Never clears a live project on failure —
/// the failure is appended to the existing doc's problems instead.
async fn refresh_project(app: &AppHandle, state: &SharedAppState) {
    let Some(root) = state
        .canvas
        .read(|s| s.project.as_ref().map(|p| p.root.clone()))
    else {
        return;
    };

    let old_entry = state
        .canvas
        .read(|s| s.project.as_ref().and_then(|p| p.board.clone()));

    let loaded = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || zen_build::load_project(&root)).await
    };
    let view = match loaded {
        Ok(Ok(doc)) => {
            // If pcb.toml renamed the entry and the canvas was on the old
            // one, follow the manifest.
            if let Some(new_board) = &doc.board {
                let new_abs = doc.root.join(new_board);
                let old_abs = old_entry.as_ref().map(|b| root.join(b));
                state.canvas.write(|s| {
                    if s.source == old_abs && s.source.as_ref() != Some(&new_abs) {
                        s.source = Some(new_abs.clone());
                    }
                });
            }
            state.canvas.write(|s| {
                s.project = Some(doc);
                s.project.as_ref().map(crate::state::ProjectView::from)
            })
        }
        Ok(Err(e)) => state.canvas.write(|s| {
            if let Some(p) = &mut s.project {
                p.problems.push(format!("project reload failed: {e:#}"));
            }
            s.project.as_ref().map(crate::state::ProjectView::from)
        }),
        Err(e) => {
            tracing::warn!("project reload panicked: {e}");
            None
        }
    };
    if let Some(view) = view {
        let _ = app.emit(PROJECT_CHANGED, &view);
    }
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
            if let Some(req) = relevant.into_request() {
                let _ = build_tx.blocking_send(req);
            }
        }
    });

    *state.watcher.lock().expect("watcher lock") = Some(watcher);
    Ok(())
}

/// What a batch of fs events touched. Orthogonal — a debounce window can
/// contain zen edits AND card edits.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Relevance {
    /// `*.zen` — rebuild.
    sources: bool,
    /// `pcb.toml` — reopen the workspace (deps/entry may have changed).
    manifest: bool,
    /// `etch.toml`, component cards, datasheets — re-read the project.
    project: bool,
}

impl Relevance {
    fn merge(self, other: Relevance) -> Relevance {
        Relevance {
            sources: self.sources || other.sources,
            manifest: self.manifest || other.manifest,
            project: self.project || other.project,
        }
    }

    fn into_request(self) -> Option<BuildRequest> {
        if self == Relevance::default() {
            return None;
        }
        Some(BuildRequest {
            reload: self.manifest,
            // pcb.toml also carries the project's name/entry.
            reload_project: self.project || self.manifest,
            build: self.sources || self.manifest,
            reply: None,
        })
    }
}

fn classify(event: &notify::Result<notify::Event>) -> Relevance {
    let Ok(event) = event else {
        return Relevance::default();
    };
    let mut relevance = Relevance::default();
    for path in &event.paths {
        // Ignore anything inside hidden dirs (.pcb cache, .git).
        if path.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|s| s.starts_with('.') && s.len() > 1)
        }) {
            continue;
        }
        let in_datasheets = path
            .components()
            .any(|c| c.as_os_str().to_str() == Some("datasheets"));
        if path.file_name().is_some_and(|f| f == "pcb.toml") {
            relevance.manifest = true;
        } else if path.extension().is_some_and(|e| e == "zen") {
            relevance.sources = true;
        } else if path.extension().is_some_and(|e| e == "toml") || in_datasheets {
            // etch.toml, component cards, or datasheet presence (cards
            // default their datasheet path from the file's existence).
            relevance.project = true;
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
                reload_project: false,
                build: true,
                reply: Some(tx),
            })
            .await
            .map_err(|_| "builder stopped".to_string())?;
        rx.await.map_err(|_| "build reply dropped".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(paths: &[&str]) -> notify::Result<notify::Event> {
        let mut ev = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Any,
        ));
        ev.paths = paths.iter().map(std::path::PathBuf::from).collect();
        Ok(ev)
    }

    #[test]
    fn classify_routes_by_file_kind() {
        assert_eq!(
            classify(&event(&["/p/board.zen"])),
            Relevance { sources: true, ..Default::default() }
        );
        assert_eq!(
            classify(&event(&["/p/pcb.toml"])),
            Relevance { manifest: true, ..Default::default() }
        );
        assert_eq!(
            classify(&event(&["/p/etch.toml"])),
            Relevance { project: true, ..Default::default() }
        );
        assert_eq!(
            classify(&event(&["/p/components/ldo.toml"])),
            Relevance { project: true, ..Default::default() }
        );
        assert_eq!(
            classify(&event(&["/p/datasheets/ldo.pdf"])),
            Relevance { project: true, ..Default::default() }
        );
        // Hidden dirs ignored; unknown files ignored.
        assert_eq!(classify(&event(&["/p/.pcb/x.zen"])), Relevance::default());
        assert_eq!(classify(&event(&["/p/readme.md"])), Relevance::default());
    }

    #[test]
    fn merged_relevance_builds_the_right_request() {
        let merged = classify(&event(&["/p/board.zen"]))
            .merge(classify(&event(&["/p/components/ldo.toml"])));
        let req = merged.into_request().expect("relevant");
        assert!(req.build && req.reload_project && !req.reload);

        // Manifest changes imply project reload + workspace reload + build.
        let req = classify(&event(&["/p/pcb.toml"]))
            .into_request()
            .expect("relevant");
        assert!(req.build && req.reload && req.reload_project);

        // Project-only: no build.
        let req = classify(&event(&["/p/etch.toml"]))
            .into_request()
            .expect("relevant");
        assert!(!req.build && req.reload_project && !req.reload);

        assert!(Relevance::default().into_request().is_none());
    }
}
