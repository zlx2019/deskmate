//! Application runtime state: engine handles and data shared across commands.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use deskmate_core::discovery::DiscoveryService;
use deskmate_core::identity::DeviceIdentity;
use deskmate_core::transfer::{ControlState, OfferDecision, ReceiverHandle, TransferEvent};
use tokio::sync::{mpsc, oneshot, watch};

use crate::settings::Settings;

/// Incoming offer awaiting a frontend decision.
pub struct PendingOffer {
    /// Decision response sent back to the receiver engine.
    pub reply: oneshot::Sender<OfferDecision>,
    /// IDs of every file in the manifest; milestone 2 accepts the entire offer.
    pub file_ids: Vec<u32>,
}

/// Pending decision map: offer ID to response channel.
pub type OfferMap = Arc<Mutex<HashMap<String, PendingOffer>>>;
/// Sender control map: transfer ID to pause/cancel state source.
pub type ControlMap = Arc<Mutex<HashMap<String, watch::Sender<ControlState>>>>;

/// Interrupted send task with the original parameters needed for resumption.
pub struct InterruptedSend {
    /// Target peer fingerprint.
    pub fingerprint: String,
    /// Original source paths.
    pub paths: Vec<PathBuf>,
    /// Ignore-rule snapshot captured when sending. Resumption reuses it so
    /// mid-transfer rule changes cannot desynchronize the manifest.
    pub ignore_rules: String,
    /// Clipboard-image marker preserved so PIN retries keep inline display.
    pub inline_image: bool,
}

/// Interrupted task map: transfer ID to sender-side resumption parameters.
pub type InterruptedMap = Arc<Mutex<HashMap<String, InterruptedSend>>>;

/// Global application state managed by Tauri and borrowed by commands.
pub struct AppState {
    /// Device identity snapshot. Profile changes replace it atomically while
    /// in-progress tasks retain the previous snapshot.
    pub identity: Mutex<Arc<DeviceIdentity>>,
    /// Actual listening port.
    pub tcp_port: u16,
    /// Data directory containing identity and settings.
    pub data_dir: PathBuf,
    /// Receiver handle for pause, cancel, and download-directory updates.
    pub receiver: ReceiverHandle,
    /// Discovery service for peer snapshots and shutdown.
    pub discovery: DiscoveryService,
    /// Event sender shared by send tasks and the receiver event pump.
    pub events_tx: mpsc::Sender<TransferEvent>,
    /// Incoming offers awaiting decisions.
    pub offers: OfferMap,
    /// Sender-side transfer controls.
    pub send_controls: ControlMap,
    /// Interrupted sends waiting for the user to resume them.
    pub interrupted_sends: InterruptedMap,
    /// Current settings shared with automatic whitelist acceptance.
    pub settings: Arc<Mutex<Settings>>,
    /// In-memory transfer history with asynchronous persistence.
    pub history: Arc<crate::history::HistoryStore>,
    /// Transfer IDs whose failure already produced a notification. Data-phase
    /// failures are observed by both the engine event pump and send settlement;
    /// whichever inserts first sends the single notification. Retries remove
    /// their ID so a repeated failure notifies again.
    pub failure_notified: Mutex<HashSet<String>>,
    /// Final paths of received inline clipboard images. read_inline_image only
    /// serves paths registered here, so the frontend can never read arbitrary
    /// files. Session-scoped and small: one entry per received screenshot.
    pub inline_image_paths: Mutex<HashSet<PathBuf>>,
}

/// Locks a standard mutex, recovering the inner data if it was poisoned.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Returns the current identity snapshot as a cheap Arc clone.
pub fn current_identity(state: &AppState) -> Arc<DeviceIdentity> {
    Arc::clone(&lock(&state.identity))
}
