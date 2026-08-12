//! Receiver engine: listens on TCP and dispatches control or data by the first frame.
//!
//! - A control session beginning with Hello answers the handshake and handles
//!   transfer offers, text, pause, resume, and cancel commands.
//! - A data session beginning with DataHello verifies sender identity, receives
//!   file streams into `.part` files, verifies BLAKE3, and renames them into place.
//!
//! Design decision #3: explicit cancellation deletes `.part` files, while
//! unexpected disconnects retain them for resume.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::Instant;

use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf, split};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;
use crate::identity::DeviceIdentity;
use crate::protocol::{
    ControlMessage, FileMeta, PeerInfo, ProtocolError, ResumeFileState, check_version, read_frame,
    write_frame,
};
use crate::tls::{peer_fingerprint, server_config};

use super::{
    ConflictPolicy, ControlState, EventSink, OFFER_TIMEOUT, PART_SUFFIX, TransferError,
    TransferEvent, dedup_path, graceful_close, receive_file_body, sanitize_rel_path,
};

/// Receiver service configuration.
#[derive(Debug, Clone)]
pub struct ReceiverOptions {
    /// Default download directory, overridable per transfer during the decision.
    pub download_dir: PathBuf,
    /// Local avatar bytes returned to peer avatar requests.
    pub avatar_image: Option<Vec<u8>>,
    /// Resume metadata directory with one JSON file per accepted task.
    pub resume_dir: PathBuf,
    /// Optional pairing PIN, mutable at runtime through [`ReceiverHandle::set_pin`].
    pub pin: Option<String>,
}

use crate::config::{
    HANDSHAKE_TIMEOUT, MAX_CONCURRENT_CONNECTIONS, PENDING_SWEEP_INTERVAL, PENDING_TTL,
    PIN_MAX_FAILURES, PIN_TRACK_CAP, PIN_WINDOW,
};

/// Resume metadata persisted on acceptance and removed on completion or cancellation.
///
/// Unexpected disconnects retain it. Unlike design section 4.2, metadata is
/// centralized in the engine data directory instead of a `.deskmate/` folder in
/// each download directory because users may choose a different destination per transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeMeta {
    /// Sender fingerprint; only the original sender may resume.
    peer_fingerprint: String,
    /// Complete file manifest.
    files: Vec<FileMeta>,
    /// Accepted file indexes.
    accepted: Vec<u32>,
    /// Fully persisted file indexes skipped during resume.
    completed: Vec<u32>,
    /// Destination directory.
    save_dir: PathBuf,
    /// Filename conflict policy retained for resume.
    conflict: ConflictPolicy,
}

/// Transfer request sent to the upper layer for a decision through `reply`.
#[derive(Debug)]
pub struct TransferOffer {
    /// Sender device information.
    pub peer: PeerInfo,
    /// Transfer task ID.
    pub transfer_id: String,
    /// File manifest.
    pub files: Vec<FileMeta>,
    /// Total size in bytes.
    pub total_size: u64,
    /// Decision response channel; five minutes without a response counts as rejection.
    pub reply: oneshot::Sender<OfferDecision>,
}

/// Receiver decision for a transfer request.
#[derive(Debug)]
pub enum OfferDecision {
    /// Accepts selected files, using the default download directory when `save_dir` is `None`.
    Accept {
        /// Accepted file indexes.
        accepted_files: Vec<u32>,
        /// Destination directory for this transfer.
        save_dir: Option<PathBuf>,
        /// Filename conflict policy.
        conflict: ConflictPolicy,
    },
    /// Rejects the transfer.
    Reject {
        /// Optional reason returned to the sender.
        reason: Option<String>,
    },
}

/// Outbound control-session message shared by read-loop responses and task notifications.
enum Outbound {
    /// One frame.
    Frame(ControlMessage),
    /// Frame followed by raw payload bytes, used for avatar responses.
    Payload(ControlMessage, Vec<u8>),
    /// Read loop ended; drain queued messages and gracefully close the write half.
    Shutdown,
}

/// Accepted task waiting for or actively running a data transfer.
struct PendingTransfer {
    /// Sender information used to verify the data connection.
    peer: PeerInfo,
    /// File manifest indexed by file ID.
    files: HashMap<u32, FileMeta>,
    /// Accepted file indexes.
    accepted: HashSet<u32>,
    /// Destination directory.
    save_dir: PathBuf,
    /// Filename conflict policy.
    conflict: ConflictPolicy,
    /// Control-state source updated by Pause, Resume, and Cancel.
    control: watch::Sender<ControlState>,
    /// Sender control-session output used to forward local pause and cancellation.
    notify: mpsc::Sender<Outbound>,
    /// Whether a data session is active, preventing concurrent `.part` writers.
    active: bool,
    /// Registration time used to expire tasks that never open a data connection.
    registered_at: Instant,
}

/// Pending task map written by control sessions and consumed by data sessions.
type PendingMap = Arc<Mutex<HashMap<String, PendingTransfer>>>;

/// Local avatar stored as paired BLAKE3 hash and bytes for consistent responses.
type AvatarData = Option<(String, Vec<u8>)>;

/// Shared receiver service context.
struct ReceiverCtx {
    /// Local device information used in handshakes and updated at runtime.
    self_info: Arc<RwLock<PeerInfo>>,
    /// Default download directory, mutable through [`ReceiverHandle::set_download_dir`].
    download_dir: Arc<RwLock<PathBuf>>,
    /// Runtime-mutable local avatar bytes and hash.
    avatar: Arc<RwLock<AvatarData>>,
    /// Resume metadata directory.
    resume_dir: PathBuf,
    /// Optional runtime-mutable pairing PIN.
    pin: Arc<RwLock<Option<String>>>,
    /// PIN failure rate state: source fingerprint to window start and failure count.
    ///
    /// Counts are per source so one device cannot lock pairing for the entire network.
    pin_failures: Mutex<HashMap<String, (Instant, u32)>>,
    /// Transfer-offer channel to the upper layer.
    offers: mpsc::Sender<TransferOffer>,
    /// Transfer event sink.
    sink: EventSink,
    /// Pending task map.
    pending: PendingMap,
    /// Finalization lock preventing concurrent insertion between dedup existence
    /// checks and rename. Otherwise two same-name files could choose one path and
    /// overwrite each other; rename is fast enough to serialize.
    finalize_lock: tokio::sync::Mutex<()>,
}

/// Receiver handle for querying the listener and controlling active transfers.
pub struct ReceiverHandle {
    /// Actual listening address.
    local_addr: SocketAddr,
    /// Pending task map shared with the service.
    pending: PendingMap,
    /// Runtime-mutable default download directory shared with the service.
    download_dir: Arc<RwLock<PathBuf>>,
    /// Runtime-mutable pairing PIN shared with the service.
    pin: Arc<RwLock<Option<String>>>,
    /// Runtime-mutable handshake identity shared with the service.
    self_info: Arc<RwLock<PeerInfo>>,
    /// Runtime-mutable avatar data shared with the service.
    avatar: Arc<RwLock<AvatarData>>,
}

impl ReceiverHandle {
    /// Returns the actual listening address, including an ephemeral port from bind port zero.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Pauses a transfer and notifies the sender, returning false when not found.
    pub fn pause(&self, transfer_id: &str) -> bool {
        let ok = set_control(&self.pending, transfer_id, ControlState::Paused);
        if ok {
            notify_peer(
                &self.pending,
                transfer_id,
                ControlMessage::Pause {
                    transfer_id: transfer_id.to_string(),
                },
            );
        }
        ok
    }

    /// Resumes a transfer and notifies the sender, returning false when not found.
    pub fn resume(&self, transfer_id: &str) -> bool {
        let ok = set_control(&self.pending, transfer_id, ControlState::Running);
        if ok {
            notify_peer(
                &self.pending,
                transfer_id,
                ControlMessage::Resume {
                    transfer_id: transfer_id.to_string(),
                },
            );
        }
        ok
    }

    /// Cancels a transfer, notifies the sender, and deletes receiving `.part` files.
    pub fn cancel(&self, transfer_id: &str) -> bool {
        cancel_transfer(&self.pending, transfer_id, true)
    }

    /// Returns the current default download directory.
    pub fn download_dir(&self) -> PathBuf {
        read_lock(&self.download_dir).clone()
    }

    /// Changes the default download directory for subsequent transfers.
    pub fn set_download_dir(&self, dir: PathBuf) {
        *self
            .download_dir
            .write()
            .unwrap_or_else(PoisonError::into_inner) = dir;
    }

    /// Changes the pairing PIN for subsequent requests; `None` disables it.
    pub fn set_pin(&self, pin: Option<String>) {
        *self.pin.write().unwrap_or_else(PoisonError::into_inner) = pin;
    }

    /// Updates handshake identity and avatar at runtime.
    ///
    /// Subsequent control sessions return the new identity in HelloAck and the new
    /// image in AvatarRequest. The hash is computed here and stored with its bytes.
    pub fn set_self_info(&self, info: PeerInfo, avatar_image: Option<Vec<u8>>) {
        *self
            .self_info
            .write()
            .unwrap_or_else(PoisonError::into_inner) = info;
        *self.avatar.write().unwrap_or_else(PoisonError::into_inner) =
            avatar_image.map(|img| (blake3::hash(&img).to_hex().to_string(), img));
    }
}

/// Reads a lock, recovering poisoned data directly.
fn read_lock(lock: &RwLock<PathBuf>) -> RwLockReadGuard<'_, PathBuf> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Starts accepting connections on the given listener and returns a receiver handle.
///
/// In M1, the service lifetime matches the process. The accept loop exits when
/// the listener closes or encounters a fatal error.
pub fn spawn_receiver(
    identity: Arc<DeviceIdentity>,
    listener: TcpListener,
    options: ReceiverOptions,
    offers: mpsc::Sender<TransferOffer>,
    events: mpsc::Sender<TransferEvent>,
) -> Result<ReceiverHandle, TransferError> {
    let acceptor = TlsAcceptor::from(Arc::new(server_config(&identity)?));
    let local_addr = listener.local_addr()?;
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    let download_dir = Arc::new(RwLock::new(options.download_dir));
    let pin = Arc::new(RwLock::new(options.pin));
    let self_info = Arc::new(RwLock::new(identity.peer_info()));
    // Precompute and pair the avatar hash with its bytes for consistent responses.
    let avatar: Arc<RwLock<AvatarData>> = Arc::new(RwLock::new(
        options
            .avatar_image
            .map(|img| (blake3::hash(&img).to_hex().to_string(), img)),
    ));
    let ctx = Arc::new(ReceiverCtx {
        self_info: Arc::clone(&self_info),
        download_dir: Arc::clone(&download_dir),
        avatar: Arc::clone(&avatar),
        resume_dir: options.resume_dir,
        pin: Arc::clone(&pin),
        pin_failures: Mutex::new(HashMap::new()),
        offers,
        sink: EventSink::new(events),
        pending: Arc::clone(&pending),
        finalize_lock: tokio::sync::Mutex::new(()),
    });
    spawn_pending_sweeper(&ctx);
    tokio::spawn(accept_loop(listener, acceptor, ctx));
    Ok(ReceiverHandle {
        local_addr,
        pending,
        download_dir,
        pin,
        self_info,
        avatar,
    })
}

/// Validates a request PIN, allowing all requests when PIN protection is disabled.
///
/// Brute-force limits are tracked per TLS certificate fingerprint. Five failures
/// in 60 seconds reject that source for the rest of the window, even with a later
/// correct PIN, while other devices remain unaffected. Success clears the source.
fn pin_ok(ctx: &ReceiverCtx, peer_fp: &str, provided: Option<&str>) -> bool {
    let expected = ctx
        .pin
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(expected) = expected else {
        return true;
    };
    let mut failures = ctx
        .pin_failures
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    // Remove expired windows so the table remains bounded by currently failing sources.
    failures.retain(|_, (start, _)| now.duration_since(*start) <= PIN_WINDOW);
    if failures
        .get(peer_fp)
        .is_some_and(|(_, fails)| *fails >= PIN_MAX_FAILURES)
    {
        tracing::warn!("too many PIN attempts; rejecting this source for the window");
        return false;
    }
    if provided == Some(expected.as_str()) {
        failures.remove(peer_fp);
        return true;
    }
    // When full, reject untracked sources conservatively; many distinct failing
    // fingerprints within one window already indicate an attack.
    if failures.len() >= PIN_TRACK_CAP && !failures.contains_key(peer_fp) {
        tracing::warn!("too many PIN failure sources; rejecting a new source");
        return false;
    }
    let entry = failures.entry(peer_fp.to_string()).or_insert((now, 0));
    entry.1 += 1;
    tracing::warn!("PIN validation failed ({}/{PIN_MAX_FAILURES})", entry.1);
    false
}

/// Starts TTL cleanup for accepted tasks whose sender never opens a data connection.
///
/// A weak context lets the sweeper exit after all service strong references from
/// the accept loop and connection tasks are gone, avoiding resource retention in tests.
fn spawn_pending_sweeper(ctx: &Arc<ReceiverCtx>) {
    let weak = Arc::downgrade(ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PENDING_SWEEP_INTERVAL);
        tick.tick().await; // Skip the interval's immediate first tick.
        loop {
            tick.tick().await;
            let Some(ctx) = weak.upgrade() else { return };
            sweep_pending(&ctx);
        }
    });
}

/// Removes expired, unstarted tasks and their resume metadata.
///
/// Only inactive entries older than the TTL are removed. Active sessions are
/// unaffected. Resume files and `.part` data left by an unexpected disconnect
/// are also outside this map and retain their existing semantics.
fn sweep_pending(ctx: &ReceiverCtx) {
    let mut expired = Vec::new();
    lock_pending(&ctx.pending).retain(|id, task| {
        let dead = !task.active && task.registered_at.elapsed() > PENDING_TTL;
        if dead {
            expired.push(id.clone());
        }
        !dead
    });
    // Delete metadata outside the lock; a single unlink is fast enough here.
    for id in &expired {
        remove_resume_meta(&ctx.resume_dir, id);
        tracing::info!(transfer_id = %id, "removed expired unstarted receive task");
    }
}

/// Accept loop that handles each connection independently.
async fn accept_loop(listener: TcpListener, acceptor: TlsAcceptor, ctx: Arc<ReceiverCtx>) {
    // Reject new connections at capacity so malicious half-open sessions cannot
    // grow without bound and exhaust file descriptors.
    let conn_permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    loop {
        let (tcp, remote) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!("accept failed; receiver service is exiting: {e}");
                return;
            }
        };
        let Ok(permit) = Arc::clone(&conn_permits).try_acquire_owned() else {
            tracing::warn!(%remote, "concurrent connection limit reached; rejecting connection");
            continue;
        };
        let acceptor = acceptor.clone();
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            let _permit = permit; // Hold one slot for the connection task's lifetime.
            if let Err(e) = handle_connection(tcp, acceptor, ctx).await {
                tracing::debug!(%remote, "connection session ended: {e}");
            }
        });
    }
}

/// Handles one connection by dispatching on the first frame after TLS.
async fn handle_connection(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    ctx: Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    tcp.set_nodelay(true)?;
    super::io_tuning::tune_socket(&tcp);
    // Limit the unauthenticated phase so silent connections cannot occupy slots.
    let mut tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp))
        .await
        .map_err(|_| TransferError::Timeout("TLS handshake"))??;
    // Mutual authentication uses the client certificate fingerprint as identity.
    let conn_fp =
        peer_fingerprint(tls.get_ref().1.peer_certificates()).ok_or(TransferError::PeerMismatch)?;

    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut tls))
        .await
        .map_err(|_| TransferError::Timeout("first frame"))?;
    match first? {
        ControlMessage::Hello { version, info } => {
            check_version(&version)?;
            // The declared identity must match the TLS certificate.
            if info.fingerprint != conn_fp {
                return Err(TransferError::PeerMismatch);
            }
            control_session(tls, info, &ctx).await
        }
        ControlMessage::DataHello { transfer_id } => {
            data_session(tls, transfer_id, &conn_fp, &ctx).await
        }
        other => Err(TransferError::Protocol(ProtocolError::Unexpected {
            expected: "hello | data_hello",
            got: other.kind().to_string(),
        })),
    }
}

/// Control session with a read loop and serialized response/notification write pump.
///
/// In addition to request/response traffic, local pause and cancel commands must
/// be pushed proactively over the same connection through a writer retained in
/// the task map. A reader blocked on `read_frame` cannot own both write sources.
async fn control_session(
    tls: TlsStream<TcpStream>,
    peer: PeerInfo,
    ctx: &Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    let (mut rd, wr) = split(tls);
    let (out_tx, out_rx) = mpsc::channel::<Outbound>(16);
    let pump = tokio::spawn(write_pump(wr, out_rx));
    let result = control_loop(&mut rd, &peer, ctx, &out_tx).await;
    // Let the write pump finish queued messages and close gracefully. The task map
    // may retain sender clones, so dropping this sender alone cannot stop the pump.
    let _ = out_tx.send(Outbound::Shutdown).await;
    drop(out_tx);
    let _ = pump.await;
    result
}

/// Control-session write pump owning the write half and serializing all output.
///
/// Any write failure indicates disconnection and closes the channel. Later
/// notifications are best-effort, while failed responses are equivalent to a
/// disconnected session.
async fn write_pump(mut wr: WriteHalf<TlsStream<TcpStream>>, mut rx: mpsc::Receiver<Outbound>) {
    while let Some(out) = rx.recv().await {
        let ok = match out {
            Outbound::Frame(msg) => write_frame(&mut wr, &msg).await.is_ok(),
            Outbound::Payload(msg, data) => {
                write_frame(&mut wr, &msg).await.is_ok()
                    && wr.write_all(&data).await.is_ok()
                    && wr.flush().await.is_ok()
            }
            Outbound::Shutdown => break,
        };
        if !ok {
            return; // The connection is already gone, so graceful closure is impossible.
        }
    }
    // Normal cleanup sends close_notify and FIN so the peer receives clean EOF.
    let _ = wr.shutdown().await;
}

/// Control-session read loop that handles frames until Bye or disconnection.
///
/// All output is queued through `out`. Queue failure means the write pump exited
/// after a connection failure, equivalent to reading a disconnect.
async fn control_loop(
    rd: &mut ReadHalf<TlsStream<TcpStream>>,
    peer: &PeerInfo,
    ctx: &Arc<ReceiverCtx>,
    out: &mpsc::Sender<Outbound>,
) -> Result<(), TransferError> {
    // Snapshot before awaiting because the standard-library read guard is not Send.
    let self_info = ctx
        .self_info
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let hello_ack = ControlMessage::HelloAck {
        version: PROTOCOL_VERSION.to_string(),
        info: self_info,
    };
    if out.send(Outbound::Frame(hello_ack)).await.is_err() {
        return Ok(());
    }

    loop {
        let reply = match read_frame(rd).await {
            Ok(ControlMessage::TransferRequest {
                transfer_id,
                files,
                total_size,
                pin,
            }) => {
                // PIN is the first gate; invalid requests never reach the confirmation UI.
                if pin_ok(ctx, &peer.fingerprint, pin.as_deref()) {
                    handle_request(peer, transfer_id, files, total_size, ctx, out).await
                } else {
                    ControlMessage::TransferResponse {
                        transfer_id,
                        accepted_files: Vec::new(),
                        reason: Some("a valid pairing PIN is required".to_string()),
                        pin_required: true,
                        reason_code: Some("pin_required".to_string()),
                    }
                }
            }
            Ok(ControlMessage::Text { text, pin }) => {
                if pin_ok(ctx, &peer.fingerprint, pin.as_deref()) {
                    ctx.sink
                        .notify(TransferEvent::TextReceived {
                            from: peer.clone(),
                            text,
                        })
                        .await;
                    ControlMessage::TextAck
                } else {
                    ControlMessage::TextRejected { pin_required: true }
                }
            }
            Ok(ControlMessage::ResumeQuery { transfer_id }) => {
                let files = resume_states(peer, &transfer_id, ctx, out);
                ControlMessage::ResumeInfo { transfer_id, files }
            }
            Ok(ControlMessage::AvatarRequest) => {
                if !send_avatar(ctx, out).await {
                    return Ok(());
                }
                continue;
            }
            Ok(ControlMessage::Pause { transfer_id }) => {
                // Synchronize a sender-side pause and emit an event for the UI.
                if set_control(&ctx.pending, &transfer_id, ControlState::Paused) {
                    ctx.sink.notify(TransferEvent::Paused { transfer_id }).await;
                }
                continue;
            }
            Ok(ControlMessage::Resume { transfer_id }) => {
                if set_control(&ctx.pending, &transfer_id, ControlState::Running) {
                    ctx.sink
                        .notify(TransferEvent::Resumed { transfer_id })
                        .await;
                }
                continue;
            }
            Ok(ControlMessage::Cancel { transfer_id }) => {
                // Do not echo peer cancellation; the data session reports final state.
                cancel_transfer(&ctx.pending, &transfer_id, false);
                continue;
            }
            // Peer farewell or disconnection ends the control session normally.
            Ok(ControlMessage::Bye) | Err(_) => return Ok(()),
            Ok(other) => {
                tracing::debug!(kind = other.kind(), "control session ignored message");
                continue;
            }
        };
        if out.send(Outbound::Frame(reply)).await.is_err() {
            return Ok(());
        }
    }
}

/// Responds to an avatar request with hash and length followed by raw bytes.
///
/// An unset avatar uses size zero and no payload. `false` means the write pump
/// exited after disconnection and the caller should end the session.
async fn send_avatar(ctx: &Arc<ReceiverCtx>, out: &mpsc::Sender<Outbound>) -> bool {
    // Snapshot paired hash and bytes so no lock crosses an await.
    let snapshot = ctx
        .avatar
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let (hash, img) = snapshot.unwrap_or_default();
    let msg = ControlMessage::AvatarResponse {
        hash,
        size: img.len() as u64,
    };
    let outbound = if img.is_empty() {
        Outbound::Frame(msg)
    } else {
        Outbound::Payload(msg, img)
    };
    out.send(outbound).await.is_ok()
}

/// Sends a transfer request for a decision, registers accepted work, and builds a response.
async fn handle_request(
    peer: &PeerInfo,
    transfer_id: String,
    files: Vec<FileMeta>,
    total_size: u64,
    ctx: &Arc<ReceiverCtx>,
    out: &mpsc::Sender<Outbound>,
) -> ControlMessage {
    let reject = |transfer_id: String, reason: &str, code: &str| ControlMessage::TransferResponse {
        transfer_id,
        accepted_files: Vec::new(),
        reason: Some(reason.to_string()),
        pin_required: false,
        reason_code: Some(code.to_string()),
    };

    // The task ID becomes part of a metadata filename, so reject unsafe characters.
    if !is_safe_transfer_id(&transfer_id) {
        return reject(transfer_id, "invalid transfer task ID", "bad_transfer_id");
    }

    let (reply_tx, reply_rx) = oneshot::channel();
    let offer = TransferOffer {
        peer: peer.clone(),
        transfer_id: transfer_id.clone(),
        files: files.clone(),
        total_size,
        reply: reply_tx,
    };
    if ctx.offers.send(offer).await.is_err() {
        return reject(transfer_id, "receiver unavailable", "receiver_unavailable");
    }

    match tokio::time::timeout(OFFER_TIMEOUT, reply_rx).await {
        Ok(Ok(OfferDecision::Accept {
            accepted_files,
            save_dir,
            conflict,
        })) => {
            let valid: Vec<u32> = accepted_files
                .into_iter()
                .filter(|id| files.iter().any(|f| f.file_id == *id))
                .collect();
            if valid.is_empty() {
                return reject(transfer_id, "no valid files selected", "no_valid_files");
            }
            let save_dir = save_dir.unwrap_or_else(|| read_lock(&ctx.download_dir).clone());
            register_pending(
                peer,
                transfer_id,
                files,
                valid,
                save_dir,
                conflict,
                ctx,
                out,
            )
        }
        Ok(Ok(OfferDecision::Reject { reason })) => ControlMessage::TransferResponse {
            transfer_id,
            accepted_files: Vec::new(),
            reason,
            pin_required: false,
            reason_code: Some("declined".to_string()),
        },
        // A dropped response or decision timeout is treated as rejection.
        Ok(Err(_)) | Err(_) => reject(
            transfer_id,
            "receiver decision timed out",
            "decision_timeout",
        ),
    }
}

/// Persists resume metadata, registers the accepted task, and returns its response.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the registration context"
)]
fn register_pending(
    peer: &PeerInfo,
    transfer_id: String,
    files: Vec<FileMeta>,
    valid: Vec<u32>,
    save_dir: PathBuf,
    conflict: ConflictPolicy,
    ctx: &Arc<ReceiverCtx>,
    out: &mpsc::Sender<Outbound>,
) -> ControlMessage {
    // Reject duplicate active or waiting IDs so their control channel is not replaced.
    if lock_pending(&ctx.pending).contains_key(&transfer_id) {
        return ControlMessage::TransferResponse {
            transfer_id,
            accepted_files: Vec::new(),
            reason: Some("a task with this ID is already active".to_string()),
            pin_required: false,
            reason_code: Some("duplicate_task".to_string()),
        };
    }
    save_resume_meta(
        &ctx.resume_dir,
        &transfer_id,
        &ResumeMeta {
            peer_fingerprint: peer.fingerprint.clone(),
            files: files.clone(),
            accepted: valid.clone(),
            completed: Vec::new(),
            save_dir: save_dir.clone(),
            conflict,
        },
    );
    let (control, _) = watch::channel(ControlState::Running);
    lock_pending(&ctx.pending).insert(
        transfer_id.clone(),
        PendingTransfer {
            peer: peer.clone(),
            files: files.into_iter().map(|f| (f.file_id, f)).collect(),
            accepted: valid.iter().copied().collect(),
            save_dir,
            conflict,
            control,
            notify: out.clone(),
            active: false,
            registered_at: Instant::now(),
        },
    );
    ControlMessage::TransferResponse {
        transfer_id,
        accepted_files: valid,
        reason: None,
        pin_required: false,
        reason_code: None,
    }
}

/// Negotiates resume by validating metadata and identity, rebuilding the task,
/// and returning offsets for incomplete files.
///
/// Invalid IDs, missing metadata, identity mismatches, and fully completed tasks
/// return an empty list, which the sender treats as unavailable.
fn resume_states(
    peer: &PeerInfo,
    transfer_id: &str,
    ctx: &Arc<ReceiverCtx>,
    out: &mpsc::Sender<Outbound>,
) -> Vec<ResumeFileState> {
    if !is_safe_transfer_id(transfer_id) {
        return Vec::new();
    }
    // Reject resume while the task is active or waiting; replacement would detach
    // the original session's control channel.
    if lock_pending(&ctx.pending).contains_key(transfer_id) {
        tracing::warn!(
            transfer_id,
            "task is still active; rejecting resume negotiation"
        );
        return Vec::new();
    }
    let Some(meta) = load_resume_meta(&ctx.resume_dir, transfer_id) else {
        return Vec::new();
    };
    // Only the original sender may resume.
    if meta.peer_fingerprint != peer.fingerprint {
        tracing::warn!(
            transfer_id,
            "resume requester does not match original sender"
        );
        return Vec::new();
    }

    // Incomplete files are accepted minus completed; the `.part` length is the offset.
    let mut states = Vec::new();
    for file in &meta.files {
        if !meta.accepted.contains(&file.file_id) || meta.completed.contains(&file.file_id) {
            continue;
        }
        let Ok(rel) = sanitize_rel_path(&file.rel_path) else {
            continue;
        };
        let received = std::fs::metadata(part_path_of(&meta.save_dir.join(&rel), transfer_id))
            .map(|m| m.len())
            .unwrap_or(0);
        states.push(ResumeFileState {
            file_id: file.file_id,
            rel_path: file.rel_path.clone(),
            size: file.size,
            // Restart from zero if an anomalous `.part` file exceeds the declared size.
            received: if received > file.size { 0 } else { received },
        });
    }
    if states.is_empty() {
        return Vec::new();
    }

    // Rebuild the task so the sender's next data connection follows `data_session`.
    let (control, _) = watch::channel(ControlState::Running);
    lock_pending(&ctx.pending).insert(
        transfer_id.to_string(),
        PendingTransfer {
            peer: peer.clone(),
            files: meta.files.iter().cloned().map(|f| (f.file_id, f)).collect(),
            accepted: states.iter().map(|s| s.file_id).collect(),
            save_dir: meta.save_dir.clone(),
            conflict: meta.conflict,
            control,
            notify: out.clone(),
            active: false,
            registered_at: Instant::now(),
        },
    );
    states
}

/// Receives a data stream after verifying sender identity and task ownership.
async fn data_session(
    mut tls: TlsStream<TcpStream>,
    transfer_id: String,
    conn_fp: &str,
    ctx: &Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    // Snapshot task information while retaining the entry for control-session commands.
    let (files, accepted, save_dir, conflict, control_rx) = {
        let mut guard = lock_pending(&ctx.pending);
        let task = guard
            .get_mut(&transfer_id)
            .ok_or_else(|| TransferError::UnknownTransfer(transfer_id.clone()))?;
        // The data connection must come from the original requester.
        if task.peer.fingerprint != conn_fp {
            return Err(TransferError::PeerMismatch);
        }
        // Exclusive ownership prevents concurrent sessions from corrupting the
        // same `.part` files. This return leaves the active session's entry intact.
        if task.active {
            return Err(TransferError::DuplicateDataSession(transfer_id.clone()));
        }
        task.active = true;
        (
            task.files.clone(),
            task.accepted.clone(),
            task.save_dir.clone(),
            task.conflict,
            task.control.subscribe(),
        )
    };

    // Receiver-local and peer commands share one watch source written by the
    // control session. Clone it only to reuse the sender's local/remote merge API.
    let mut local = control_rx.clone();
    let mut remote = control_rx;
    let result = receive_data_stream(
        &mut tls,
        &transfer_id,
        &files,
        &accepted,
        &save_dir,
        conflict,
        &ctx.resume_dir,
        &ctx.finalize_lock,
        &mut local,
        &mut remote,
        &ctx.sink,
    )
    .await;

    // On success, exchange close_notify and drain the peer for a clean bidirectional close.
    if result.is_ok() {
        graceful_close(&mut tls).await;
    }

    // Remove the task map entry whenever the session ends.
    lock_pending(&ctx.pending).remove(&transfer_id);
    match result {
        Ok(()) => {
            remove_resume_meta(&ctx.resume_dir, &transfer_id);
            ctx.sink
                .notify(TransferEvent::Completed { transfer_id })
                .await;
            Ok(())
        }
        Err(TransferError::Cancelled) => {
            remove_resume_meta(&ctx.resume_dir, &transfer_id);
            ctx.sink
                .notify(TransferEvent::Cancelled { transfer_id })
                .await;
            Ok(())
        }
        Err(e) => {
            // Unexpected interruption retains `.part` files and resume metadata.
            ctx.sink
                .notify(TransferEvent::Interrupted {
                    transfer_id,
                    reason: e.to_string(),
                    code: e.code(),
                    detail: e.detail(),
                })
                .await;
            Err(e)
        }
    }
}

/// Receives FileHeader, byte stream, and FileFooter sequences until DataDone.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the session context"
)]
async fn receive_data_stream(
    tls: &mut TlsStream<TcpStream>,
    transfer_id: &str,
    files: &HashMap<u32, FileMeta>,
    accepted: &HashSet<u32>,
    save_dir: &Path,
    conflict: ConflictPolicy,
    resume_dir: &Path,
    finalize_lock: &tokio::sync::Mutex<()>,
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<(), TransferError> {
    // Reuse the chunk buffer, task ID, and in-memory resume metadata for the session.
    let mut buf = vec![0u8; super::CHUNK_SIZE];
    let tid: Arc<str> = Arc::from(transfer_id);
    let mut resume_meta = load_resume_meta(resume_dir, transfer_id);
    loop {
        // Frame gaps share the idle limit. Peer-local pause can stop at file
        // boundaries for a long time, so the configured value is deliberately long.
        let frame = tokio::time::timeout(crate::config::DATA_IDLE_TIMEOUT, read_frame(tls))
            .await
            .map_err(|_| TransferError::Timeout("waiting for data frame"))?;
        match frame? {
            ControlMessage::FileHeader { file_id, offset } => {
                let meta = files
                    .get(&file_id)
                    .ok_or(TransferError::BadFileId(file_id))?;
                if !accepted.contains(&file_id) {
                    return Err(TransferError::BadFileId(file_id));
                }
                receive_one_file(
                    tls,
                    &tid,
                    meta,
                    offset,
                    save_dir,
                    conflict,
                    finalize_lock,
                    &mut buf,
                    local,
                    remote,
                    sink,
                )
                .await?;
                // Persist completion so later resume negotiation skips this file.
                if let Some(state) = resume_meta.as_mut() {
                    mark_completed_persist(state, resume_dir, transfer_id, file_id).await;
                }
            }
            ControlMessage::DataDone => return Ok(()),
            other => {
                return Err(TransferError::Protocol(ProtocolError::Unexpected {
                    expected: "file_header | data_done",
                    got: other.kind().to_string(),
                }));
            }
        }
    }
}

/// Receives one file into `.part`, verifies FileFooter, and renames it into place.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the session context"
)]
async fn receive_one_file(
    tls: &mut TlsStream<TcpStream>,
    transfer_id: &Arc<str>,
    meta: &FileMeta,
    offset: u64,
    save_dir: &Path,
    conflict: ConflictPolicy,
    finalize_lock: &tokio::sync::Mutex<()>,
    buf: &mut [u8],
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<(), TransferError> {
    let rel = sanitize_rel_path(&meta.rel_path)?;
    let target = save_dir.join(&rel);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part_path = part_path_of(&target, transfer_id);

    // FileFooter hashes the whole file, so resume first replays the existing prefix.
    let mut hasher = blake3::Hasher::new();
    let mut file = open_part(&part_path, offset, meta.size, &mut hasher, buf).await?;

    let (tid, rel_str, fid, size) = (
        Arc::clone(transfer_id),
        Arc::<str>::from(meta.rel_path.as_str()),
        meta.file_id,
        meta.size,
    );
    let progress_sink = sink.clone();
    let received_hash = match receive_file_body(
        tls,
        &mut file,
        meta.size.saturating_sub(offset),
        hasher,
        offset,
        buf,
        move |done| {
            progress_sink.progress(TransferEvent::Progress {
                transfer_id: Arc::clone(&tid),
                file_id: fid,
                rel_path: Arc::clone(&rel_str),
                done,
                size,
            });
        },
        local,
        remote,
    )
    .await
    {
        Ok(hash) => hash,
        Err(e) => {
            drop(file);
            // Explicit cancellation deletes the temporary file; interruptions retain it.
            if matches!(e, TransferError::Cancelled) {
                let _ = tokio::fs::remove_file(&part_path).await;
            }
            return Err(e);
        }
    };
    drop(file);

    // Hash or file-ID mismatch discards data; other protocol errors retain `.part`.
    if let Err(e) = expect_footer(tls, meta, &received_hash).await {
        if matches!(e, TransferError::HashMismatch { .. }) {
            let _ = tokio::fs::remove_file(&part_path).await;
        }
        return Err(e);
    }

    finalize_file(
        &target,
        &part_path,
        conflict,
        finalize_lock,
        transfer_id,
        meta,
        sink,
    )
    .await
}

/// Builds a `.part` path containing the task ID prefix so tasks do not collide.
///
/// Naming must be deterministic for resume and therefore depends only on target
/// and transfer ID. [`is_safe_transfer_id`] guarantees ASCII, and eight characters
/// are sufficient to avoid practical collisions.
fn part_path_of(target: &Path, transfer_id: &str) -> PathBuf {
    let tid = &transfer_id[..transfer_id.len().min(8)];
    let mut os = target.to_path_buf().into_os_string();
    os.push(format!(".{tid}{PART_SUFFIX}"));
    PathBuf::from(os)
}

/// Opens a `.part` file, creating it for a new transfer or validating and replaying
/// its prefix when resuming.
///
/// `size` is the full-file size. New transfers preallocate so insufficient space
/// fails before partial writes, and large files bypass page cache. Preallocation
/// does not change visible length because resume depends on it.
async fn open_part(
    part_path: &Path,
    offset: u64,
    size: u64,
    hasher: &mut blake3::Hasher,
    buf: &mut [u8],
) -> Result<tokio::fs::File, TransferError> {
    if offset == 0 {
        let file = tokio::fs::File::create(part_path).await?;
        // Remove a newly created empty `.part` after preallocation failure because
        // it contains nothing resumable.
        if let Err(e) = super::io_tuning::preallocate(&file, size).await {
            drop(file);
            let _ = tokio::fs::remove_file(part_path).await;
            return Err(e.into());
        }
        super::io_tuning::advise_no_cache(&file, size);
        return Ok(file);
    }
    // Resume offset must match current `.part` length to prevent stream misalignment.
    let part_len = tokio::fs::metadata(part_path).await.map(|m| m.len()).ok();
    if part_len != Some(offset) {
        return Err(TransferError::ResumeOffsetMismatch {
            offset,
            part_len: part_len.unwrap_or(0),
        });
    }
    let mut existing = tokio::fs::File::open(part_path).await?;
    super::hash_prefix(&mut existing, hasher, offset, buf).await?;
    drop(existing);
    let file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(part_path)
        .await?;
    super::io_tuning::advise_no_cache(&file, size);
    Ok(file)
}

/// Reads and verifies FileFooter, requiring both file ID and whole-file hash to match.
async fn expect_footer(
    tls: &mut TlsStream<TcpStream>,
    meta: &FileMeta,
    received_hash: &str,
) -> Result<(), TransferError> {
    let footer = tokio::time::timeout(crate::config::DATA_IDLE_TIMEOUT, read_frame(tls))
        .await
        .map_err(|_| TransferError::Timeout("waiting for file footer"))??;
    let ControlMessage::FileFooter {
        file_id,
        hash: expected,
    } = footer
    else {
        return Err(TransferError::Protocol(ProtocolError::Unexpected {
            expected: "file_footer",
            got: footer.kind().to_string(),
        }));
    };
    if file_id != meta.file_id || received_hash != expected {
        return Err(TransferError::HashMismatch {
            rel_path: meta.rel_path.clone(),
        });
    }
    Ok(())
}

/// Finalizes a verified file by resolving conflicts, renaming atomically, and emitting an event.
///
/// The finalization lock spans dedup existence checks through rename. Without it,
/// concurrent same-name files could choose one path and overwrite each other.
async fn finalize_file(
    target: &Path,
    part_path: &Path,
    conflict: ConflictPolicy,
    finalize_lock: &tokio::sync::Mutex<()>,
    transfer_id: &str,
    meta: &FileMeta,
    sink: &EventSink,
) -> Result<(), TransferError> {
    let final_path = {
        let _guard = finalize_lock.lock().await;
        // Apply the receiver-selected policy; rename atomically overwrites on both platforms.
        let final_path = match conflict {
            // Dedup performs synchronous stat calls, so run it in the blocking pool.
            ConflictPolicy::Rename => {
                let t = target.to_path_buf();
                tokio::task::spawn_blocking(move || dedup_path(&t))
                    .await
                    .map_err(|e| TransferError::Io(std::io::Error::other(e)))?
            }
            ConflictPolicy::Overwrite => target.to_path_buf(),
        };
        tokio::fs::rename(part_path, &final_path).await?;
        final_path
        // Release before event backpressure can delay other file finalizations.
    };
    sink.notify(TransferEvent::FileCompleted {
        transfer_id: transfer_id.to_string(),
        file_id: meta.file_id,
        path: final_path,
    })
    .await;
    Ok(())
}

/// Returns whether a transfer ID is safe for filenames using UUID-compatible characters.
fn is_safe_transfer_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Returns the resume metadata path after callers validate [`is_safe_transfer_id`].
fn resume_meta_path(dir: &Path, transfer_id: &str) -> PathBuf {
    dir.join(format!("{transfer_id}.resume.json"))
}

/// Writes resume metadata on a best-effort basis.
///
/// Failure only disables resume and does not affect the current transfer.
fn save_resume_meta(dir: &Path, transfer_id: &str, meta: &ResumeMeta) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(meta).unwrap_or_default();
        std::fs::write(resume_meta_path(dir, transfer_id), json)
    };
    if let Err(e) = write() {
        tracing::warn!(
            transfer_id,
            "failed to write resume metadata; current transfer continues: {e}"
        );
    }
}

/// Loads resume metadata, returning `None` when missing or invalid.
fn load_resume_meta(dir: &Path, transfer_id: &str) -> Option<ResumeMeta> {
    let bytes = std::fs::read(resume_meta_path(dir, transfer_id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Removes resume metadata after completion or cancellation.
fn remove_resume_meta(dir: &Path, transfer_id: &str) {
    let _ = std::fs::remove_file(resume_meta_path(dir, transfer_id));
}

/// Marks a file complete in memory and persists an ordered snapshot in the blocking pool.
async fn mark_completed_persist(
    meta: &mut ResumeMeta,
    dir: &Path,
    transfer_id: &str,
    file_id: u32,
) {
    if meta.completed.contains(&file_id) {
        return;
    }
    meta.completed.push(file_id);
    let snapshot = meta.clone();
    let dir = dir.to_path_buf();
    let tid = transfer_id.to_string();
    let _ = tokio::task::spawn_blocking(move || save_resume_meta(&dir, &tid, &snapshot)).await;
}

/// Locks the pending map, recovering poisoned data directly.
fn lock_pending(pending: &PendingMap) -> MutexGuard<'_, HashMap<String, PendingTransfer>> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Updates transfer control state, returning false when the task does not exist.
///
/// `send_replace` stores state even without subscribers. Commands may arrive
/// before the data session subscribes, and late subscribers still receive the latest value.
fn set_control(pending: &PendingMap, transfer_id: &str, state: ControlState) -> bool {
    match lock_pending(pending).get(transfer_id) {
        Some(t) => {
            t.control.send_replace(state);
            true
        }
        None => false,
    }
}

/// Forwards a local control command through the sender's control-session write pump.
///
/// This is best-effort after disconnection; the peer falls back to data-channel
/// idle timeout. `try_send` is non-blocking and safe on synchronous Tauri IPC threads.
fn notify_peer(pending: &PendingMap, transfer_id: &str, msg: ControlMessage) {
    let tx = lock_pending(pending)
        .get(transfer_id)
        .map(|t| t.notify.clone());
    if let Some(tx) = tx
        && tx.try_send(Outbound::Frame(msg)).is_err()
    {
        tracing::debug!(
            transfer_id,
            "failed to forward control command; session may be closed"
        );
    }
}

/// Cancels and removes a transfer while subscribers retain terminal `Cancelled` state.
///
/// `notify` forwards locally initiated cancellation. Peer-initiated cancellation
/// must pass false to prevent command loops.
fn cancel_transfer(pending: &PendingMap, transfer_id: &str, notify: bool) -> bool {
    match lock_pending(pending).remove(transfer_id) {
        Some(task) => {
            task.control.send_replace(ControlState::Cancelled);
            if notify
                && task
                    .notify
                    .try_send(Outbound::Frame(ControlMessage::Cancel {
                        transfer_id: transfer_id.to_string(),
                    }))
                    .is_err()
            {
                tracing::debug!(
                    transfer_id,
                    "failed to forward cancellation; session may be closed"
                );
            }
            true
        }
        None => false,
    }
}
