//! Deskmate desktop shell: plugins, core engine, system tray, and frontend commands.

mod bridge;
mod clipfiles;
mod commands;
mod copykey;
mod history;
mod locale;
mod settings;
mod state;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};

/// Initializes logging and the engine, then starts the Tauri runtime.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    let app = tauri::Builder::default()
        // Register the single-instance lock first so the original process can
        // show its window when another instance starts.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // macOS autostart uses a LaunchAgent and begins hidden in the tray.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .setup(|app| {
            // On macOS: ~/Library/Application Support/<identifier>.
            let data_dir = app.path().app_data_dir()?;
            // Wait for engine startup so state exists before commands become available.
            let state = tauri::async_runtime::block_on(bridge::start_engine(
                app.handle().clone(),
                data_dir,
            ))?;
            app.manage(state);
            setup_tray(app.handle())?;
            // Register startup hotkeys without blocking launch on failure.
            {
                let app_state = app.state::<state::AppState>();
                let (send_hotkey, copy_hotkey) = {
                    let s = state::lock(&app_state.settings);
                    (s.send_clipboard_hotkey.clone(), s.copy_send_hotkey.clone())
                };
                if let Err(e) = commands::prefs::apply_clipboard_hotkey(
                    app.handle(),
                    None,
                    send_hotkey.as_deref(),
                ) {
                    tracing::warn!("failed to register send-clipboard hotkey at startup: {e}");
                }
                if let Err(e) = commands::prefs::apply_copy_send_hotkey(
                    app.handle(),
                    None,
                    copy_hotkey.as_deref(),
                ) {
                    tracing::warn!("failed to register copy-and-send hotkey at startup: {e}");
                }
            }
            // Convert Ctrl-C and SIGTERM into graceful shutdown so RunEvent::Exit
            // can send goodbye and unregister mDNS before the process exits.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                wait_for_termination().await;
                handle.exit(0);
            });
            // Autostart instances remain hidden in the tray.
            if std::env::args().any(|a| a == "--hidden")
                && let Some(window) = app.get_webview_window("main")
            {
                let _ = window.hide();
            }
            Ok(())
        })
        // Closing the window hides it in the tray; quitting uses the tray menu.
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            // Treat focus as read and clear Dock or taskbar unread indicators.
            tauri::WindowEvent::Focused(true) => {
                bridge::clear_unread(window.app_handle());
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_self_info,
            commands::list_peers,
            commands::send::send_files_to,
            commands::send::stage_clipboard_image,
            commands::send::send_clipboard_image,
            commands::send::send_text_to,
            commands::send::resume_send_transfer,
            commands::send::retry_send_transfer,
            commands::receive::respond_offer,
            commands::receive::precheck_receive,
            commands::receive::read_inline_image,
            commands::receive::pause_transfer,
            commands::receive::resume_transfer,
            commands::receive::cancel_transfer,
            commands::prefs::get_settings,
            commands::prefs::save_settings,
            commands::prefs::set_avatar_image,
            commands::prefs::get_avatar_image,
            commands::get_history,
            commands::append_history,
            commands::delete_history,
            commands::clear_history,
            commands::notify,
            commands::read_clipboard_files,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Tauri application");

    app.run(|app_handle, event| {
        match event {
            // Restore the hidden main window when the macOS Dock icon is clicked.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_main_window(app_handle),
            // Send goodbye and unregister mDNS before exit.
            tauri::RunEvent::Exit => {
                let state = app_handle.state::<state::AppState>();
                tauri::async_runtime::block_on(state.discovery.shutdown());
            }
            _ => {}
        }
    });
}

/// Waits for Ctrl-C, and for SIGTERM on Unix.
async fn wait_for_termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        // Fall back to Ctrl-C if SIGTERM listener registration fails.
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Shows and focuses the main window, then clears unread indicators.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    bridge::clear_unread(app);
}

/// Builds the tray menu using current-language text.
fn build_tray_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let lang = locale::lang(app);
    let show = MenuItem::with_id(
        app,
        "show",
        locale::Text::TrayShow.localize(lang),
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(
        app,
        "settings",
        locale::Text::TraySettings.localize(lang),
        true,
        None::<&str>,
    )?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        locale::Text::TrayQuit.localize(lang),
        true,
        None::<&str>,
    )?;
    Menu::with_items(app, &[&show, &settings, &sep, &quit])
}

/// Rebuilds the tray menu after a language change because item text is immutable.
pub(crate) fn refresh_tray_menu(app: &tauri::AppHandle) {
    let result = build_tray_menu(app).and_then(|menu| match app.tray_by_id("main-tray") {
        Some(tray) => tray.set_menu(Some(menu)),
        None => Ok(()),
    });
    if let Err(e) = result {
        tracing::warn!("failed to refresh tray menu language: {e}");
    }
}

/// Creates the system tray with show, settings, and quit actions.
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let menu = build_tray_menu(app)?;

    let mut tray = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        // Left-click shows the window; right-click opens the menu on all platforms.
        .show_menu_on_left_click(false)
        .tooltip("Deskmate")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            // Show the window and ask the frontend to open settings.
            "settings" => {
                show_main_window(app);
                if let Err(e) = app.emit(bridge::events::OPEN_SETTINGS, ()) {
                    tracing::warn!("failed to emit open-settings event: {e}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    // macOS uses a monochrome template icon that follows system appearance.
    #[cfg(target_os = "macos")]
    {
        match tauri::image::Image::from_bytes(include_bytes!("../icons/tray-iconTemplate.png")) {
            Ok(template) => tray = tray.icon(template).icon_as_template(true),
            Err(e) => {
                tracing::warn!("failed to decode tray template icon ({e}); using app icon");
                if let Some(icon) = app.default_window_icon() {
                    tray = tray.icon(icon.clone());
                }
            }
        }
    }
    // Windows and Linux use the full-color application icon.
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Initializes stderr tracing controlled by RUST_LOG, defaulting to info.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
