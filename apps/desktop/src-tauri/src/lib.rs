mod agent;
mod builder;
mod commands;
mod state;

use tauri::Manager;

use state::Registry;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,etchable=debug".into()),
        )
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Registry::default())
        // Project windows are documents: closing one tears its instance down
        // (agent, builder, watcher, MCP server). The dashboard only hides —
        // unless it's the last visible window, which quits.
        .on_window_event(|window, event| {
            let app = window.app_handle();
            let registry = app.state::<Registry>();
            match event {
                tauri::WindowEvent::CloseRequested { api, .. }
                    if window.label() == "dashboard" =>
                {
                    let any_project_visible = app
                        .webview_windows()
                        .values()
                        .any(|w| w.label() != "dashboard" && w.is_visible().unwrap_or(false));
                    if any_project_visible {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        app.exit(0);
                    }
                }
                tauri::WindowEvent::Destroyed if window.label() != "dashboard" => {
                    commands::teardown_instance(&registry, window.label());
                    // Last project window gone: come back to the dashboard
                    // (unless the whole app is quitting).
                    if registry.is_empty()
                        && !registry.exiting.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        if let Some(dash) = app.get_webview_window("dashboard") {
                            let _ = dash.show();
                            let _ = dash.set_focus();
                        }
                    }
                }
                _ => {}
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let registry = app.state::<Registry>();

            // ~/.etchable/ housekeeping: adopt any pre-0005 cache, open the
            // state db. A failed open (e.g. a db from a newer build) logs
            // once and the app runs without persistence — never bricked.
            store::paths::migrate_legacy_lcsc_cache();
            let opened = tauri::async_runtime::block_on(store::Store::open_default());
            let _ = registry.store.set(match opened {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::error!("state db unavailable, running without persistence: {e:#}");
                    None
                }
            });

            // Packaged apps carry the stdlib in Resources/stdlib (see
            // bundle.resources); exe-ancestor discovery can never find
            // lib/std from inside an .app. Under `tauri dev` the bundled
            // copy is absent and discovery finds the repo checkout.
            if let Ok(dir) = app.path().resource_dir() {
                let bundled = dir.join("stdlib");
                if bundled.join("pcb.toml").is_file() {
                    tracing::info!("using bundled stdlib at {}", bundled.display());
                    let _ = registry.stdlib_source.set(bundled);
                }
            }

            // Per-instance mcp-config files land in ~/.etchable/runtime
            // (see create_instance), pid-suffixed so concurrent app
            // processes never clobber each other; stale files from dead
            // processes are swept here.
            let runtime_dir = store::paths::runtime_dir();
            sweep_stale_runtime_files(&runtime_dir);
            let _ = registry.config_dir.set(runtime_dir);

            // Dev nicety: ETCHABLE_OPEN=<board.zen or project dir> opens at
            // startup (relative to the invocation cwd).
            if let Ok(target) = std::env::var("ETCHABLE_OPEN") {
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
                            let registry = handle.state::<Registry>();
                            if let Err(e) =
                                commands::open_board_file(&handle, &registry, entry, project)
                                    .await
                            {
                                tracing::error!("ETCHABLE_OPEN build failed: {e}");
                            }
                        }
                        Err(e) => tracing::error!("ETCHABLE_OPEN: bad path {target}: {e}"),
                    }
                });
            }

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
            commands::list_recent_projects,
            commands::remove_recent_project,
            commands::list_sessions,
            commands::get_prefs,
            commands::set_pref,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            // Quit in progress: window teardown must not resurrect the
            // dashboard from its Destroyed handler.
            app.state::<Registry>()
                .exiting
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

/// Remove runtime scratch left by dead processes. Age-based on purpose:
/// pid-liveness checks are platform-fiddly, the files are ~100 bytes, and
/// week-old scratch from a still-running process is vanishingly rare.
/// (This process's own files are pid-prefixed, so age never bites them —
/// they're rewritten on every instance creation.)
fn sweep_stale_runtime_files(dir: &std::path::Path) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    let own_prefix = format!("mcp-config-{}-", std::process::id());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("mcp-config-") || name.starts_with(own_prefix.as_str()) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > MAX_AGE);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
