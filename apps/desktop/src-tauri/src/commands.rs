//! Tauri commands — the webview's API surface.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use tokio::sync::mpsc;
use zen_build::BuildSummary;

use crate::state::{AppState, BuildRequest, BuildView, Registry, SharedAppState, UiStateSnapshot};
use crate::{agent, builder};

type CmdResult<T> = Result<T, String>;

/// Ask for a rebuild NOW instead of waiting out the watcher's debounce —
/// gesture commands call this right after their write so the canvas
/// answers as fast as the build allows. The watcher's own (deterministic,
/// byte-identical) echo is deduped in the frontend.
fn nudge_build(state: &SharedAppState) {
    let _ = state.build_tx.try_send(BuildRequest {
        reload: false,
        reload_project: false,
        build: true,
        reply: None,
    });
}

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
        initial_prompt: std::sync::Mutex::new(None),
        resume_target: std::sync::Mutex::new(None),
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
        min_width: Some(720.0),
        min_height: Some(480.0),
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

/// "Sketch it": scaffold a project for a described board (auto-named,
/// under ~/Documents/Etchable), open it, and queue the description as the
/// agent's first message — consumed by the app window's chat on mount.
#[tauri::command]
pub async fn sketch_board(
    app: AppHandle,
    registry: State<'_, Registry>,
    description: String,
) -> CmdResult<BuildSummary> {
    let description = description.trim().to_string();
    if description.is_empty() {
        return Err("describe the board first".into());
    }

    let parent = app
        .path()
        .document_dir()
        .map_err(|e| format!("no documents dir: {e}"))?
        .join("Etchable");
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;

    let base = slugify(&description);
    let mut name = base.clone();
    let mut n = 2;
    while parent.join(&name).exists() {
        name = format!("{base}-{n}");
        n += 1;
    }

    let result = {
        let (parent, name) = (parent.clone(), name.clone());
        tauri::async_runtime::spawn_blocking(move || {
            zen_build::scaffold_project_detailed(&parent, &name)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?
    };

    let doc = {
        let root = result.root.clone();
        tauri::async_runtime::spawn_blocking(move || zen_build::load_project(&root))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?
    };
    let Some(board) = &doc.board else {
        return Err("scaffold produced no board entry".into());
    };
    let entry = doc.root.join(board);

    let summary = open_board_file(&app, &registry, entry.clone(), Some(doc)).await?;
    if let Some(state) = registry.find_by_source(&entry) {
        *state.initial_prompt.lock().expect("initial prompt lock") =
            Some(format!("Sketch this board: {description}"));
    }
    Ok(summary)
}

/// Project-name slug from a board description ("a USB-C power breakout,
/// 5V at 3A" → "a-usb-c-power-breakout-5v-at-3a").
fn slugify(desc: &str) -> String {
    let mut s: String = desc
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    let s = if s.len() > 40 {
        s[..40].trim_end_matches('-')
    } else {
        s
    };
    if s.is_empty() { "sketch".into() } else { s.to_string() }
}

/// One-shot pickup of the dashboard's "Sketch it" prompt (see sketch_board).
#[tauri::command]
pub fn take_initial_prompt(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<Option<String>> {
    let state = instance(&registry, &window)?;
    let taken = state.initial_prompt.lock().expect("initial prompt lock").take();
    Ok(taken)
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
        build: s
            .build
            .as_ref()
            .map(|b| BuildView::new(b, s.source.as_deref())),
        project: s.project.as_ref().map(crate::state::ProjectView::from),
        pending_permissions: pending_permissions.clone(),
    }))
}

#[derive(serde::Deserialize)]
pub struct MoveIn {
    /// Schematic-space center (get_circuit_json units, y-up).
    pub x: f64,
    pub y: f64,
    /// Degrees; omitted keeps the authored rotation or, for never-authored
    /// components, the DERIVED orientation (rail idioms stand vertical).
    #[serde(default)]
    pub rotation: Option<f64>,
    /// Degrees added to whatever the base rotation resolves to — the
    /// rotate gesture (the client can't know derived bases).
    #[serde(default)]
    pub rotate_by: Option<f64>,
}

/// Persist canvas moves: PARTIAL schematic-space moves merged server-side
/// into the save-all map the layout's all-or-nothing authored rule expects
/// (`merge_positions` fills every unmoved component from its authored or
/// derived spot — including derived ORIENTATION, so the first save never
/// flips rail idioms flat). The fs watcher picks the write up and rebuilds —
/// that rebuild IS the edit loop's confirmation. `base_hash` is the
/// `source_hash` of the build the edit was made against; a mismatch means
/// someone (likely the agent) changed the file since, and the stale edit is
/// rejected.
#[tauri::command]
pub fn save_positions(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    moves: std::collections::BTreeMap<String, MoveIn>,
    base_hash: String,
) -> CmdResult<()> {
    let state = instance(&registry, &window)?;
    let (source, sch) = state.canvas.read(|s| {
        (
            s.source.clone(),
            s.build.as_ref().and_then(|b| b.schematic.clone()),
        )
    });
    let source = source.ok_or("no board open")?;
    if moves.is_empty() {
        return Err("no moves to save".into());
    }
    // Paths the build doesn't know yet (a provisional part on a red board)
    // can't merge — convert them directly; write_positions merges into the
    // block without touching anything else.
    let mut known: std::collections::BTreeMap<String, zen_build::MovedPosition> =
        Default::default();
    let mut extra: std::collections::BTreeMap<String, zen_build::PositionDoc> =
        Default::default();
    for (path, m) in moves {
        if sch.as_ref().is_some_and(|s| s.instance(&path).is_some()) {
            known.insert(
                path,
                zen_build::MovedPosition {
                    x: m.x,
                    y: m.y,
                    rotation: m.rotation,
                    rotate_by: m.rotate_by,
                },
            );
        } else if let Some(key) = path.strip_prefix("root.") {
            extra.insert(
                key.to_string(),
                zen_build::PositionDoc {
                    x: m.x * 25.4,
                    y: -m.y * 25.4,
                    rotation: m.rotation.unwrap_or(0.0) + m.rotate_by.unwrap_or(0.0),
                    mirror: None,
                },
            );
        } else {
            return Err(format!("not an instance path: {path}"));
        }
    }
    // The schematic is only needed to merge KNOWN components; all-unknown
    // moves (a provisional part while the build is hard-failing) go direct.
    let mut full = if known.is_empty() {
        Default::default()
    } else {
        let sch = sch.ok_or("no schematic to merge against")?;
        zen_build::merge_positions(&sch, &known).map_err(|e| e.to_string())?
    };
    full.extend(extra);
    // Through the shared write gate: serialized against the agent's
    // structured writes, hash-guarded, snapshotted for undo.
    state
        .canvas
        .gate()
        .apply("move", &[source.clone()], Some((&source, &base_hash)), || {
            zen_build::write_positions(&source, &full)
        })
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(())
}

/// Place one instance (decision 0009 phase 1): the append-shaped structured
/// writer, through the write gate. `base_hash` is the `source_hash` of the
/// build the ghost was placed against; a mismatch rejects the drop (the
/// canvas re-offers after the rebuild).
#[tauri::command]
pub fn add_instance(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    module: String,
    name: String,
    attrs: Vec<(String, String)>,
    position: Option<zen_build::PlacedPosition>,
    base_hash: String,
) -> CmdResult<zen_build::AddInstanceResult> {
    let state = instance(&registry, &window)?;
    let (source, root, stdlib, sch) = state.canvas.read(|s| {
        (
            s.source.clone(),
            s.workspace_root.clone(),
            s.stdlib_dir.clone().unwrap_or_default(),
            s.build.as_ref().and_then(|b| b.schematic.clone()),
        )
    });
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;
    let req = zen_build::AddInstanceRequest {
        module,
        name,
        attrs,
        position,
    };
    let mut out = None;
    state
        .canvas
        .gate()
        .apply(
            "add_instance",
            &[source.clone()],
            Some((&source, &base_hash)),
            || {
                out = Some(zen_build::add_instance(
                    &source,
                    &root,
                    &stdlib,
                    sch.as_ref(),
                    &req,
                )?);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(out.expect("write ran"))
}

/// Rename an instance: name literal + `# pcb:sch` key migration, one write.
#[tauri::command]
pub fn rename_instance(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    from: String,
    to: String,
    base_hash: String,
) -> CmdResult<zen_build::RenameInstanceResult> {
    let state = instance(&registry, &window)?;
    let (source, root) = state
        .canvas
        .read(|s| (s.source.clone(), s.workspace_root.clone()));
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;
    let mut out = None;
    state
        .canvas
        .gate()
        .apply(
            "rename_instance",
            &[source.clone()],
            Some((&source, &base_hash)),
            || {
                out = Some(zen_build::rename_instance(&source, &root, &from, &to)?);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(out.expect("write ran"))
}

/// Rename a net from the canvas (double-click a label). The defining file
/// resolves server-side from the build's editability map.
#[tauri::command]
pub fn rename_net(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    from: String,
    to: String,
    base_hash: String,
) -> CmdResult<zen_build::RenameNetResult> {
    let state = instance(&registry, &window)?;
    let (source, root, def_file) = state.canvas.read(|s| {
        (
            s.source.clone(),
            s.workspace_root.clone(),
            s.build
                .as_ref()
                .and_then(|b| b.editability.as_ref())
                .and_then(|e| e.nets.get(&from))
                .and_then(|n| n.file.clone()),
        )
    });
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;
    let target = def_file.map(|f| root.join(f)).unwrap_or(source.clone());
    let mut out = None;
    state
        .canvas
        .gate()
        .apply(
            "rename_net",
            &[target.clone()],
            Some((&source, &base_hash)),
            || {
                out = Some(zen_build::rename_net(&target, &root, &from, &to)?);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(out.expect("write ran"))
}

/// Attach a pin to a net (the label/rail gesture): the canvas passes the
/// clicked component's instance path and pin; the anchor call site and its
/// file resolve server-side from editability.
#[tauri::command]
pub fn attach_pin_net(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    instance_path: String,
    pin: String,
    net_name: String,
    kind: String,
    base_hash: String,
) -> CmdResult<zen_build::AttachPinResult> {
    let state = instance(&registry, &window)?;
    // The same fallback-aware resolution as every other wiring door — a
    // provisional part (root.NAME, unknown to a red build) resolves to its
    // top-level name in the entry file.
    let (file, ep) = wire_endpoint(&state, &instance_path, &pin)?;
    let (source, root, stdlib) = state.canvas.read(|s| {
        (
            s.source.clone(),
            s.workspace_root.clone(),
            s.stdlib_dir.clone().unwrap_or_default(),
        )
    });
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;
    let target = root.join(file);
    let req = zen_build::AttachPinRequest {
        instance: ep.instance,
        pin: ep.pin,
        net_name,
        kind,
    };
    let mut out = None;
    state
        .canvas
        .gate()
        .apply(
            "attach_pin_net",
            &[target.clone()],
            Some((&source, &base_hash)),
            || {
                out = Some(zen_build::attach_pin_net(&target, &root, &stdlib, &req)?);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(out.expect("write ran"))
}

/// Resolve a canvas wiring endpoint (component instance path + pin) to the
/// anchor call site's file and local name via editability. Instances the
/// build doesn't know yet (just placed, build still red) fall back to the
/// path's top-level name in the entry file — the writers' name-anchored
/// resolution validates against the CURRENT source either way (a red board
/// stays fully editable, PRD §8).
fn wire_endpoint(
    state: &SharedAppState,
    path: &str,
    pin: &str,
) -> Result<(String, zen_build::PinEndpoint), String> {
    state.canvas.read(|s| {
        let fallback = || -> Result<(String, zen_build::PinEndpoint), String> {
            // Only the provisional shape (`root.NAME`, a just-placed
            // top-level part the build hasn't seen) falls back — deeper
            // unknown paths must NOT silently retarget an ancestor.
            let segs: Vec<&str> = path.split('.').collect();
            let ["root", top] = segs.as_slice() else {
                return Err(format!("no such instance: {path}"));
            };
            let (Some(source), Some(root)) = (&s.source, &s.workspace_root) else {
                return Err("no board open".into());
            };
            let file = source
                .strip_prefix(root)
                .unwrap_or(source)
                .display()
                .to_string();
            Ok((
                file,
                zen_build::PinEndpoint {
                    instance: top.to_string(),
                    pin: pin.to_string(),
                },
            ))
        };
        let Some(ed) = s.build.as_ref().and_then(|b| b.editability.as_ref()) else {
            return fallback();
        };
        let Some(entry) = ed.instances.get(path) else {
            return fallback();
        };
        let anchor = if entry.editable {
            path.to_string()
        } else {
            entry.anchor.clone().ok_or_else(|| {
                entry
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("{path} is not editable"))
            })?
        };
        let file = ed
            .instances
            .get(&anchor)
            .and_then(|a| a.file.clone())
            .ok_or_else(|| format!("no source file resolved for {anchor}"))?;
        let instance = anchor
            .rsplit('.')
            .next()
            .ok_or("bad anchor path")?
            .to_string();
        Ok((
            file,
            zen_build::PinEndpoint {
                instance,
                pin: pin.to_string(),
            },
        ))
    })
}

/// Wire two pins (the drag gesture). Returns the tagged ConnectOutcome —
/// `needs_merge` is not an error; the canvas confirms and retries with
/// allow_merge.
#[tauri::command]
pub fn connect_pins(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    a_path: String,
    a_pin: String,
    b_path: String,
    b_pin: String,
    net: Option<String>,
    allow_merge: bool,
    base_hash: String,
) -> CmdResult<serde_json::Value> {
    let state = instance(&registry, &window)?;
    let (mut a_file, mut a_ep) = wire_endpoint(&state, &a_path, &a_pin)?;
    let (mut b_file, mut b_ep) = wire_endpoint(&state, &b_path, &b_pin)?;
    let (source, root, stdlib, sch) = state.canvas.read(|s| {
        (
            s.source.clone(),
            s.workspace_root.clone(),
            s.stdlib_dir.clone().unwrap_or_default(),
            s.build.as_ref().and_then(|b| b.schematic.clone()),
        )
    });
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;

    // Cross-module wiring resolves the way a human means it: a pin inside
    // a module whose net flows out through a PORT becomes (module, port)
    // at the board level. Only genuinely internal nets refuse.
    let mut via_ports: Vec<String> = Vec::new();
    if a_file != b_file {
        let entry_rel = source
            .strip_prefix(&root)
            .unwrap_or(&source)
            .display()
            .to_string();
        for (file, ep, path, pin) in [
            (&mut a_file, &mut a_ep, &a_path, &a_pin),
            (&mut b_file, &mut b_ep, &b_path, &b_pin),
        ] {
            if *file == entry_rel {
                continue;
            }
            let translated = sch
                .as_ref()
                .map(|s| zen_build::translate_endpoint_via_port(s, &source, &root, path, pin))
                .transpose()
                .map_err(|e| e.to_string())?
                .flatten();
            if let Some(t) = translated {
                via_ports.push(format!("{}.{}", t.instance, t.pin));
                *file = entry_rel.clone();
                *ep = t;
            }
        }
        if a_file != b_file {
            let module = a_path
                .split('.')
                .nth(1)
                .or_else(|| b_path.split('.').nth(1))
                .unwrap_or("the module");
            return Err(format!(
                "that pin's net stays inside {module} and isn't exposed as a port — ask \
                 the agent to expose it (add an io), or wire it inside the module"
            ));
        }
    }
    let target = root.join(&a_file);
    let req = zen_build::ConnectPinsRequest {
        a: a_ep,
        b: b_ep,
        net,
        allow_merge,
    };
    let mut out = None;
    state
        .canvas
        .gate()
        .apply(
            "connect_pins",
            &[target.clone()],
            Some((&source, &base_hash)),
            || {
                out = Some(zen_build::connect_pins(
                    &target,
                    &root,
                    &stdlib,
                    sch.as_ref(),
                    &req,
                )?);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    let mut payload =
        serde_json::to_value(out.expect("write ran")).map_err(|e| e.to_string())?;
    // Teach the model as it works: the toast names the port the wire
    // resolved through.
    if let Some(port) = via_ports.first() {
        payload["via_port"] = serde_json::Value::String(port.clone());
    }
    Ok(payload)
}

/// Detach one pin from its net.
#[tauri::command]
pub fn disconnect_pin(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    instance_path: String,
    pin: String,
    base_hash: String,
) -> CmdResult<zen_build::DisconnectResult> {
    let state = instance(&registry, &window)?;
    let (file, ep) = wire_endpoint(&state, &instance_path, &pin)?;
    let (source, root, stdlib) = state.canvas.read(|s| {
        (
            s.source.clone(),
            s.workspace_root.clone(),
            s.stdlib_dir.clone().unwrap_or_default(),
        )
    });
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;
    let target = root.join(&file);
    let mut out = None;
    state
        .canvas
        .gate()
        .apply(
            "disconnect_pin",
            &[target.clone()],
            Some((&source, &base_hash)),
            || {
                out = Some(zen_build::disconnect_pin(
                    &target,
                    &root,
                    &stdlib,
                    &ep.instance,
                    &ep.pin,
                )?);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(out.expect("write ran"))
}

/// Set one attribute (double-click value edit). Anchor resolves
/// server-side from editability.
#[tauri::command]
pub fn set_attribute(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    instance_path: String,
    key: String,
    value: String,
    base_hash: String,
) -> CmdResult<zen_build::SetAttributeResult> {
    let state = instance(&registry, &window)?;
    let (file, ep) = wire_endpoint(&state, &instance_path, "")?;
    let (source, root, stdlib) = state.canvas.read(|s| {
        (
            s.source.clone(),
            s.workspace_root.clone(),
            s.stdlib_dir.clone().unwrap_or_default(),
        )
    });
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;
    let target = root.join(&file);
    let mut out = None;
    state
        .canvas
        .gate()
        .apply(
            "set_attribute",
            &[target.clone()],
            Some((&source, &base_hash)),
            || {
                out = Some(zen_build::set_attribute(
                    &target,
                    &root,
                    &stdlib,
                    &ep.instance,
                    &key,
                    &value,
                )?);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(out.expect("write ran"))
}

/// Delete the selection (batch, grouped per file — one gate application).
#[tauri::command]
pub fn remove_instances(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    instance_paths: Vec<String>,
    base_hash: String,
) -> CmdResult<Vec<zen_build::RemoveInstancesResult>> {
    let state = instance(&registry, &window)?;
    let (source, root) = state
        .canvas
        .read(|s| (s.source.clone(), s.workspace_root.clone()));
    let source = source.ok_or("no board open")?;
    let root = root.ok_or("no workspace open")?;
    // Resolve every path to its anchor, dedupe, group by file.
    let mut by_file: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for path in &instance_paths {
        let (file, ep) = wire_endpoint(&state, path, "")?;
        let names = by_file.entry(file).or_default();
        if !names.contains(&ep.instance) {
            names.push(ep.instance);
        }
    }
    if by_file.is_empty() {
        return Err("nothing to remove".into());
    }
    let touches: Vec<PathBuf> = by_file.keys().map(|f| root.join(f)).collect();
    let mut out = Vec::new();
    state
        .canvas
        .gate()
        .apply(
            "remove_instances",
            &touches,
            Some((&source, &base_hash)),
            || {
                for (file, names) in &by_file {
                    out.push(zen_build::remove_instances(
                        &root.join(file),
                        &root,
                        names,
                    )?);
                }
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(out)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteGeneric {
    pub name: String,
    /// The Module("…") spec add_instance takes.
    pub spec: String,
    /// Refdes prefix for name suggestions (from the generic's Component).
    pub prefix: Option<String>,
    pub params: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteComponent {
    pub name: String,
    pub spec: String,
    pub description: Option<String>,
    pub lcsc: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteView {
    pub generics: Vec<PaletteGeneric>,
    pub components: Vec<PaletteComponent>,
}

/// Pre-warm the placement preflight when a palette item is ARMED: by the
/// time the user drops, the pin set and geometry are cached and the drop
/// commits without evaluator round-trips inside the click. Returns the
/// part's real outline for the aiming ghost.
#[tauri::command]
pub async fn warm_placement(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    spec: String,
) -> CmdResult<Option<zen_build::GhostGeometry>> {
    let state = instance(&registry, &window)?;
    let (root, stdlib) = state.canvas.read(|s| {
        (
            s.workspace_root.clone(),
            s.stdlib_dir.clone().unwrap_or_default(),
        )
    });
    let Some(root) = root else {
        return Ok(None);
    };
    Ok(
        tokio::task::spawn_blocking(move || zen_build::warm_placement(&root, &stdlib, &spec))
            .await
            .unwrap_or(None),
    )
}

/// The palette's offline tier: stdlib generics (with refdes prefixes) and
/// this project's components.
#[tauri::command]
pub fn get_palette(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<PaletteView> {
    let state = instance(&registry, &window)?;
    let (stdlib, project, root) = state.canvas.read(|s| {
        (
            s.stdlib_dir.clone(),
            s.project.clone(),
            s.workspace_root.clone().unwrap_or_default(),
        )
    });
    let stdlib = stdlib.ok_or("no workspace open yet")?;
    let listing = zen_build::list_library(&stdlib, project.as_ref(), None);
    let generics = listing
        .generics
        .iter()
        .map(|g| {
            let spec = format!("@stdlib/generics/{}.zen", g.name);
            let facts = zen_build::module_facts(&spec, &root, &stdlib);
            PaletteGeneric {
                name: g.name.clone(),
                spec,
                prefix: facts.prefix,
                params: g.params.clone(),
            }
        })
        .collect();
    let components = listing
        .project_components
        .iter()
        .map(|c| PaletteComponent {
            name: c.name.clone(),
            spec: format!("./components/{}.zen", c.name),
            description: c.description.clone(),
            lcsc: c.lcsc.clone(),
        })
        .collect();
    Ok(PaletteView {
        generics,
        components,
    })
}

/// The palette's live tier: JLCPCB assembly search (stock, price,
/// Basic/Extended), ranked Basic-first — the same data the agent's
/// search_parts sees.
#[tauri::command]
pub async fn search_lcsc(query: String) -> CmdResult<serde_json::Value> {
    Ok(mcp::lcsc_tools::search_tier(&query).await)
}

/// Pre-commit part detail (lifecycle, price breaks, CAD-quality probe).
#[tauri::command]
pub async fn lcsc_part_detail(code: String) -> CmdResult<serde_json::Value> {
    Ok(mcp::lcsc_tools::get_part(&code).await)
}

/// Install an LCSC part into components/ (fetch → convert → vendor → card)
/// — the same pipeline as the agent's add_component. Scaffold writes only
/// touch new `components/<name>.*` paths (its own clobber guard applies),
/// so this skips the board-file write gate; the returned component is then
/// placed via `add_instance` like anything else.
#[tauri::command]
pub async fn lcsc_install(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    name: String,
    lcsc: String,
) -> CmdResult<serde_json::Value> {
    let state = instance(&registry, &window)?;
    let root = state
        .canvas
        .read(|s| s.project.as_ref().map(|p| p.root.clone()))
        .ok_or("no project open — installing parts requires an etchable project")?;
    mcp::lcsc_tools::add_component(
        &root,
        &mcp::lcsc_tools::AddLcscArgs {
            name,
            lcsc,
            include_3d: true,
            fetch_datasheet: true,
            overwrite: false,
        },
    )
    .await
}

/// Undo the newest canvas gesture (gate snapshots). Returns the gesture's
/// label; refuses (and drops the entry) if the agent or an editor wrote the
/// file since — invalidate, never clobber. The watcher rebuild confirms.
#[tauri::command]
pub fn undo_gesture(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<String> {
    let state = instance(&registry, &window)?;
    let label = state.canvas.gate().undo().map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(label)
}

#[tauri::command]
pub fn redo_gesture(
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
) -> CmdResult<String> {
    let state = instance(&registry, &window)?;
    let label = state.canvas.gate().redo().map_err(|e| e.to_string())?;
    nudge_build(&state);
    Ok(label)
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
    agent::ensure_session(&app, &state)
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

/// Resume a previous CLI session: load its history into the chat and arm
/// `--resume` for the next send. Deliberately does NOT spawn the CLI — the
/// user may only want to read.
#[tauri::command]
pub async fn resume_session(
    app: AppHandle,
    registry: State<'_, Registry>,
    window: tauri::WebviewWindow,
    session_id: String,
) -> CmdResult<Vec<serde_json::Value>> {
    let state = instance(&registry, &window)?;
    {
        let mut guard = state.agent.lock().await;
        if let Some(session) = guard.take() {
            let _ = session.kill().await;
        }
    }
    state
        .pending_permissions
        .lock()
        .expect("pending permissions lock")
        .clear();
    // `--resume` forks a NEW session id; this links the fork to its
    // ancestor so the old row is hidden from listings (consumed when the
    // next send actually spawns).
    *state
        .pending_resumed_from
        .lock()
        .expect("pending_resumed_from") = Some(session_id.clone());
    *state.resume_target.lock().expect("resume target lock") = Some(session_id.clone());

    let root = state
        .canvas
        .read(|s| s.workspace_root.clone())
        .ok_or("no workspace open")?;
    agent::load_session_history(&app, &root, &session_id)
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
