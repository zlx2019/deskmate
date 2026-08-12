//! Transfer history persisted to history.json in the data directory.
//!
//! The frontend reports records when transfers reach a terminal state because
//! it has the most complete aggregate status. History is loaded once at
//! startup, then memory becomes authoritative. Appends update memory
//! immediately and persist through the blocking thread pool, avoiding a full
//! read-modify-rewrite on the command thread for every completed transfer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::state::lock;

/// History file name.
const HISTORY_FILE: &str = "history.json";
/// Maximum number of entries to retain, discarding the oldest first.
const HISTORY_CAP: usize = 200;

/// One transfer history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// Transfer task ID.
    pub transfer_id: String,
    /// Direction: "send" or "recv".
    pub direction: String,
    /// Peer name.
    pub peer_name: String,
    /// Terminal status: completed, cancelled, interrupted, or rejected.
    pub status: String,
    /// Number of completed files.
    pub files_done: u32,
    /// Approximate byte count based on progress at the terminal state.
    pub bytes: u64,
    /// Completion time in Unix milliseconds.
    pub at: u64,
    /// Last persisted path, used by "Show in Folder" for received transfers.
    pub last_path: Option<String>,
}

/// In-memory history store with asynchronous persistence.
pub struct HistoryStore {
    /// Data directory used for persistence.
    data_dir: PathBuf,
    /// All entries, newest first.
    entries: Mutex<Vec<HistoryEntry>>,
    /// Serializes persistence. Interleaved fs::write calls could truncate or
    /// corrupt the JSON, which load would treat as an empty history.
    io_lock: Mutex<()>,
}

impl HistoryStore {
    /// Loads history from disk, treating a missing or invalid file as empty.
    pub fn load(data_dir: &Path) -> Self {
        let entries = std::fs::read(data_dir.join(HISTORY_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            data_dir: data_dir.to_path_buf(),
            entries: Mutex::new(entries),
            io_lock: Mutex::new(()),
        }
    }

    /// Returns a snapshot of all history entries, newest first.
    pub fn snapshot(&self) -> Vec<HistoryEntry> {
        lock(&self.entries).clone()
    }

    /// Adds an entry, replacing the same transfer ID, then persists asynchronously.
    pub fn append(self: &Arc<Self>, entry: HistoryEntry) {
        {
            let mut entries = lock(&self.entries);
            entries.retain(|e| e.transfer_id != entry.transfer_id);
            entries.insert(0, entry);
            entries.truncate(HISTORY_CAP);
        }
        let store = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || store.flush());
    }

    /// Removes an entry by transfer ID, then persists asynchronously.
    pub fn remove(self: &Arc<Self>, transfer_id: &str) {
        lock(&self.entries).retain(|e| e.transfer_id != transfer_id);
        let store = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || store.flush());
    }

    /// Clears all history, then persists asynchronously.
    pub fn clear(self: &Arc<Self>) {
        lock(&self.entries).clear();
        let store = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || store.flush());
    }

    /// Persists the current in-memory snapshot.
    ///
    /// This blocking operation is only used from the thread pool. Snapshotting
    /// and writing both hold io_lock, so a later flush observes a newer
    /// snapshot and writes cannot interleave and corrupt the JSON.
    fn flush(&self) {
        let _io = lock(&self.io_lock);
        let json = {
            let entries = lock(&self.entries);
            serde_json::to_vec_pretty(&*entries).unwrap_or_default()
        };
        if let Err(e) = std::fs::write(self.data_dir.join(HISTORY_FILE), json) {
            tracing::warn!("failed to persist transfer history: {e}");
        }
    }
}
