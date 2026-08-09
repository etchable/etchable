mod agent;
mod builder;
mod commands;
mod state;

use std::sync::Arc;

use tauri::Manager;
use tokio::sync::mpsc;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,etchable=debug".into()),
        )
        .init();

    let (build_tx, build_rx) = mpsc::channel::<state::BuildRequest>(16);
    let (rebuild_tx, rebuild_rx) = mpsc::channel::<mcp::RebuildRequest>(16);

    let app_state: state::SharedAppState = Arc::new(AppState {
        canvas: mcp::SharedState::new(rebuild_tx),
        build_tx,
        agent: tokio::sync::Mutex::new(None),
        mcp_config_path: std::sync::OnceLock::new(),
        stdlib_source: std::sync::OnceLock::new(),
        watcher: std::sync::Mutex::new(None),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state.clone())
        // Windows hide instead of closing (the app window's webview holds the
        // chat transcript; destroying it would lose the conversation view).
        // Closing the last visible window quits.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                match window.label() {
                    "app" => {
                        api.prevent_close();
                        let _ = window.hide();
                        if let Some(dash) = app.get_webview_window("dashboard") {
                            let _ = dash.show();
                            let _ = dash.set_focus();
                        }
                    }
                    "dashboard" => {
                        let app_visible = app
                            .get_webview_window("app")
                            .and_then(|w| w.is_visible().ok())
                            .unwrap_or(false);
                        if app_visible {
                            api.prevent_close();
                            let _ = window.hide();
                        } else {
                            // Last visible window: a destroyed dashboard plus
                            // a hidden app window would leave a zombie process.
                            app.exit(0);
                        }
                    }
                    _ => {}
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();

            // Packaged apps carry the stdlib in Resources/stdlib (see
            // bundle.resources); exe-ancestor discovery can never find
            // lib/std from inside an .app. Under `tauri dev` the bundled
            // copy is absent and discovery finds the repo checkout.
            if let Ok(dir) = app.path().resource_dir() {
                let bundled = dir.join("stdlib");
                if bundled.join("pcb.toml").is_file() {
                    tracing::info!("using bundled stdlib at {}", bundled.display());
                    let _ = app_state.stdlib_source.set(bundled);
                }
            }

            builder::spawn_builder(handle.clone(), app_state.clone(), build_rx, rebuild_rx);

            // Dev nicety: ETCHABLE_OPEN=<board.zen or project dir> opens at
            // startup (relative to the invocation cwd).
            if let Ok(target) = std::env::var("ETCHABLE_OPEN") {
                let state = app_state.clone();
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    match std::path::PathBuf::from(&target).canonicalize() {
                        Ok(path) => {
                            let (entry, project) = if path.is_dir() {
                                match zen_build::load_project(&path) {
                                    Ok(doc) => match &doc.board {
                                        Some(b) => (doc.root.join(b), Some(doc)),
                                        None => {
                                            tracing::error!(
                                                "ETCHABLE_OPEN: no board entry: {:?}",
                                                doc.problems
                                            );
                                            return;
                                        }
                                    },
                                    Err(e) => {
                                        tracing::error!("ETCHABLE_OPEN: {e:#}");
                                        return;
                                    }
                                }
                            } else {
                                (path, None)
                            };
                            if let Err(e) =
                                commands::open_board_file(&handle, &state, entry, project).await
                            {
                                tracing::error!("ETCHABLE_OPEN build failed: {e}");
                            }
                        }
                        Err(e) => tracing::error!("ETCHABLE_OPEN: bad path {target}: {e}"),
                    }
                });
            }

            // Start the MCP server and write the generated mcp-config the
            // agent gets pointed at (`--mcp-config`) — zero user setup.
            let mcp_state = app_state.canvas.clone();
            let state_for_mcp = app_state.clone();
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            tauri::async_runtime::spawn(async move {
                match mcp::serve(mcp_state).await {
                    Ok((addr, _handle)) => {
                        tracing::info!("mcp server on http://{addr}/mcp");
                        let config = mcp::mcp_config_json(addr);
                        let path = config_dir.join("mcp-config.json");
                        if let Err(e) = std::fs::create_dir_all(&config_dir) {
                            tracing::error!("cannot create config dir: {e}");
                            return;
                        }
                        match std::fs::write(&path, config.to_string()) {
                            Ok(()) => {
                                let _ = state_for_mcp.mcp_config_path.set(path);
                            }
                            Err(e) => tracing::error!("cannot write mcp config: {e}"),
                        }
                    }
                    Err(e) => tracing::error!("mcp server failed to start: {e}"),
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::select_board,
            commands::show_dashboard,
            commands::open_project,
            commands::create_project,
            commands::get_state,
            commands::set_selection,
            commands::save_positions,
            commands::send_message,
            commands::respond_permission,
            commands::interrupt_agent,
            commands::new_session,
            commands::resume_session,
            commands::rebuild,
            commands::reload_workspace,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
