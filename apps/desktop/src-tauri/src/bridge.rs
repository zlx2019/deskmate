//! Engine bridge that starts deskmate-core and forwards engine events to Tauri.
//!
//! Frontend event contract:
//! - `peer-up` / `peer-down`: peer online DTO or offline fingerprint
//! - `transfer-offer`: incoming offer awaiting user action
//! - `transfer-event`: transfer lifecycle event distinguished by kind

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use deskmate_core::DEFAULT_DISCOVERY_PORT;
use deskmate_core::discovery::{DiscoveryService, Peer, PeerEvent};
use deskmate_core::identity::DeviceIdentity;
use deskmate_core::protocol::FileMeta;
use deskmate_core::transfer::{
    ConflictPolicy, OfferDecision, ReceiverOptions, TransferEvent, TransferOffer, bind_dual_stack,
    fetch_avatar, spawn_receiver,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::settings::{AVATAR_CUSTOM, AVATAR_FILE, ConflictPolicySetting, Settings};
use crate::state::{AppState, ControlMap, InterruptedMap, OfferMap, PendingOffer, lock};

/// Avatar cache subdirectory; files are named `<hash>.jpg`.
pub const AVATAR_CACHE_DIR: &str = "avatars";

/// Tauri event names mirrored by frontend src/events.ts.
pub mod events {
    /// Peer came online; payload is PeerDto.
    pub const PEER_UP: &str = "peer-up";
    /// Peer went offline; payload is its fingerprint.
    pub const PEER_DOWN: &str = "peer-down";
    /// Incoming transfer awaiting a user decision; payload is OfferDto.
    pub const TRANSFER_OFFER: &str = "transfer-offer";
    /// Transfer lifecycle event; payload is TransferEventDto.
    pub const TRANSFER_EVENT: &str = "transfer-event";
    /// Trusted-device automatic receive started; payload is AutoStartDto.
    pub const TRANSFER_AUTOSTART: &str = "transfer-autostart";
    /// Peer avatar cache is ready; payload is AvatarReadyDto.
    pub const AVATAR_READY: &str = "avatar-ready";
    /// Global hotkey requesting a frontend clipboard send to the selected peer.
    /// Shared by send-clipboard and copy-and-send after copy confirmation.
    pub const HOTKEY_SEND_CLIPBOARD: &str = "hotkey-send-clipboard";
    /// Tray settings action requesting the frontend settings dialog.
    pub const OPEN_SETTINGS: &str = "open-settings";
}

/// Emits a transfer event; emission failure is logged without affecting the engine.
pub(crate) fn emit_transfer_event(app: &AppHandle, dto: TransferEventDto) {
    if let Err(e) = app.emit(events::TRANSFER_EVENT, dto) {
        tracing::debug!("failed to emit transfer-event: {e}");
    }
}

/// Minimum progress-event forwarding interval, limiting updates to about 10 Hz.
const PROGRESS_EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Avatar hashes currently being fetched, used to deduplicate concurrent requests.
type InflightAvatars = Arc<Mutex<HashSet<String>>>;

/// Peer information displayed by the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerDto {
    /// Device ID.
    pub device_id: String,
    /// Display name.
    pub name: String,
    /// Hex certificate fingerprint, also used as the frontend peer key.
    pub fingerprint: String,
    /// Platform: macos, windows, or linux.
    pub platform: String,
    /// Candidate addresses for display.
    pub addrs: Vec<String>,
    /// TCP port.
    pub port: u16,
    /// Built-in emoji avatar; None uses the frontend initial style.
    pub avatar: Option<String>,
    /// Operating-system version, or None for older peers.
    pub os_version: Option<String>,
}

impl From<&Peer> for PeerDto {
    fn from(p: &Peer) -> Self {
        Self {
            device_id: p.info.device_id.clone(),
            name: p.info.name.clone(),
            fingerprint: p.info.fingerprint.clone(),
            platform: p.info.platform.clone(),
            addrs: p.addrs.iter().map(|a| a.to_string()).collect(),
            port: p.port,
            avatar: p.info.avatar.clone(),
            os_version: p.info.os_version.clone(),
        }
    }
}

/// Manifest file entry displayed by the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetaDto {
    /// File ID.
    pub file_id: u32,
    /// Relative path.
    pub rel_path: String,
    /// Size in bytes.
    pub size: u64,
}

impl From<&FileMeta> for FileMetaDto {
    fn from(f: &FileMeta) -> Self {
        Self {
            file_id: f.file_id,
            rel_path: f.rel_path.clone(),
            size: f.size,
        }
    }
}

/// Transfer offer awaiting a frontend decision.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferDto {
    /// Decision response ID used by respond_offer.
    pub offer_id: String,
    /// Transfer task ID.
    pub transfer_id: String,
    /// Sender name.
    pub peer_name: String,
    /// Sender fingerprint.
    pub peer_fingerprint: String,
    /// Sender platform.
    pub peer_platform: String,
    /// Sender emoji avatar.
    pub peer_avatar: Option<String>,
    /// File manifest.
    pub files: Vec<FileMetaDto>,
    /// Total bytes.
    pub total_size: u64,
}

/// Transfer lifecycle event distinguished by the kind field.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TransferEventDto {
    /// Progress for one file.
    Progress {
        transfer_id: String,
        file_id: u32,
        rel_path: String,
        done: u64,
        size: u64,
    },
    /// One file completed. inline_image marks a received clipboard image the
    /// frontend also shows in the message stream.
    FileCompleted {
        transfer_id: String,
        file_id: u32,
        path: String,
        inline_image: bool,
    },
    /// Transfer completed.
    Completed { transfer_id: String },
    /// Transfer cancelled and temporary files removed.
    Cancelled { transfer_id: String },
    /// Unexpected interruption retaining .part data; code/detail support
    /// frontend localization and reason is the fallback message.
    Interrupted {
        transfer_id: String,
        reason: String,
        code: String,
        detail: Option<String>,
    },
    /// The peer paused the transfer; local actions update optimistically.
    Paused { transfer_id: String },
    /// The peer resumed the transfer.
    Resumed { transfer_id: String },
    /// The local send manifest was empty because all files were ignored or the
    /// source directory was empty. No peer connection was attempted.
    Ignored { transfer_id: String },
    /// Sender-side peer rejection. pin_required identifies a missing or invalid
    /// pairing PIN. reason_code is structured for 1.4+ peers and reason is the
    /// peer-language fallback.
    Rejected {
        transfer_id: String,
        reason: Option<String>,
        pin_required: bool,
        reason_code: Option<String>,
    },
    /// Text received.
    TextReceived {
        from_name: String,
        from_fingerprint: String,
        text: String,
    },
}

impl From<TransferEvent> for TransferEventDto {
    fn from(ev: TransferEvent) -> Self {
        match ev {
            TransferEvent::Progress {
                transfer_id,
                file_id,
                rel_path,
                done,
                size,
            } => Self::Progress {
                // The engine uses Arc<str>; serialized DTOs need owned strings.
                transfer_id: transfer_id.to_string(),
                file_id,
                rel_path: rel_path.to_string(),
                done,
                size,
            },
            TransferEvent::FileCompleted {
                transfer_id,
                file_id,
                path,
                inline_image,
            } => Self::FileCompleted {
                transfer_id,
                file_id,
                path: path.display().to_string(),
                inline_image,
            },
            TransferEvent::Completed { transfer_id } => Self::Completed { transfer_id },
            TransferEvent::Cancelled { transfer_id } => Self::Cancelled { transfer_id },
            TransferEvent::Interrupted {
                transfer_id,
                reason,
                code,
                detail,
            } => Self::Interrupted {
                transfer_id,
                reason,
                code: code.to_string(),
                detail,
            },
            TransferEvent::Paused { transfer_id } => Self::Paused { transfer_id },
            TransferEvent::Resumed { transfer_id } => Self::Resumed { transfer_id },
            TransferEvent::TextReceived { from, text } => Self::TextReceived {
                from_name: from.name,
                from_fingerprint: from.fingerprint,
                text,
            },
        }
    }
}

/// Reads the custom avatar, returning None when unselected or unreadable.
pub(crate) fn load_avatar_image(settings: &Settings, data_dir: &Path) -> Option<Vec<u8>> {
    if settings.avatar.as_deref() != Some(AVATAR_CUSTOM) {
        return None;
    }
    match std::fs::read(data_dir.join(AVATAR_FILE)) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            tracing::warn!("failed to read custom avatar ({e}); using default style");
            None
        }
    }
}

/// Builds device identity using the configured name before the hostname.
///
/// Emoji avatars are embedded directly. Custom images advertise `img:<hash>`
/// and are fetched over TCP. Startup and live settings updates share this path,
/// loading the same certificate and preserving the fingerprint.
pub(crate) fn build_identity(
    data_dir: &Path,
    settings: &Settings,
    avatar_image: Option<&[u8]>,
) -> Result<Arc<DeviceIdentity>> {
    let mut identity =
        DeviceIdentity::load_or_create(data_dir).context("failed to load device identity")?;
    if let Some(name) = &settings.display_name
        && !name.trim().is_empty()
    {
        identity.display_name = name.clone();
    }
    identity.avatar = match avatar_image {
        Some(img) => Some(format!("img:{}", blake3::hash(img).to_hex())),
        None => settings.avatar.clone().filter(|a| a != AVATAR_CUSTOM),
    };
    Ok(Arc::new(identity))
}

/// Binds a dual-stack TCP listener, falling back to a random port when occupied.
async fn bind_listener(port: u16) -> Result<TcpListener> {
    match bind_dual_stack(port).await {
        Ok(l) => Ok(l),
        Err(e) => {
            tracing::warn!("failed to listen on port {port} ({e}); using a random port");
            bind_dual_stack(0).await.context("failed to listen")
        }
    }
}

/// Starts identity, listener, receiver, discovery, and event pumps.
pub async fn start_engine(app: AppHandle, data_dir: PathBuf) -> Result<AppState> {
    // Commands and automatic offer acceptance share settings through Arc.
    // Use one startup snapshot to avoid repeated locking.
    let shared_settings = Arc::new(Mutex::new(Settings::load(&data_dir)));
    let settings = lock(&shared_settings).clone();

    let avatar_image = load_avatar_image(&settings, &data_dir);
    let identity = build_identity(&data_dir, &settings, avatar_image.as_deref())?;

    tokio::fs::create_dir_all(&settings.download_dir)
        .await
        .context("failed to create download directory")?;

    let listener = bind_listener(settings.tcp_port).await?;
    let tcp_port = listener
        .local_addr()
        .context("failed to read listening address")?
        .port();

    let (offers_tx, offers_rx) = mpsc::channel::<TransferOffer>(16);
    let (events_tx, events_rx) = mpsc::channel::<TransferEvent>(256);
    let receiver = spawn_receiver(
        Arc::clone(&identity),
        listener,
        ReceiverOptions {
            download_dir: settings.download_dir.clone(),
            avatar_image,
            resume_dir: data_dir.join("resume"),
            pin: settings.pin.clone().filter(|p| !p.is_empty()),
        },
        offers_tx,
        events_tx.clone(),
    )
    .context("failed to start receiver")?;

    // Passive mode discovers peers without advertising this device.
    let (discovery, peers_rx) = DiscoveryService::start(
        &identity,
        tcp_port,
        DEFAULT_DISCOVERY_PORT,
        settings.passive,
    )
    .await
    .context("failed to start discovery service")?;

    let offers: OfferMap = Arc::new(Mutex::new(HashMap::new()));
    let send_controls: ControlMap = Arc::new(Mutex::new(HashMap::new()));
    spawn_pumps(
        app,
        peers_rx,
        events_rx,
        offers_rx,
        Arc::clone(&offers),
        Arc::clone(&identity),
        data_dir.join(AVATAR_CACHE_DIR),
        Arc::clone(&shared_settings),
    );

    tracing::info!(
        name = %identity.display_name,
        port = tcp_port,
        "deskmate engine started"
    );
    let history = Arc::new(crate::history::HistoryStore::load(&data_dir));
    Ok(AppState {
        identity: Mutex::new(identity),
        tcp_port,
        data_dir,
        receiver,
        discovery,
        events_tx,
        offers,
        send_controls,
        interrupted_sends: Arc::new(Mutex::new(HashMap::new())) as InterruptedMap,
        settings: shared_settings,
        history,
        failure_notified: Mutex::new(std::collections::HashSet::new()),
        inline_image_paths: Mutex::new(std::collections::HashSet::new()),
    })
}

/// Starts peer, transfer, and incoming-offer event pumps.
#[expect(
    clippy::too_many_arguments,
    reason = "arguments represent the complete event-pump context"
)]
fn spawn_pumps(
    app: AppHandle,
    mut peers_rx: mpsc::Receiver<PeerEvent>,
    events_rx: mpsc::Receiver<TransferEvent>,
    mut offers_rx: mpsc::Receiver<TransferOffer>,
    offers: OfferMap,
    identity: Arc<DeviceIdentity>,
    avatar_cache: PathBuf,
    settings: Arc<Mutex<Settings>>,
) {
    let peer_app = app.clone();
    let inflight: InflightAvatars = Arc::new(Mutex::new(HashSet::new()));
    tauri::async_runtime::spawn(async move {
        while let Some(event) = peers_rx.recv().await {
            let _ = match event {
                PeerEvent::Up(p) => {
                    // Fetch uncached advertised image avatars without blocking the pump.
                    ensure_peer_avatar(&peer_app, &identity, &avatar_cache, &inflight, &p);
                    peer_app.emit(events::PEER_UP, PeerDto::from(&p))
                }
                PeerEvent::Down(fp) => peer_app.emit(events::PEER_DOWN, fp),
            };
        }
    });

    tauri::async_runtime::spawn(pump_transfer_events(app.clone(), events_rx));

    tauri::async_runtime::spawn(async move {
        while let Some(offer) = offers_rx.recv().await {
            // Trusted-device offers bypass the pending-decision queue.
            let Some(offer) = try_auto_accept(&app, &settings, offer) else {
                continue;
            };
            let offer_id = uuid::Uuid::new_v4().to_string();
            let dto = OfferDto {
                offer_id: offer_id.clone(),
                transfer_id: offer.transfer_id.clone(),
                peer_name: offer.peer.name.clone(),
                peer_fingerprint: offer.peer.fingerprint.clone(),
                peer_platform: offer.peer.platform.clone(),
                peer_avatar: offer.peer.avatar.clone(),
                files: offer.files.iter().map(FileMetaDto::from).collect(),
                total_size: offer.total_size,
            };
            lock(&offers).insert(
                offer_id,
                PendingOffer {
                    reply: offer.reply,
                    file_ids: offer.files.iter().map(|f| f.file_id).collect(),
                },
            );
            notify_if_unfocused(
                &app,
                "Deskmate",
                &crate::locale::text(
                    &app,
                    crate::locale::Text::OfferIncoming {
                        name: &dto.peer_name,
                        n: dto.files.len(),
                        size: &human_bytes(dto.total_size),
                    },
                ),
            );
            let _ = app.emit(events::TRANSFER_OFFER, dto);
        }
    });
}

/// Pumps transfer events, throttling progress per task and forwarding all others.
///
/// At gigabit speeds the engine emits hundreds of updates per second. Limiting
/// them to about 10 Hz reduces frontend rendering work and also drives aggregate
/// taskbar or Dock progress.
async fn pump_transfer_events(app: AppHandle, mut events_rx: mpsc::Receiver<TransferEvent>) {
    // Last forwarded progress time per task, removed at terminal state.
    let mut last_progress: HashMap<String, std::time::Instant> = HashMap::new();
    // Current file progress per active transfer for system progress aggregation.
    let mut active: HashMap<String, (u64, u64)> = HashMap::new();
    while let Some(event) = events_rx.recv().await {
        let mut progress_dirty = false;
        match &event {
            TransferEvent::Progress {
                transfer_id,
                done,
                size,
                ..
            } => {
                active.insert(transfer_id.to_string(), (*done, *size));
                let now = std::time::Instant::now();
                let due = last_progress
                    .get(transfer_id.as_ref())
                    .is_none_or(|t| now.duration_since(*t) >= PROGRESS_EMIT_INTERVAL);
                // Always forward final-file progress so the entry reaches 100 percent.
                if !due && done < size {
                    continue;
                }
                last_progress.insert(transfer_id.to_string(), now);
                progress_dirty = true;
            }
            TransferEvent::Completed { transfer_id }
            | TransferEvent::Cancelled { transfer_id }
            | TransferEvent::Interrupted { transfer_id, .. } => {
                last_progress.remove(transfer_id);
                active.remove(transfer_id);
                progress_dirty = true;
            }
            // Copy received text to the system clipboard when configured.
            TransferEvent::TextReceived { text, .. } => auto_copy_text(&app, text),
            // Authorize read_inline_image for exactly the files the engine
            // finalized as inline clipboard images.
            TransferEvent::FileCompleted {
                path,
                inline_image: true,
                ..
            } => {
                let state = app.state::<crate::state::AppState>();
                crate::state::lock(&state.inline_image_paths).insert(path.clone());
            }
            _ => {}
        }
        if progress_dirty {
            update_taskbar_progress(&app, &active);
        }
        notify_transfer_event(&app, &event);
        emit_transfer_event(&app, TransferEventDto::from(event));
    }
}

/// Synchronizes aggregate active-transfer progress to the taskbar or Dock.
///
/// Engine events do not include task totals, so each file fills the indicator
/// in sequence, matching common download behavior. Linux support depends on
/// libunity and failures are ignored.
fn update_taskbar_progress(app: &AppHandle, active: &HashMap<String, (u64, u64)>) {
    use tauri::window::{ProgressBarState, ProgressBarStatus};
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let state = if active.is_empty() {
        ProgressBarState {
            status: Some(ProgressBarStatus::None),
            progress: None,
        }
    } else {
        let (done, size) = active
            .values()
            .fold((0u64, 0u64), |(d, s), (fd, fs)| (d + fd, s + fs));
        let pct = (done * 100).checked_div(size).unwrap_or(0).min(100);
        ProgressBarState {
            status: Some(ProgressBarStatus::Normal),
            progress: Some(pct),
        }
    };
    if let Err(e) = window.set_progress_bar(state) {
        tracing::debug!("failed to update system progress indicator: {e}");
    }
}

/// Unread event count accumulated while the window is unfocused.
static UNREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Clears unread indicators when the window is shown or focused.
pub(crate) fn clear_unread(app: &AppHandle) {
    if UNREAD.swap(0, std::sync::atomic::Ordering::Relaxed) != 0 {
        apply_badge(app, 0);
    }
}

/// Increments unread count and refreshes the indicator.
fn bump_unread(app: &AppHandle) {
    let n = UNREAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    apply_badge(app, n);
}

/// Displays unread state as a macOS Dock badge or Windows taskbar overlay.
#[cfg_attr(
    target_os = "linux",
    expect(unused_variables, reason = "Linux has no general badge protocol")
)]
fn apply_badge(app: &AppHandle, count: u32) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        let badge = if count == 0 { None } else { Some(count as i64) };
        if let Err(e) = window.set_badge_count(badge) {
            tracing::debug!("failed to update Dock badge: {e}");
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Windows has no numeric badge, so use a dot overlay for unread state.
        let icon = if count == 0 {
            None
        } else {
            tauri::image::Image::from_bytes(include_bytes!("../icons/unread-dot.png")).ok()
        };
        if let Err(e) = window.set_overlay_icon(icon) {
            tracing::debug!("failed to update taskbar overlay icon: {e}");
        }
    }
    #[cfg(target_os = "linux")]
    let _ = window;
}

/// Ensures a local cache entry for a peer's advertised image avatar.
///
/// Cache misses are fetched in the background and emit avatar-ready after
/// persistence. Existing cache entries need no event. Hash mismatches are
/// discarded until a refreshed advertisement arrives.
fn ensure_peer_avatar(
    app: &AppHandle,
    identity: &Arc<DeviceIdentity>,
    cache_dir: &Path,
    inflight: &InflightAvatars,
    peer: &Peer,
) {
    let Some(hash) = peer
        .info
        .avatar
        .as_deref()
        .and_then(|a| a.strip_prefix("img:"))
    else {
        return;
    };
    if !is_safe_hash(hash) {
        return;
    }
    let cache_path = cache_dir.join(format!("{hash}.jpg"));
    if cache_path.exists() {
        return;
    }
    // Allow only one in-flight request per hash.
    if !lock(inflight).insert(hash.to_string()) {
        return;
    }

    let app = app.clone();
    let identity = Arc::clone(identity);
    let inflight = Arc::clone(inflight);
    let hash = hash.to_string();
    let fingerprint = peer.info.fingerprint.clone();
    let (addrs, port) = (peer.addrs.clone(), peer.port);
    tauri::async_runtime::spawn(async move {
        match fetch_avatar(&identity, &addrs, port, Some(fingerprint.clone())).await {
            // fetch_avatar validates the response hash; also verify the advertised hash.
            Ok(Some((got, data))) if got == hash => {
                let ok = async {
                    if let Some(parent) = cache_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&cache_path, &data).await
                }
                .await;
                match ok {
                    Ok(()) => {
                        let _ = app.emit(
                            events::AVATAR_READY,
                            AvatarReadyDto {
                                fingerprint,
                                hash: hash.clone(),
                            },
                        );
                    }
                    Err(e) => tracing::warn!("failed to write avatar cache: {e}"),
                }
            }
            Ok(Some(_)) => {
                tracing::debug!("avatar hash differs from advertisement; discarding")
            }
            Ok(None) => tracing::debug!("peer removed its avatar"),
            Err(e) => tracing::debug!("failed to fetch peer avatar: {e}"),
        }
        lock(&inflight).remove(&hash);
    });
}

/// Automatically accepts trusted-device offers and returns untrusted offers unchanged.
fn try_auto_accept(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    offer: TransferOffer,
) -> Option<TransferOffer> {
    let conflict = {
        let guard = lock(settings);
        if !guard
            .trusted
            .iter()
            .any(|t| t.fingerprint == offer.peer.fingerprint)
        {
            return Some(offer);
        }
        // Without a user in the loop, map Ask to the safer automatic rename.
        match guard.conflict_policy {
            ConflictPolicySetting::Overwrite => ConflictPolicy::Overwrite,
            _ => ConflictPolicy::Rename,
        }
    };
    let decision = OfferDecision::Accept {
        accepted_files: offer.files.iter().map(|f| f.file_id).collect(),
        save_dir: None,
        conflict,
    };
    if offer.reply.send(decision).is_err() {
        // The session is gone; discard it silently.
        return None;
    }
    notify_if_unfocused(
        app,
        "Deskmate",
        &crate::locale::text(
            app,
            crate::locale::Text::AutoReceiving {
                name: &offer.peer.name,
                n: offer.files.len(),
                size: &human_bytes(offer.total_size),
            },
        ),
    );
    // Let the frontend create a progress entry without a confirmation dialog.
    let _ = app.emit(
        events::TRANSFER_AUTOSTART,
        AutoStartDto {
            transfer_id: offer.transfer_id.clone(),
            peer_name: offer.peer.name.clone(),
        },
    );
    None
}

/// Automatic receive event used to create a frontend progress entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoStartDto {
    /// Transfer task ID.
    transfer_id: String,
    /// Sender name.
    peer_name: String,
}

/// Avatar-cache-ready event prompting the frontend to reload it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvatarReadyDto {
    /// Peer fingerprint.
    fingerprint: String,
    /// Avatar hash.
    hash: String,
}

/// Validates an avatar hash before using it in a cache file name.
pub(crate) fn is_safe_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.len() <= 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// Copies received text to the system clipboard when enabled.
fn auto_copy_text(app: &AppHandle, text: &str) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let enabled = lock(&app.state::<AppState>().settings).auto_copy_text;
    if !enabled {
        return;
    }
    if let Err(e) = app.clipboard().write_text(text.to_string()) {
        tracing::debug!("failed to copy received text to clipboard: {e}");
    }
}

/// Sends localized system notifications for significant non-progress events.
fn notify_transfer_event(app: &AppHandle, event: &TransferEvent) {
    use crate::locale::Text;
    let lang = crate::locale::lang(app);
    match event {
        TransferEvent::Completed { transfer_id } => {
            lock(&app.state::<AppState>().failure_notified).remove(transfer_id);
            notify_if_unfocused(app, "Deskmate", &Text::TransferCompleted.localize(lang));
        }
        TransferEvent::Cancelled { transfer_id } => {
            lock(&app.state::<AppState>().failure_notified).remove(transfer_id);
            notify_if_unfocused(app, "Deskmate", &Text::TransferCancelled.localize(lang));
        }
        TransferEvent::Interrupted { transfer_id, .. } => {
            // Send settlement may observe the same failure; only the first
            // observer notifies (see AppState::failure_notified).
            if lock(&app.state::<AppState>().failure_notified).insert(transfer_id.clone()) {
                notify_if_unfocused(app, "Deskmate", &Text::TransferInterrupted.localize(lang));
            }
        }
        TransferEvent::TextReceived { from, text } => {
            // Mark successful automatic copy in the title so the user knows the
            // text is ready to paste without opening the window.
            let copied = lock(&app.state::<AppState>().settings).auto_copy_text;
            let title = Text::IncomingMessage {
                name: &from.name,
                copied,
            }
            .localize(lang);
            notify_if_unfocused(app, &title, &preview_of(text));
        }
        _ => {}
    }
}

/// Sends a notification only when the main window is unfocused.
///
/// The same path increments unread state, which is cleared when the window appears.
/// Focused failures are surfaced by the frontend's in-app notification tray.
pub(crate) fn notify_if_unfocused(app: &AppHandle, title: &str, body: &str) {
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    bump_unread(app);
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!("failed to send system notification: {e}");
    }
}

/// Builds a notification preview from the first 60 characters of the first line.
fn preview_of(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("");
    let mut preview: String = first_line.chars().take(60).collect();
    if preview.len() < text.len() {
        preview.push('…');
    }
    preview
}

/// Formats byte counts, for example 1536 as "1.5 KB".
pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
