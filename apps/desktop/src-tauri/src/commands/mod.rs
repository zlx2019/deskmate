//! Tauri commands exposed to the frontend.
//!
//! Commands are grouped by domain: [`send`] handles files, text, clipboard
//! screenshots, and task orchestration; [`receive`] handles offer responses and
//! transfer controls; [`prefs`] handles settings and avatars. This module keeps
//! local-device information, history, and miscellaneous commands.

pub mod prefs;
pub mod receive;
pub mod send;

use deskmate_core::identity::platform;
use deskmate_core::transfer::TransferError;
use serde::Serialize;
use tauri::State;

use crate::bridge::PeerDto;
use crate::state::AppState;

/// Structured command error localized by frontend `code`.
///
/// `detail` carries untranslated raw context such as I/O messages or paths.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrDto {
    /// Stable error code matching the frontend i18n errors table.
    pub code: &'static str,
    /// Detail parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl std::fmt::Display for ErrDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "{} ({d})", self.code),
            None => f.write_str(self.code),
        }
    }
}

impl ErrDto {
    /// Creates an error without details.
    pub fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    /// Creates an error with details.
    pub fn with(code: &'static str, detail: impl ToString) -> Self {
        Self {
            code,
            detail: Some(detail.to_string()),
        }
    }
}

impl From<&TransferError> for ErrDto {
    fn from(e: &TransferError) -> Self {
        Self {
            code: e.code(),
            detail: e.detail(),
        }
    }
}

/// Local-device information displayed by the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfInfoDto {
    /// Display name.
    pub name: String,
    /// Device ID.
    pub device_id: String,
    /// Certificate fingerprint.
    pub fingerprint: String,
    /// Platform.
    pub platform: String,
    /// Actual listening port.
    pub port: u16,
    /// Current download directory.
    pub download_dir: String,
    /// Built-in emoji avatar.
    pub avatar: Option<String>,
}

/// Returns local-device information.
#[tauri::command]
pub fn get_self_info(state: State<'_, AppState>) -> SelfInfoDto {
    let identity = crate::state::current_identity(&state);
    SelfInfoDto {
        name: identity.display_name.clone(),
        device_id: identity.device_id.clone(),
        fingerprint: identity.fingerprint.clone(),
        platform: platform(),
        port: state.tcp_port,
        download_dir: state.receiver.download_dir().display().to_string(),
        avatar: identity.avatar.clone(),
    }
}

/// Returns the current peer snapshot; peer events maintain it after startup.
#[tauri::command]
pub fn list_peers(state: State<'_, AppState>) -> Vec<PeerDto> {
    state.discovery.peers().iter().map(PeerDto::from).collect()
}

/// Returns an in-memory transfer-history snapshot, newest first.
#[tauri::command]
pub fn get_history(state: State<'_, AppState>) -> Vec<crate::history::HistoryEntry> {
    state.history.snapshot()
}

/// Appends a terminal transfer reported by the frontend and persists it asynchronously.
#[tauri::command]
pub fn append_history(state: State<'_, AppState>, entry: crate::history::HistoryEntry) {
    state.history.append(entry);
}

/// Deletes a transfer-history entry by transfer ID.
#[tauri::command]
pub fn delete_history(state: State<'_, AppState>, transfer_id: String) {
    state.history.remove(&transfer_id);
}

/// Clears all transfer history.
#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) {
    state.history.clear();
}

/// Sends a system notification when the window is unfocused.
#[tauri::command]
pub fn notify(app: tauri::AppHandle, title: String, body: String) {
    crate::bridge::notify_if_unfocused(&app, &title, &body);
}

/// Reads copied file paths from the clipboard for hotkey-triggered sends.
///
/// Clipboard access uses system IPC and may retry the Windows clipboard lock,
/// so it runs on the blocking thread pool instead of the main thread.
#[tauri::command]
pub async fn read_clipboard_files() -> Result<Vec<String>, ErrDto> {
    tauri::async_runtime::spawn_blocking(crate::clipfiles::read_file_paths)
        .await
        .map_err(|e| ErrDto::with("io", e))
}
