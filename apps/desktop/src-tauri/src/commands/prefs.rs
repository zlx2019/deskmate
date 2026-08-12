//! Settings commands: persistence, live application, avatars, and global hotkeys.

use tauri::State;

use super::ErrDto;
use crate::settings::Settings;
use crate::state::{AppState, lock};

/// Returns the current settings.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    lock(&state.settings).clone()
}

/// Saves settings. Everything except the startup-bound listening port updates immediately.
#[tauri::command]
pub fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), ErrDto> {
    std::fs::create_dir_all(&settings.download_dir)
        .map_err(|e| ErrDto::with("download_dir_unavailable", e))?;
    // Validate ignore-rule syntax before persisting invalid glob patterns.
    if !settings.ignore_rules.trim().is_empty() {
        deskmate_core::transfer::IgnoreRules::parse(&settings.ignore_rules)
            .map_err(|e| ErrDto::with("ignore_rules_invalid", e))?;
    }
    // Identical hotkeys would conflict during registration.
    if settings.send_clipboard_hotkey.is_some()
        && settings.send_clipboard_hotkey == settings.copy_send_hotkey
    {
        return Err(ErrDto::new("hotkey_conflict"));
    }
    // Apply hotkeys before persistence so registration failure aborts the save.
    {
        let old = lock(&state.settings).send_clipboard_hotkey.clone();
        if old != settings.send_clipboard_hotkey {
            apply_clipboard_hotkey(
                &app,
                old.as_deref(),
                settings.send_clipboard_hotkey.as_deref(),
            )?;
        }
    }
    {
        let old = lock(&state.settings).copy_send_hotkey.clone();
        if old != settings.copy_send_hotkey {
            apply_copy_send_hotkey(&app, old.as_deref(), settings.copy_send_hotkey.as_deref())?;
        }
    }
    settings
        .save(&state.data_dir)
        .map_err(|e| ErrDto::with("settings_save_failed", e))?;
    state
        .receiver
        .set_download_dir(settings.download_dir.clone());
    // Rebuild and publish the identity snapshot when the name or avatar changes.
    // Active transfers retain the previous snapshot; certificates remain unchanged.
    {
        let old = crate::state::current_identity(&state);
        let avatar_image = crate::bridge::load_avatar_image(&settings, &state.data_dir);
        let identity =
            crate::bridge::build_identity(&state.data_dir, &settings, avatar_image.as_deref())
                .map_err(|e| ErrDto::with("identity_update_failed", e))?;
        // Re-advertise only when peer-visible fields changed.
        if identity.display_name != old.display_name || identity.avatar != old.avatar {
            state
                .receiver
                .set_self_info(identity.peer_info(), avatar_image);
            state.discovery.update_info(&identity.peer_info());
            *lock(&state.identity) = identity;
        }
    }
    // Synchronize the idempotent autostart state immediately.
    {
        use tauri_plugin_autostart::ManagerExt;
        let launcher = app.autolaunch();
        let result = if settings.autostart {
            launcher.enable()
        } else {
            launcher.disable()
        };
        if let Err(e) = result {
            tracing::warn!("failed to synchronize autostart state: {e}");
        }
    }
    // Apply the pairing PIN immediately, treating an empty string as disabled.
    state
        .receiver
        .set_pin(settings.pin.clone().filter(|p| !p.is_empty()));
    // Toggle passive mode live; unchanged values are a no-op.
    state.discovery.set_passive(settings.passive);
    // Language changes affect notifications immediately; rebuild the tray menu.
    let language_changed = lock(&state.settings).language != settings.language;
    *lock(&state.settings) = settings;
    if language_changed {
        crate::refresh_tray_menu(&app);
    }
    Ok(())
}

/// Stores a frontend-compressed custom avatar of approximately 128 by 128 pixels.
///
/// This command only writes the image. The frontend persists
/// `settings.avatar = "custom"` through save_settings.
#[tauri::command]
pub fn set_avatar_image(state: State<'_, AppState>, data: Vec<u8>) -> Result<(), ErrDto> {
    if data.is_empty() {
        return Err(ErrDto::new("avatar_empty"));
    }
    if data.len() as u64 > deskmate_core::protocol::MAX_AVATAR_SIZE {
        return Err(ErrDto::new("avatar_too_large"));
    }
    std::fs::write(state.data_dir.join(crate::settings::AVATAR_FILE), &data)
        .map_err(|e| ErrDto::with("io", e))
}

/// Reads local custom-avatar bytes or a cached peer avatar by hash.
///
/// A cache miss returns None. The frontend retries after avatar-ready fires.
#[tauri::command]
pub fn get_avatar_image(state: State<'_, AppState>, hash: Option<String>) -> Option<Vec<u8>> {
    let path = match hash {
        None => state.data_dir.join(crate::settings::AVATAR_FILE),
        Some(h) => {
            // Peer-advertised hashes are untrusted; restrict them to hexadecimal.
            if !crate::bridge::is_safe_hash(&h) {
                return None;
            }
            state
                .data_dir
                .join(crate::bridge::AVATAR_CACHE_DIR)
                .join(format!("{h}.jpg"))
        }
    };
    std::fs::read(path).ok()
}

/// Applies the global send-clipboard hotkey.
pub(crate) fn apply_clipboard_hotkey(
    app: &tauri::AppHandle,
    old: Option<&str>,
    new: Option<&str>,
) -> Result<(), ErrDto> {
    apply_hotkey(app, old, new, emit_send_clipboard)
}

/// Applies the global copy-and-send hotkey, confirming the clipboard update
/// before entering the send-clipboard flow.
pub(crate) fn apply_copy_send_hotkey(
    app: &tauri::AppHandle,
    old: Option<&str>,
    new: Option<&str>,
) -> Result<(), ErrDto> {
    apply_hotkey(app, old, new, |app| {
        // Clipboard confirmation can take hundreds of milliseconds, so keep it
        // off the main-thread hotkey callback.
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            use crate::copykey::CopyOutcome;
            use crate::locale::Text;
            let notify = |text: Text<'_>| {
                crate::bridge::notify_if_unfocused(
                    &app,
                    "Deskmate",
                    &crate::locale::text(&app, text),
                );
            };
            match crate::copykey::copy_selection().await {
                CopyOutcome::Copied => emit_send_clipboard(&app),
                // Do not send stale clipboard data when no copy occurred.
                CopyOutcome::NothingCopied => notify(Text::CopySendNothing),
                CopyOutcome::PermissionNeeded => notify(Text::CopySendPermission),
                CopyOutcome::Unsupported => notify(Text::CopySendUnsupported),
            }
        });
    })
}

/// Emits the shared read-and-send-clipboard event for both hotkeys.
/// Peer selection, PIN caching, clipboard reads, and sending live in the frontend.
fn emit_send_clipboard(app: &tauri::AppHandle) {
    use tauri::Emitter;
    if let Err(e) = app.emit(crate::bridge::events::HOTKEY_SEND_CLIPBOARD, ()) {
        tracing::debug!("failed to emit hotkey event: {e}");
    }
}

/// Replaces an old hotkey registration and invokes `on_press` when pressed.
///
/// None or empty only unregisters. Registration errors are returned to settings.
fn apply_hotkey(
    app: &tauri::AppHandle,
    old: Option<&str>,
    new: Option<&str>,
    on_press: impl Fn(&tauri::AppHandle) + Send + Sync + 'static,
) -> Result<(), ErrDto> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let gs = app.global_shortcut();
    // An unparsable old value was never registered successfully.
    if let Some(old) = old.filter(|s| !s.is_empty())
        && let Ok(sc) = old.parse::<Shortcut>()
    {
        let _ = gs.unregister(sc);
    }
    let Some(new) = new.filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let sc: Shortcut = new.parse().map_err(|e| ErrDto::with("hotkey_invalid", e))?;
    gs.on_shortcut(sc, move |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            on_press(app);
        }
    })
    .map_err(|e| ErrDto::with("hotkey_conflict", e))
}
