//! Transfer layer: data channels and file transfer engine.
//!
//! Version 1 strategy, sufficient to saturate 2.5 GbE; see `docs/PLAN.md` section 4.4:
//! - Tokio streaming I/O with one MiB chunks and incremental BLAKE3 hashing.
//! - Sequential data channels: `FileHeader`, raw byte stream, then `FileFooter`.
//! - Pause, resume, and cancel use watch channels with local and remote state merged.
//! - Design decision #3: explicit cancellation deletes `.part` files, while
//!   unexpected disconnects retain them for resume.
//!
//! Version 2 leaves room for platform-specific backends such as Linux io_uring
//! plus kTLS, Windows IOCP, macOS `F_NOCACHE`, and parallel streams without
//! changing the public interface.

mod io_tuning;
mod receiver;
mod sender;

pub use receiver::{OfferDecision, ReceiverHandle, ReceiverOptions, TransferOffer, spawn_receiver};
pub use sender::{SendSummary, fetch_avatar, resume_send, send_files, send_text};

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use crate::protocol::{PeerInfo, ProtocolError};
use crate::tls::TlsError;

/// Re-exported from centralized tuning configuration to preserve existing paths.
pub use crate::config::CHUNK_SIZE;
pub(crate) use crate::config::{DATA_IDLE_TIMEOUT, OFFER_TIMEOUT};

/// Temporary suffix for incomplete files.
pub const PART_SUFFIX: &str = ".deskmate.part";

/// Transfer layer errors.
#[derive(Debug, Error)]
pub enum TransferError {
    /// Underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Protocol frame error.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// TLS configuration or handshake error.
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// Peer identity does not match its TLS certificate fingerprint.
    #[error("peer identity does not match its certificate fingerprint")]
    PeerMismatch,
    /// Relative path is unsafe because it is absolute, drive-qualified, or traverses upward.
    #[error("unsafe file path: {path}")]
    InvalidPath {
        /// Original path string.
        path: String,
    },
    /// Source file does not exist or cannot be read.
    #[error("source file unavailable: {0}")]
    SourceNotFound(PathBuf),
    /// Peer rejected the transfer request.
    #[error("peer rejected transfer: {}", reason.as_deref().unwrap_or("no reason provided"))]
    Rejected {
        /// Rejection text supplied by the peer.
        reason: Option<String>,
        /// Structured rejection code added in protocol 1.4 for local UI rendering.
        reason_code: Option<String>,
    },
    /// File integrity verification failed.
    #[error("file integrity check failed: {rel_path}")]
    HashMismatch {
        /// Relative path.
        rel_path: String,
    },
    /// Transfer was explicitly cancelled.
    #[error("transfer cancelled")]
    Cancelled,
    /// Timed out waiting for a handshake response or receiver decision.
    #[error("wait timed out: {0}")]
    Timeout(&'static str),
    /// Data channel referenced an unknown or unaccepted file index.
    #[error("invalid file index: {0}")]
    BadFileId(u32),
    /// Data channel referenced a transfer task that does not exist.
    #[error("unknown transfer task: {0}")]
    UnknownTransfer(String),
    /// The same task already has an active data session.
    #[error("transfer task already has an active data session: {0}")]
    DuplicateDataSession(String),
    /// Resume is unavailable because peer metadata is missing or the source changed.
    #[error("cannot resume transfer: {0}")]
    ResumeUnavailable(String),
    /// Declared resume offset differs from the receiver's `.part` file length.
    #[error("resume offset mismatch: declared {offset}, .part length {part_len} bytes")]
    ResumeOffsetMismatch {
        /// Offset declared by the sender.
        offset: u64,
        /// Current receiver `.part` file length.
        part_len: u64,
    },
    /// Peer avatar exceeds the protocol limit.
    #[error("avatar exceeds size limit: {size} bytes")]
    AvatarTooLarge {
        /// Byte count declared by the peer.
        size: u64,
    },
    /// Peer requires a pairing PIN and the request omitted it or supplied the wrong value.
    #[error("peer requires a pairing PIN")]
    PinRequired,
    /// Collection found no sendable files because sources were empty or ignored.
    #[error("no files available to send")]
    NoValidFiles,
}

impl TransferError {
    /// Stable error code used by localized UIs; `Display` serves logs and the CLI.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Protocol(_) => "protocol",
            Self::Tls(_) => "tls",
            Self::PeerMismatch => "peer_mismatch",
            Self::InvalidPath { .. } => "invalid_path",
            Self::SourceNotFound(_) => "source_not_found",
            Self::Rejected { .. } => "rejected",
            Self::HashMismatch { .. } => "hash_mismatch",
            Self::Cancelled => "cancelled",
            Self::Timeout(_) => "timeout",
            Self::BadFileId(_) => "bad_file_id",
            Self::UnknownTransfer(_) => "unknown_transfer",
            Self::DuplicateDataSession(_) => "duplicate_data_session",
            Self::ResumeUnavailable(_) => "resume_unavailable",
            Self::ResumeOffsetMismatch { .. } => "resume_offset_mismatch",
            Self::AvatarTooLarge { .. } => "avatar_too_large",
            Self::PinRequired => "pin_required",
            Self::NoValidFiles => "no_valid_files",
        }
    }

    /// Returns an optional detail appended to the localized primary message.
    ///
    /// Details are not translated. I/O and protocol errors retain their source
    /// language, while paths and numbers are language-neutral.
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Io(e) => Some(e.to_string()),
            Self::Protocol(e) => Some(e.to_string()),
            Self::Tls(e) => Some(e.to_string()),
            Self::InvalidPath { path } => Some(path.clone()),
            Self::SourceNotFound(p) => Some(p.display().to_string()),
            Self::Rejected { reason, .. } => reason.clone(),
            Self::HashMismatch { rel_path } => Some(rel_path.clone()),
            Self::Timeout(scene) => Some((*scene).to_string()),
            Self::BadFileId(id) => Some(id.to_string()),
            Self::UnknownTransfer(id) | Self::DuplicateDataSession(id) => Some(id.clone()),
            Self::ResumeUnavailable(why) => Some(why.clone()),
            Self::ResumeOffsetMismatch { offset, part_len } => {
                Some(format!("{offset} != {part_len}"))
            }
            Self::AvatarTooLarge { size } => Some(size.to_string()),
            Self::PeerMismatch | Self::Cancelled | Self::PinRequired | Self::NoValidFiles => None,
        }
    }
}

/// Filename conflict policy applied when the receiver finalizes a file.
///
/// "Ask every time" is a UI concept. The UI resolves the user's decision into
/// this enum before dispatch. Serde persists the policy in resume metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Renames automatically, for example `file.txt` to `file (1).txt`.
    #[default]
    Rename,
    /// Overwrites an existing file with the same name.
    Overwrite,
}

/// Transfer control state ordered by merge priority between local and remote state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ControlState {
    /// Normal transfer.
    Running,
    /// Paused and resumable.
    Paused,
    /// Terminal cancellation; the receiver deletes `.part` files.
    Cancelled,
}

/// Transfer event emitted from the engine to the UI or CLI.
#[derive(Debug, Clone)]
pub enum TransferEvent {
    /// Per-file progress update that may be dropped because later updates supersede it.
    ///
    /// String fields use `Arc<str>` because every one MiB chunk emits an event;
    /// shared references avoid repeated allocations on the hot path.
    Progress {
        /// Transfer task ID.
        transfer_id: Arc<str>,
        /// File index.
        file_id: u32,
        /// Relative path.
        rel_path: Arc<str>,
        /// Completed bytes.
        done: u64,
        /// Total file size in bytes.
        size: u64,
    },
    /// One file completed; path is final destination on receive and source on send.
    FileCompleted {
        /// Transfer task ID.
        transfer_id: String,
        /// File index.
        file_id: u32,
        /// File path.
        path: PathBuf,
        /// Peer-declared clipboard-image marker from the manifest, meaningful
        /// only on the receive side; the send side always reports `false`.
        inline_image: bool,
    },
    /// Entire transfer task completed.
    Completed {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// Transfer was explicitly cancelled and temporary files were deleted.
    Cancelled {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// Unexpected interruption retaining `.part` files for design-decision #3 resume.
    Interrupted {
        /// Transfer task ID.
        transfer_id: String,
        /// Locally formatted interruption reason for logs and the CLI.
        reason: String,
        /// Stable code used by the UI for localization.
        code: &'static str,
        /// Untranslated error detail appended for display.
        detail: Option<String>,
    },
    /// Peer paused the transfer; local actions are updated optimistically by the UI.
    Paused {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// Peer resumed the transfer.
    Resumed {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// Text received from a peer.
    TextReceived {
        /// Sender information.
        from: PeerInfo,
        /// Byte-exact text content.
        text: String,
    },
}

/// Event sender using droppable `try_send` for progress and awaited `send` for key events.
#[derive(Clone)]
pub(crate) struct EventSink {
    /// Underlying channel.
    tx: mpsc::Sender<TransferEvent>,
}

impl EventSink {
    /// Wraps a transfer event channel.
    pub(crate) fn new(tx: mpsc::Sender<TransferEvent>) -> Self {
        Self { tx }
    }

    /// Sends progress, dropping it silently when the channel is full.
    fn progress(&self, event: TransferEvent) {
        let _ = self.tx.try_send(event);
    }

    /// Waits to send a key event, ignoring a closed consumer.
    async fn notify(&self, event: TransferEvent) {
        let _ = self.tx.send(event).await;
    }
}

/// Binds a TCP listener, preferring IPv6 dual stack and falling back to IPv4.
///
/// Discovery may advertise global IPv6 addresses. If the receiver listened only
/// on IPv4, the sender would wait through a timeout for every IPv6 candidate.
pub async fn bind_dual_stack(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Socket, Type};
    let try_v6 = || -> std::io::Result<std::net::TcpListener> {
        let s = Socket::new(Domain::IPV6, Type::STREAM, None)?;
        // Windows defaults to v6-only, so disable it to accept mapped IPv4 addresses.
        s.set_only_v6(false)?;
        s.bind(&std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())?;
        s.listen(128)?;
        Ok(s.into())
    };
    let std_listener = match try_v6() {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("IPv6 dual-stack listener unavailable ({e}); falling back to IPv4");
            std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port))?
        }
    };
    std_listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(std_listener)
}

/// Windows reserved device names, including forms with any extension.
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Sanitizes one path component for safe storage on the target platform.
///
/// Windows rules:
/// - Replace `< > : " | ? *` and control characters with `_`. A colon is
///   especially dangerous because NTFS treats `foo:bar` as an alternate data stream.
/// - Remove trailing dots and spaces that Windows would silently strip.
/// - Prefix reserved device names such as CON, PRN, AUX, NUL, COM1-9, and LPT1-9
///   with `_`, including names that have extensions.
fn sanitize_component(name: &str, windows_rules: bool) -> String {
    if !windows_rules {
        return name.to_string();
    }
    let mut cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_end_matches(['.', ' ']);
    if trimmed.len() != cleaned.len() {
        cleaned = if trimmed.is_empty() {
            "_".to_string()
        } else {
            trimmed.to_string()
        };
    }
    let stem = cleaned.split('.').next().unwrap_or("");
    if WINDOWS_RESERVED
        .iter()
        .any(|r| stem.eq_ignore_ascii_case(r))
    {
        cleaned.insert(0, '_');
    }
    cleaned
}

/// Sanitizes a peer-provided relative path against traversal and invalid filenames.
///
/// Backslashes are normalized as separators. Only normal components are retained;
/// drive prefixes, roots, `..`, and empty paths are rejected. Each component is
/// then sanitized for the **local platform** by `sanitize_component`. The result
/// is deterministic, keeping resume `.part` paths aligned with the original
/// `rel_path` used during negotiation.
pub fn sanitize_rel_path(rel: &str) -> Result<PathBuf, TransferError> {
    sanitize_rel_path_for(rel, cfg!(target_os = "windows"))
}

/// Platform-parameterized implementation of [`sanitize_rel_path`] for testing.
fn sanitize_rel_path_for(rel: &str, windows_rules: bool) -> Result<PathBuf, TransferError> {
    let normalized = rel.replace('\\', "/");
    let mut out = PathBuf::new();
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(c) => {
                out.push(sanitize_component(&c.to_string_lossy(), windows_rules));
            }
            Component::CurDir => {}
            _ => {
                return Err(TransferError::InvalidPath {
                    path: rel.to_string(),
                });
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(TransferError::InvalidPath {
            path: rel.to_string(),
        });
    }
    Ok(out)
}

/// Appends a sequence number when the destination exists, such as `file (1).txt`.
pub fn dedup_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()));
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for n in 1u32.. {
        let candidate = parent.join(format!("{stem} ({n}){}", ext.as_deref().unwrap_or("")));
        if !candidate.exists() {
            return candidate;
        }
    }
    // The sequence space exceeds practical directory capacity; use a UUID fallback.
    parent.join(format!(
        "{stem}-{}{}",
        uuid::Uuid::new_v4(),
        ext.as_deref().unwrap_or("")
    ))
}

/// Transfer ignore rules using gitignore syntax.
///
/// Matching uses manifest-relative paths with each selected item's parent as the
/// root, equivalent to `.gitignore` at a repository root. `*.log` matches any
/// depth, `dir/` prunes a subtree, `!keep.log` re-includes a path, and `#` starts
/// a comment. Negations inside a pruned directory are ineffective, matching Git.
#[derive(Debug, Clone)]
pub struct IgnoreRules(ignore::gitignore::Gitignore);

impl IgnoreRules {
    /// Compiles one rule per line and returns the offending line on syntax errors.
    pub fn parse(text: &str) -> Result<Self, String> {
        // An empty root keeps matching relative to the manifest without filesystem prefixes.
        let mut builder = ignore::gitignore::GitignoreBuilder::new("");
        for line in text.lines() {
            builder
                .add_line(None, line)
                .map_err(|e| format!("{line}: {e}"))?;
        }
        let compiled = builder.build().map_err(|e| e.to_string())?;
        Ok(Self(compiled))
    }

    /// Returns whether a relative path is ignored, honoring negated matches.
    fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        self.0.matched(rel, is_dir).is_ignore()
    }
}

/// Expands selected paths into `(absolute path, relative path, size)` manifest entries.
///
/// Relative paths use `/`. A single file uses its filename, while a directory uses
/// `directory-name/internal-path`. `None` disables ignore filtering. Empty results
/// return [`TransferError::NoValidFiles`] instead of starting an empty transfer.
pub fn collect_files(
    paths: &[PathBuf],
    rules: Option<&IgnoreRules>,
) -> Result<Vec<(PathBuf, String, u64)>, TransferError> {
    let mut out = Vec::new();
    for path in paths {
        let meta =
            std::fs::metadata(path).map_err(|_| TransferError::SourceNotFound(path.clone()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| TransferError::SourceNotFound(path.clone()))?;
        // Apply rules to top-level selections so directories can prune entire trees.
        if let Some(rules) = rules
            && rules.is_ignored(&name, meta.is_dir())
        {
            continue;
        }
        if meta.is_dir() {
            walk_dir(path, &name, rules, &mut out)?;
        } else {
            out.push((path.clone(), name, meta.len()));
        }
    }
    if out.is_empty() {
        return Err(TransferError::NoValidFiles);
    }
    Ok(out)
}

/// Recursively collects regular files while skipping non-directory special entries.
///
/// An ignored directory is pruned entirely, so inner negations do not apply, matching Git.
fn walk_dir(
    dir: &Path,
    rel_base: &str,
    rules: Option<&IgnoreRules>,
    out: &mut Vec<(PathBuf, String, u64)>,
) -> Result<(), TransferError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Cross-platform filenames require UTF-8, so invalid bytes are converted lossily.
        let rel = format!("{rel_base}/{}", entry.file_name().to_string_lossy());
        let meta = std::fs::metadata(&path)?;
        if let Some(rules) = rules
            && rules.is_ignored(&rel, meta.is_dir())
        {
            continue;
        }
        if meta.is_dir() {
            walk_dir(&path, &rel, rules, out)?;
        } else if meta.is_file() {
            out.push((path, rel, meta.len()));
        }
    }
    Ok(())
}

/// Waits for control state to return to `Running`, returning an error on cancellation.
///
/// Called before every chunk. Local and remote states are merged by taking the
/// higher-priority value. A dropped watch sender does not release the pause;
/// a 50 ms fallback sleep prevents busy waiting.
pub(crate) async fn wait_if_paused(
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<(), TransferError> {
    loop {
        let state = (*local.borrow()).max(*remote.borrow());
        match state {
            ControlState::Running => return Ok(()),
            ControlState::Cancelled => return Err(TransferError::Cancelled),
            ControlState::Paused => {
                tokio::select! {
                    r = local.changed() => {
                        if r.is_err() {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                    r = remote.changed() => {
                        if r.is_err() {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
            }
        }
    }
}

/// Waits for either control channel to change before the caller re-evaluates both.
///
/// A channel whose sender was dropped is converted to a permanent pending future,
/// allowing the other channel or select branches to progress without a busy loop.
async fn changed_either(
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) {
    async fn wait_one(r: &mut watch::Receiver<ControlState>) {
        if r.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
    tokio::select! {
        _ = wait_one(local) => {}
        _ = wait_one(remote) => {}
    }
}

/// Gracefully closes a connection by sending `close_notify` and draining to EOF.
///
/// With bidirectional TLS shutdown, unread receive data such as the peer's
/// `close_notify` can cause the kernel to send RST and discard in-flight
/// `FileFooter` or `DataDone` frames. Draining to EOF ensures a clean FIN.
pub(crate) async fn graceful_close<S>(stream: &mut S)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _ = stream.shutdown().await;
    let mut drain = [0u8; 256];
    // Wait at most three seconds for an uncooperative peer.
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match stream.read(&mut drain).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
}

/// Reads `len` sequential bytes into the hasher to replay a resume prefix.
///
/// `FileFooter` covers the whole file. After reading, the file position is exactly
/// `len`, so callers can continue sequentially.
pub(crate) async fn hash_prefix(
    file: &mut tokio::fs::File,
    hasher: &mut blake3::Hasher,
    len: u64,
    buf: &mut [u8],
) -> Result<(), TransferError> {
    let mut remaining = len;
    while remaining > 0 {
        let want = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(buf.len()));
        let n = file.read(&mut buf[..want]).await?;
        if n == 0 {
            return Err(TransferError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "file is too short to replay the transferred prefix",
            )));
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(())
}

/// Pumps exactly `len` bytes from `r` to `w` while hashing and reporting progress.
///
/// Shared sender/receiver loop: wait for pause, read, hash, write, and report.
/// Progress starts at `start`, and early EOF reports `eof_msg`.
///
/// Reads and writes race control signals and idle timeouts, so local cancellation
/// is not blocked by a peer that stopped sending or reading. Prolonged inactivity
/// interrupts while preserving resume data.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the pump context"
)]
async fn pump_chunks<R, W>(
    r: &mut R,
    w: &mut W,
    len: u64,
    start: u64,
    hasher: &mut blake3::Hasher,
    eof_msg: &'static str,
    buf: &mut [u8],
    mut on_progress: impl FnMut(u64),
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<(), TransferError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut remaining = len;
    let mut done = start;
    while remaining > 0 {
        wait_if_paused(local, remote).await?;
        let want = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(buf.len()));
        // Reads are cancellation-safe; a control signal returns to the loop head.
        let n = tokio::select! {
            got = r.read(&mut buf[..want]) => got?,
            _ = changed_either(local, remote) => continue,
            _ = tokio::time::sleep(DATA_IDLE_TIMEOUT) => {
                return Err(TransferError::Timeout("data-channel read idle"));
            }
        };
        if n == 0 {
            return Err(TransferError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                eof_msg,
            )));
        }
        hasher.update(&buf[..n]);
        write_all_controlled(w, &buf[..n], local, remote).await?;
        remaining -= n as u64;
        done += n as u64;
        on_progress(done);
    }
    // Flush can also block on a peer that stopped reading, so apply the same timeout.
    tokio::time::timeout(DATA_IDLE_TIMEOUT, w.flush())
        .await
        .map_err(|_| TransferError::Timeout("data-channel write idle"))??;
    Ok(())
}

/// Interruptible `write_all` whose partial writes race control signals and idle timeout.
///
/// Within a chunk, only cancellation takes effect because data already entered
/// the hasher and pausing mid-write would misalign the stream. Pause applies at
/// the next chunk boundary. Individual writes are cancellation-safe.
async fn write_all_controlled<W>(
    w: &mut W,
    mut data: &[u8],
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<(), TransferError>
where
    W: AsyncWrite + Unpin,
{
    while !data.is_empty() {
        if (*local.borrow()).max(*remote.borrow()) == ControlState::Cancelled {
            return Err(TransferError::Cancelled);
        }
        let n = tokio::select! {
            wrote = w.write(data) => wrote?,
            _ = changed_either(local, remote) => continue,
            _ = tokio::time::sleep(DATA_IDLE_TIMEOUT) => {
                return Err(TransferError::Timeout("data-channel write idle"));
            }
        };
        if n == 0 {
            return Err(TransferError::Io(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "data channel wrote zero bytes",
            )));
        }
        data = &data[n..];
    }
    Ok(())
}

/// Sends file content from `offset` through `size` while hashing and reporting progress.
///
/// Returns the hexadecimal full-file hash. When `offset > 0`, the prefix is
/// replayed into the hasher so `FileFooter` always contains the whole-file hash.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the send context"
)]
pub(crate) async fn send_file_body<W>(
    w: &mut W,
    path: &Path,
    size: u64,
    offset: u64,
    buf: &mut [u8],
    on_progress: impl FnMut(u64),
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<String, TransferError>
where
    W: AsyncWrite + Unpin,
{
    let mut file = tokio::fs::File::open(path).await?;
    // Keep one-pass large-file reads out of the macOS page cache.
    io_tuning::advise_no_cache(&file, size);
    let mut hasher = blake3::Hasher::new();
    if offset > 0 {
        // Prefix replay naturally leaves the read position at offset.
        hash_prefix(&mut file, &mut hasher, offset, buf).await?;
    }
    pump_chunks(
        &mut file,
        w,
        size.saturating_sub(offset),
        offset,
        &mut hasher,
        "source file was truncated during transfer",
        buf,
        on_progress,
        local,
        remote,
    )
    .await?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Receives exactly `len` bytes into a file while hashing and reporting progress.
///
/// The caller supplies the hasher, preloaded with an existing `.part` prefix when
/// resuming. Returns the hexadecimal whole-file hash for `FileFooter` comparison.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the receive context"
)]
pub(crate) async fn receive_file_body<R>(
    r: &mut R,
    file: &mut tokio::fs::File,
    len: u64,
    mut hasher: blake3::Hasher,
    offset: u64,
    buf: &mut [u8],
    on_progress: impl FnMut(u64),
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<String, TransferError>
where
    R: AsyncRead + Unpin,
{
    pump_chunks(
        r,
        file,
        len,
        offset,
        &mut hasher,
        "data stream ended early",
        buf,
        on_progress,
        local,
        remote,
    )
    .await?;
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests;
