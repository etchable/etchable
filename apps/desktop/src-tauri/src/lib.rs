mod menu;
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
        .on_menu_event(|app, event| menu::handle(app, event))
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
                tauri::WindowEvent::CloseRequested { .. } if window.label() != "dashboard" => {
                    // The user is done with this project: drop it from the set
                    // that gets reopened next launch.
                    commands::forget_open_board(&registry, window.label());
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

            // Install a menu immediately, with no recents and no db access. The
            // list is filled in by the deferred refresh at the end of setup:
            // reading it here would put a db-touching task alongside setup's own
            // blocking read, and the two contend badly enough to hang startup.
            menu::install(&handle, &[]);

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

            // Reopen what was open. A clean quit recorded its boards; a crash
            // never got to, so this comes back empty and the dashboard stands.
            // The flag is cleared immediately: from here until the next
            // ExitRequested, dying at any point means "do not restore".
            let restore = registry
                .store()
                .map(|store| {
                    let store = store.clone();
                    tauri::async_runtime::block_on(async move {
                        let boards = store.boards_to_restore().await.unwrap_or_default();
                        let _ = store.set_clean_exit(false).await;
                        boards
                    })
                })
                .unwrap_or_default();
            if !restore.is_empty() {
                // Hide it now, not once the first project window appears: the
                // dashboard is created visible by the window config, and the
                // restore below is async, so otherwise it flashes first.
                if let Some(dash) = app.get_webview_window("dashboard") {
                    let _ = dash.hide();
                }
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let mut opened = 0usize;
                    for board in restore {
                        let path = std::path::PathBuf::from(&board);
                        // A board that moved or was deleted is simply dropped —
                        // never a startup error, and never a reason to show
                        // nothing at all.
                        if !path.is_file() {
                            tracing::info!("not restoring {board}: no longer there");
                            continue;
                        }
                        let project = zen_build::load_project(
                            path.parent().unwrap_or(&path),
                        )
                        .ok();
                        let registry = handle.state::<Registry>();
                        match commands::open_board_file(&handle, &registry, path, project).await {
                            Ok(_) => opened += 1,
                            Err(e) => tracing::warn!("restoring {board} failed: {e}"),
                        }
                    }
                    // Everything we meant to reopen is gone or broken: leave the
                    // user somewhere useful rather than with a hidden dashboard.
                    if opened == 0 {
                        if let Some(dash) = handle.get_webview_window("dashboard") {
                            let _ = dash.show();
                            let _ = dash.set_focus();
                        }
                    }
                });
            }

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

            // Now that setup's blocking reads are done, fill in Open Recent.
            menu::refresh_soon(&app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::select_board,
            commands::sketch_board,
            commands::take_initial_prompt,
            commands::show_dashboard,
            commands::open_project,
            commands::create_project,
            commands::get_state,
            commands::set_selection,
            commands::save_positions,
            commands::add_instance,
            commands::rename_instance,
            commands::rename_net,
            commands::attach_pin_net,
            commands::connect_pins,
            commands::disconnect_pin,
            commands::undo_gesture,
            commands::redo_gesture,
            commands::set_attribute,
            commands::remove_instances,
            commands::get_palette,
            commands::warm_placement,
            commands::search_lcsc,
            commands::lcsc_part_detail,
            commands::lcsc_install,
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
            let registry = app.state::<Registry>();
            registry
                .exiting
                .store(true, std::sync::atomic::Ordering::Relaxed);
            // The restore set is already whatever the user left open — it is
            // maintained on open and on CloseRequested, never derived here,
            // because windows may already be gone by the time this fires.
            // Stamping the flag is all that's needed, and it is what separates a
            // quit from a crash: a process that dies never runs this line.
            if let Some(store) = registry.store().cloned() {
                tauri::async_runtime::block_on(async move {
                    if let Err(e) = store.set_clean_exit(true).await {
                        tracing::warn!("recording clean exit failed: {e:#}");
                    }
                });
            }
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
