//! Receive commands: offer responses, preflight checks, and transfer controls.

use std::path::{Path, PathBuf};

use deskmate_core::transfer::{ConflictPolicy, ControlState, OfferDecision, sanitize_rel_path};
use serde::Serialize;
use tauri::State;

use super::ErrDto;
use crate::state::{AppState, lock};

/// Responds to an incoming offer.
///
/// `accept=true` accepts the entire offer, optionally with another save
/// directory. `overwrite` selects overwrite or automatic rename for conflicts,
/// based on the frontend setting or the user's modal selection.
#[tauri::command]
pub fn respond_offer(
    state: State<'_, AppState>,
    offer_id: String,
    accept: bool,
    save_dir: Option<String>,
    overwrite: bool,
) -> Result<(), ErrDto> {
    let pending = lock(&state.offers)
        .remove(&offer_id)
        .ok_or_else(|| ErrDto::new("offer_expired"))?;
    let decision = if accept {
        OfferDecision::Accept {
            accepted_files: pending.file_ids,
            save_dir: save_dir.map(PathBuf::from),
            conflict: if overwrite {
                ConflictPolicy::Overwrite
            } else {
                ConflictPolicy::Rename
            },
        }
    } else {
        OfferDecision::Reject {
            reason: Some("rejected by receiver".to_string()),
        }
    };
    pending
        .reply
        .send(decision)
        .map_err(|_| ErrDto::new("session_gone"))
}

/// Receive preflight result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrecheckDto {
    /// Available bytes on the target disk. None skips the frontend space check.
    pub free_bytes: Option<u64>,
    /// Relative paths that conflict with existing files in the target directory.
    pub conflicts: Vec<String>,
}

/// Checks target-disk capacity and file-name conflicts before receiving.
///
/// Uses the current default download directory when `dir` is None. The
/// frontend calls this when the modal opens and when the directory changes.
#[tauri::command]
pub fn precheck_receive(
    state: State<'_, AppState>,
    dir: Option<String>,
    rel_paths: Vec<String>,
) -> PrecheckDto {
    let base = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| state.receiver.download_dir());
    let conflicts = rel_paths
        .into_iter()
        .filter(|rel| {
            sanitize_rel_path(rel)
                .map(|safe| base.join(safe).exists())
                .unwrap_or(false)
        })
        .collect();
    PrecheckDto {
        free_bytes: available_space_of(&base),
        conflicts,
    }
}

/// Queries available space, walking up to the first existing ancestor if needed.
fn available_space_of(dir: &Path) -> Option<u64> {
    let mut probe = dir;
    loop {
        if probe.exists() {
            return fs4::available_space(probe).ok();
        }
        probe = probe.parent()?;
    }
}

/// Pauses a sender-side or receiver-side transfer.
#[tauri::command]
pub fn pause_transfer(state: State<'_, AppState>, transfer_id: String) -> bool {
    set_transfer_state(&state, &transfer_id, ControlState::Paused)
}

/// Resumes a transfer.
#[tauri::command]
pub fn resume_transfer(state: State<'_, AppState>, transfer_id: String) -> bool {
    set_transfer_state(&state, &transfer_id, ControlState::Running)
}

/// Cancels a transfer. Receiver-side .part files are deleted.
#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, transfer_id: String) -> bool {
    // Receiver-side tasks are managed by ReceiverHandle.
    if state.receiver.cancel(&transfer_id) {
        return true;
    }
    // Sender side: mark it cancelled and remove the registration.
    match lock(&state.send_controls).remove(&transfer_id) {
        Some(tx) => tx.send(ControlState::Cancelled).is_ok(),
        None => false,
    }
}

/// Attempts to update receiver-side state first, then sender-side state.
fn set_transfer_state(state: &State<'_, AppState>, transfer_id: &str, s: ControlState) -> bool {
    let receiver_side = match s {
        ControlState::Paused => state.receiver.pause(transfer_id),
        ControlState::Running => state.receiver.resume(transfer_id),
        ControlState::Cancelled => state.receiver.cancel(transfer_id),
    };
    if receiver_side {
        return true;
    }
    lock(&state.send_controls)
        .get(transfer_id)
        .map(|tx| tx.send(s).is_ok())
        .unwrap_or(false)
}

/// Upper bound for inline-image reads; larger files stay file-only in the UI.
const MAX_INLINE_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// Reads a received inline clipboard image for message-stream display.
///
/// Only paths registered by the event pump from engine FileCompleted events
/// are served, so the frontend cannot read arbitrary files. The size cap
/// bounds memory even when a peer marks a huge file as inline.
#[tauri::command]
pub fn read_inline_image(
    state: State<'_, AppState>,
    path: String,
) -> Result<tauri::ipc::Response, ErrDto> {
    let path = PathBuf::from(path);
    if !lock(&state.inline_image_paths).contains(&path) {
        return Err(ErrDto::new("inline_image_unavailable"));
    }
    let meta = std::fs::metadata(&path).map_err(|e| ErrDto::with("io", e))?;
    if meta.len() > MAX_INLINE_IMAGE_BYTES {
        return Err(ErrDto::new("inline_image_too_large"));
    }
    let bytes = std::fs::read(&path).map_err(|e| ErrDto::with("io", e))?;
    Ok(tauri::ipc::Response::new(bytes))
}
