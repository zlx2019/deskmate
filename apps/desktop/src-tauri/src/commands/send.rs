//! Send commands for files, text, clipboard screenshots, and task orchestration.

use std::path::PathBuf;
use std::sync::Arc;

use deskmate_core::discovery::Peer;
use deskmate_core::transfer::{
    ControlState, IgnoreRules, TransferError, resume_send, send_files, send_text,
};
use serde::Serialize;
use tauri::{Manager, State};
use tokio::sync::watch;

use super::ErrDto;
use crate::bridge::{TransferEventDto, emit_transfer_event};
use crate::state::{AppState, InterruptedMap, InterruptedSend, lock};

/// Sends files or directories to a peer and immediately returns the task ID.
///
/// Progress is emitted through transfer-event. The frontend attaches a cached
/// session PIN when available and prompts before retrying a PIN rejection.
#[tauri::command]
pub async fn send_files_to(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fingerprint: String,
    paths: Vec<String>,
    pin: Option<String>,
) -> Result<String, ErrDto> {
    if paths.is_empty() {
        return Err(ErrDto::new("no_files_selected"));
    }
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let path_bufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let ignore_rules = current_ignore_rules(&state);
    spawn_transfer_task(
        &app,
        &state,
        transfer_id.clone(),
        fingerprint,
        path_bufs,
        ignore_rules,
        SendMode::Fresh {
            pin,
            inline_image: false,
        },
    )?;
    Ok(transfer_id)
}

/// Stages clipboard screenshot bytes through the raw binary IPC channel.
///
/// Avoids the serialization expansion and parse cost of sending large images
/// as JSON arrays. Returns a staging ID for send_clipboard_image.
#[tauri::command]
pub fn stage_clipboard_image(request: tauri::ipc::Request<'_>) -> Result<String, ErrDto> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err(ErrDto::with("internal", "raw body expected"));
    };
    if data.is_empty() {
        return Err(ErrDto::new("screenshot_empty"));
    }
    let staged_id = uuid::Uuid::new_v4().to_string();
    let dir = std::env::temp_dir().join("deskmate-staging");
    std::fs::create_dir_all(&dir).map_err(|e| ErrDto::with("io", e))?;
    std::fs::write(dir.join(&staged_id), data).map_err(|e| ErrDto::with("io", e))?;
    Ok(staged_id)
}

/// Moves a staged screenshot into a task directory and sends it as a file.
///
/// The peer receives a normal PNG, reusing confirmation, allowlists, progress,
/// history, and PIN retries without protocol changes. Task-specific directories
/// prevent same-second screenshot names from overwriting each other. Temporary
/// files remain available for retries and are left to system cleanup policy.
#[tauri::command]
pub async fn send_clipboard_image(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fingerprint: String,
    file_name: String,
    staged: String,
    pin: Option<String>,
) -> Result<String, ErrDto> {
    // The frontend generates the screenshot name; allowlist it against traversal.
    let legal_name = !file_name.is_empty()
        && !file_name.contains("..")
        && file_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !legal_name {
        return Err(ErrDto::with("internal", "bad screenshot file name"));
    }
    // The staging ID should be the UUID returned by stage_clipboard_image.
    if staged.is_empty() || !staged.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(ErrDto::with("internal", "bad staged id"));
    }
    let staged_path = std::env::temp_dir().join("deskmate-staging").join(&staged);
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let task_dir = std::env::temp_dir()
        .join("deskmate-screenshots")
        .join(&transfer_id);
    std::fs::create_dir_all(&task_dir).map_err(|e| ErrDto::with("io", e))?;
    let path = task_dir.join(&file_name);
    // Same-volume rename is atomic and removes the staged source without copying.
    std::fs::rename(&staged_path, &path)
        .map_err(|e| ErrDto::with("screenshot_stage_missing", e))?;

    let ignore_rules = current_ignore_rules(&state);
    spawn_transfer_task(
        &app,
        &state,
        transfer_id.clone(),
        fingerprint,
        vec![path],
        ignore_rules,
        SendMode::Fresh {
            pin,
            inline_image: true,
        },
    )?;
    Ok(transfer_id)
}

/// Retries a PIN-rejected send with the original transfer ID and progress entry.
#[tauri::command]
pub async fn retry_send_transfer(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    transfer_id: String,
    pin: Option<String>,
) -> Result<(), ErrDto> {
    let (fingerprint, paths, ignore_rules, inline_image) =
        interrupted_params(&state, &transfer_id, "retry_unavailable")?;
    spawn_transfer_task(
        &app,
        &state,
        transfer_id,
        fingerprint,
        paths,
        ignore_rules,
        SendMode::Fresh { pin, inline_image },
    )
}

/// Resumes an interrupted send using its original transfer ID and progress entry.
#[tauri::command]
pub async fn resume_send_transfer(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<(), ErrDto> {
    let (fingerprint, paths, ignore_rules, _) =
        interrupted_params(&state, &transfer_id, "resume_unavailable")?;
    spawn_transfer_task(
        &app,
        &state,
        transfer_id,
        fingerprint,
        paths,
        ignore_rules,
        SendMode::Resume,
    )
}

/// Text-send result indicating whether the peer requires a pairing PIN.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTextOutcome {
    /// Whether the send was rejected for a missing or incorrect PIN.
    pub pin_required: bool,
}

/// Sends byte-identical text to a peer.
///
/// PIN rejection is returned as structured data; other failures remain errors.
#[tauri::command]
pub async fn send_text_to(
    state: State<'_, AppState>,
    fingerprint: String,
    text: String,
    pin: Option<String>,
) -> Result<SendTextOutcome, ErrDto> {
    let peer = find_peer(&state, &fingerprint)?;
    let identity = crate::state::current_identity(&state);
    match send_text(
        &identity,
        &peer.addrs,
        peer.port,
        Some(peer.info.fingerprint.clone()),
        pin,
        &text,
    )
    .await
    {
        Ok(_) => Ok(SendTextOutcome {
            pin_required: false,
        }),
        Err(TransferError::PinRequired) => Ok(SendTextOutcome { pin_required: true }),
        Err(other) => Err(ErrDto::from(&other)),
    }
}

/// Send task mode: a fresh transfer or a resumed transfer.
enum SendMode {
    /// Fresh send or PIN retry.
    Fresh {
        /// Pairing PIN required by the peer.
        pin: Option<String>,
        /// Marks the manifest as an inline clipboard image (protocol 1.5).
        /// Resume reads the marker back from the receiver's resume metadata.
        inline_image: bool,
    },
    /// Resume after negotiating offsets and send only missing ranges.
    Resume,
}

/// Retrieves original task parameters from the interrupted-send registry.
fn interrupted_params(
    state: &State<'_, AppState>,
    transfer_id: &str,
    missing_code: &'static str,
) -> Result<(String, Vec<PathBuf>, String, bool), ErrDto> {
    let guard = lock(&state.interrupted_sends);
    let item = guard
        .get(transfer_id)
        .ok_or_else(|| ErrDto::new(missing_code))?;
    Ok((
        item.fingerprint.clone(),
        item.paths.clone(),
        item.ignore_rules.clone(),
        item.inline_image,
    ))
}

/// Snapshots current ignore rules for retries and resumption.
fn current_ignore_rules(state: &State<'_, AppState>) -> String {
    lock(&state.settings).ignore_rules.clone()
}

/// Registers controls, runs the engine task in the background, and settles it.
fn spawn_transfer_task(
    app: &tauri::AppHandle,
    state: &State<'_, AppState>,
    transfer_id: String,
    fingerprint: String,
    paths: Vec<PathBuf>,
    ignore_rules: String,
    mode: SendMode,
) -> Result<(), ErrDto> {
    let peer = find_peer(state, &fingerprint)?;
    // Settings validation should make parse failure exceptional; report it.
    let ignore = match ignore_rules.trim() {
        "" => None,
        _ => Some(
            IgnoreRules::parse(&ignore_rules)
                .map_err(|e| ErrDto::with("ignore_rules_invalid", e))?,
        ),
    };

    // Pre-register controls so the frontend can pause or cancel immediately.
    let (control_tx, control_rx) = watch::channel(ControlState::Running);
    lock(&state.send_controls).insert(transfer_id.clone(), control_tx);
    // Retries and resumes reuse the transfer ID; allow a fresh failure to notify.
    lock(&state.failure_notified).remove(&transfer_id);

    let app = app.clone();
    let identity = crate::state::current_identity(state);
    let events_tx = state.events_tx.clone();
    let controls = Arc::clone(&state.send_controls);
    let interrupted = Arc::clone(&state.interrupted_sends);
    let tid = transfer_id;
    tauri::async_runtime::spawn(async move {
        let is_resume = matches!(mode, SendMode::Resume);
        // Preserved for the interrupted-send registry so PIN retries keep it.
        let inline_image = matches!(
            mode,
            SendMode::Fresh {
                inline_image: true,
                ..
            }
        );
        let result = match mode {
            SendMode::Fresh { pin, inline_image } => send_files(
                &identity,
                &peer.addrs,
                peer.port,
                Some(peer.info.fingerprint.clone()),
                Some(tid.clone()),
                pin,
                &paths,
                inline_image,
                ignore.as_ref(),
                control_rx,
                events_tx,
            )
            .await
            .map(|_| ()),
            SendMode::Resume => resume_send(
                &identity,
                &peer.addrs,
                peer.port,
                Some(peer.info.fingerprint.clone()),
                &tid,
                &paths,
                ignore.as_ref(),
                control_rx,
                events_tx,
            )
            .await
            .map(|_| ()),
        };
        settle_send_result(
            &app,
            &interrupted,
            &tid,
            &peer,
            paths,
            ignore_rules,
            inline_image,
            is_resume,
            &result,
        );
        lock(&controls).remove(&tid);
    });
    Ok(())
}

/// Emits events missing from the engine and maintains interrupted-send state.
///
/// Rejections, PIN gates, and resume negotiation failures do not emit engine
/// events, so this layer supplies them. Success and cancellation clear the
/// registry; fresh-send failures register parameters; resume failures retain
/// the existing registration.
#[expect(
    clippy::too_many_arguments,
    reason = "settlement arguments are the complete task context"
)]
fn settle_send_result(
    app: &tauri::AppHandle,
    interrupted: &InterruptedMap,
    transfer_id: &str,
    peer: &Peer,
    paths: Vec<PathBuf>,
    ignore_rules: String,
    inline_image: bool,
    is_resume: bool,
    result: &Result<(), TransferError>,
) {
    match result {
        Ok(()) | Err(TransferError::Cancelled) => {
            lock(interrupted).remove(transfer_id);
        }
        Err(e) => {
            match e {
                TransferError::Rejected {
                    reason,
                    reason_code,
                } => {
                    crate::bridge::notify_if_unfocused(
                        app,
                        "Deskmate",
                        &crate::locale::text(app, crate::locale::Text::TransferRejected),
                    );
                    emit_transfer_event(
                        app,
                        TransferEventDto::Rejected {
                            transfer_id: transfer_id.to_string(),
                            reason: reason.clone(),
                            pin_required: false,
                            reason_code: reason_code.clone(),
                        },
                    );
                }
                TransferError::PinRequired => {
                    emit_transfer_event(
                        app,
                        TransferEventDto::Rejected {
                            transfer_id: transfer_id.to_string(),
                            reason: None,
                            pin_required: true,
                            reason_code: Some("pin_required".to_string()),
                        },
                    );
                }
                // An empty manifest never starts a transfer or emits an engine
                // event. Emit Ignored to terminate the frontend entry without
                // registering a task that cannot be retried or resumed.
                TransferError::NoValidFiles => {
                    crate::bridge::notify_if_unfocused(
                        app,
                        "Deskmate",
                        &crate::locale::text(app, crate::locale::Text::NothingToSend),
                    );
                    emit_transfer_event(
                        app,
                        TransferEventDto::Ignored {
                            transfer_id: transfer_id.to_string(),
                        },
                    );
                }
                // Collection and handshake failures emit no engine event, so
                // Interrupted must terminate the frontend entry. Data-phase
                // failures may duplicate an event, but frontend updates are
                // idempotent and history replaces entries by transfer ID.
                _ => {
                    // Data-phase failures also surface as an engine Interrupted
                    // event; the shared set makes whichever observer comes
                    // first send the single failure notification.
                    if lock(&app.state::<AppState>().failure_notified)
                        .insert(transfer_id.to_string())
                    {
                        crate::bridge::notify_if_unfocused(
                            app,
                            "Deskmate",
                            &crate::locale::text(app, crate::locale::Text::TransferFailed),
                        );
                    }
                    emit_transfer_event(
                        app,
                        TransferEventDto::Interrupted {
                            transfer_id: transfer_id.to_string(),
                            reason: e.to_string(),
                            code: e.code().to_string(),
                            detail: e.detail(),
                        },
                    );
                }
            }
            // Empty-manifest tasks cannot be retried or resumed.
            if !is_resume && !matches!(e, TransferError::NoValidFiles) {
                lock(interrupted).insert(
                    transfer_id.to_string(),
                    InterruptedSend {
                        fingerprint: peer.info.fingerprint.clone(),
                        paths,
                        ignore_rules,
                        inline_image,
                    },
                );
            }
            tracing::warn!(transfer_id, is_resume, "send finished with an error: {e}");
        }
    }
}

/// Finds an online peer by fingerprint.
fn find_peer(state: &State<'_, AppState>, fingerprint: &str) -> Result<Peer, ErrDto> {
    state
        .discovery
        .peer_by_fingerprint(fingerprint)
        .ok_or_else(|| ErrDto::new("peer_offline"))
}
