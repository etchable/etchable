//! The application menu.
//!
//! Most items are the frontend's work (it owns the file dialogs, the canvas
//! camera, the dashboard's new-project form), so those emit `menu-action` to
//! the focused window and the webview does the rest — one place per action
//! instead of a second implementation behind the menu bar. Only the things that
//! are genuinely backend-side (revealing a project, rebuilding the recents
//! list) are handled here.
//!
//! Deliberately ABSENT: Undo/Redo. A menu accelerator wins over the webview, so
//! putting Cmd+Z here would take it from the canvas — and the canvas already
//! does the better thing, handing Cmd+Z to the focused text field when you are
//! typing (see the isTyping guard in CircuitCanvas) and undoing a board gesture
//! otherwise. Clipboard items stay, since text fields need them.

use tauri::menu::{Menu, MenuEvent, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::state::Registry;

/// How many recent projects the Open Recent submenu shows.
const RECENTS: usize = 10;

/// Emitted to the focused window; the payload is the menu item's id.
const MENU_ACTION: &str = "menu-action";

pub fn build(app: &AppHandle, recents: &[(String, String)]) -> tauri::Result<Menu<Wry>> {
    let app_menu = SubmenuBuilder::new(app, "etchable")
        .about(None)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let mut recent = SubmenuBuilder::new(app, "Open Recent");
    if recents.is_empty() {
        recent = recent.item(
            &MenuItemBuilder::with_id("recent-none", "No Recent Projects")
                .enabled(false)
                .build(app)?,
        );
    } else {
        for (root, name) in recents.iter().take(RECENTS) {
            // The id carries the path, so the handler needs no side table.
            recent = recent.item(
                &MenuItemBuilder::with_id(format!("recent:{root}"), name).build(app)?,
            );
        }
        recent = recent.separator().item(
            &MenuItemBuilder::with_id("recent-clear", "Clear Menu").build(app)?,
        );
    }
    let recent = recent.build()?;

    let file = SubmenuBuilder::new(app, "File")
        .item(
            &MenuItemBuilder::with_id("new-project", "New Project…")
                .accelerator("CmdOrCtrl+Shift+N")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id("open-project", "Open Project…")
                .accelerator("CmdOrCtrl+O")
                .build(app)?,
        )
        .item(&recent)
        .separator()
        .item(
            &MenuItemBuilder::with_id("rebuild", "Rebuild")
                .accelerator("CmdOrCtrl+R")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id("reveal", "Reveal Project in Finder")
                .accelerator("CmdOrCtrl+Shift+R")
                .build(app)?,
        )
        .separator()
        .close_window()
        .build()?;

    let edit = SubmenuBuilder::new(app, "Edit")
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let view = SubmenuBuilder::new(app, "View")
        .item(
            &MenuItemBuilder::with_id("zoom-in", "Zoom In")
                .accelerator("CmdOrCtrl+=")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id("zoom-out", "Zoom Out")
                .accelerator("CmdOrCtrl+-")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id("zoom-fit", "Zoom to Fit")
                .accelerator("CmdOrCtrl+0")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id("show-dashboard", "Show Dashboard")
                .accelerator("CmdOrCtrl+Shift+D")
                .build(app)?,
        )
        .build()?;

    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .fullscreen()
        .build()?;

    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window])
}

/// Rebuild and install the menu, picking up the current recents list.
///
/// Async because reading recents is: calling this from a synchronous context on
/// the async runtime (as an earlier version did via `block_on`) panics with
/// "cannot start a runtime from within a runtime", and the only symptom was an
/// Open Recent list that silently never updated.
pub async fn refresh(app: &AppHandle) {
    let recents = match app.state::<Registry>().store().cloned() {
        Some(store) => store
            .recent_projects(RECENTS as u64)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.root, r.name))
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };
    install(app, &recents);
}

/// Install a menu built from an already-fetched recents list. Safe from
/// anywhere — no awaiting, no blocking.
pub fn install(app: &AppHandle, recents: &[(String, String)]) {
    match build(app, recents) {
        Ok(menu) => {
            if let Err(e) = app.set_menu(menu) {
                tracing::warn!("installing the menu failed: {e:#}");
            }
        }
        Err(e) => tracing::warn!("building the menu failed: {e:#}"),
    }
}

/// Fire-and-forget refresh for synchronous callers (menu handlers, setup).
pub fn refresh_soon(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        refresh(&app).await;
    });
}

pub fn handle(app: &AppHandle, event: MenuEvent) {
    let id = event.id().0.as_str();

    // Opening a recent project needs no window: the backend funnel handles
    // deduping and window creation.
    if let Some(root) = id.strip_prefix("recent:") {
        let root = std::path::PathBuf::from(root);
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let entry = match zen_build::load_project(&root) {
                Ok(doc) => match &doc.board {
                    Some(b) => (doc.root.join(b), Some(doc)),
                    None => {
                        tracing::warn!("recent {}: no board entry", root.display());
                        return;
                    }
                },
                Err(e) => {
                    tracing::warn!("recent {}: {e:#}", root.display());
                    return;
                }
            };
            let registry = handle.state::<Registry>();
            if let Err(e) =
                crate::commands::open_board_file(&handle, &registry, entry.0, entry.1).await
            {
                tracing::warn!("opening a recent project failed: {e}");
            }
        });
        return;
    }

    match id {
        "recent-clear" => {
            let registry = app.state::<Registry>();
            if let Some(store) = registry.store().cloned() {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    for r in store.recent_projects(200).await.unwrap_or_default() {
                        let _ = store.remove_recent_project(&r.root).await;
                    }
                    refresh(&handle).await;
                });
            }
        }
        "reveal" => reveal_project(app),
        // Everything else is the webview's: it owns the dialogs, the camera and
        // the dashboard's forms.
        _ => {
            let target = app
                .webview_windows()
                .into_iter()
                .find(|(_, w)| w.is_focused().unwrap_or(false))
                .map(|(label, _)| label);
            match target {
                Some(label) => {
                    let _ = app.emit_to(tauri::EventTarget::webview_window(&label), MENU_ACTION, id);
                }
                // No focused window (menu used with everything hidden): the
                // dashboard is the only sensible audience.
                None => {
                    if let Some(dash) = app.get_webview_window("dashboard") {
                        let _ = dash.show();
                        let _ = dash.set_focus();
                        let _ = app.emit_to(
                            tauri::EventTarget::webview_window("dashboard"),
                            MENU_ACTION,
                            id,
                        );
                    }
                }
            }
        }
    }
}

/// Show the focused project's folder in the file manager.
fn reveal_project(app: &AppHandle) {
    let registry = app.state::<Registry>();
    let root = app
        .webview_windows()
        .into_iter()
        .find(|(_, w)| w.is_focused().unwrap_or(false))
        .and_then(|(label, _)| registry.get(&label))
        .and_then(|s| s.canvas.read(|c| c.workspace_root.clone()));
    let Some(root) = root else {
        tracing::info!("reveal: no project window focused");
        return;
    };
    if let Err(e) = tauri_plugin_opener::open_path(root.display().to_string(), None::<&str>) {
        tracing::warn!("reveal failed: {e:#}");
    }
}
