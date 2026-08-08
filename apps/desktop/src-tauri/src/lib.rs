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
        watcher: std::sync::Mutex::new(None),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state.clone())
        .setup(move |app| {
            let handle = app.handle().clone();

            builder::spawn_builder(handle.clone(), app_state.clone(), build_rx, rebuild_rx);

            // Dev nicety: ETCHABLE_OPEN=path/to/board.zen opens a board at
            // startup (relative to the invocation cwd).
            if let Ok(board) = std::env::var("ETCHABLE_OPEN") {
                let state = app_state.clone();
                tauri::async_runtime::spawn(async move {
                    match std::path::PathBuf::from(&board).canonicalize() {
                        Ok(path) => {
                            state.canvas.write(|s| s.source = Some(path));
                            if let Err(e) = state.request_build_and_wait().await {
                                tracing::error!("ETCHABLE_OPEN build failed: {e}");
                            }
                            if let Some(root) = state.canvas.read(|s| s.workspace_root.clone()) {
                                let _ = builder::start_watcher(&state, &root);
                            }
                        }
                        Err(e) => tracing::error!("ETCHABLE_OPEN: bad path {board}: {e}"),
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
            commands::get_state,
            commands::set_selection,
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
