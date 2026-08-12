//! Sender engine: opens control sessions, waits for decisions, and pushes files.
//!
//! Session flow from design section 4.2:
//! 1. Control connection: TLS handshake with fingerprint pin, Hello/HelloAck,
//!    then TransferRequest.
//! 2. Wait up to five minutes for the receiver's human decision.
//! 3. Data connection: DataHello, then FileHeader, raw bytes, and FileFooter per file.
//! 4. The control read half continuously listens for peer pause, resume, and cancel.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use rustls_pki_types::ServerName;
use tokio::io::{AsyncReadExt, ReadHalf, WriteHalf, split};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::PROTOCOL_VERSION;
use crate::identity::DeviceIdentity;
use crate::protocol::{
    ControlMessage, FileMeta, MAX_AVATAR_SIZE, PeerInfo, ProtocolError, check_version, read_frame,
    write_frame,
};
use crate::tls::{client_config, peer_fingerprint};

use super::{
    ControlState, EventSink, IgnoreRules, OFFER_TIMEOUT, TransferError, TransferEvent,
    collect_files, graceful_close, send_file_body,
};

use crate::config::{CONNECT_TIMEOUT, REPLY_TIMEOUT};

/// Transfer result summary.
#[derive(Debug)]
pub struct SendSummary {
    /// Transfer task ID.
    pub transfer_id: String,
    /// Peer device information.
    pub peer: PeerInfo,
    /// Files actually sent after possible partial acceptance.
    pub files_sent: usize,
    /// Bytes actually sent.
    pub bytes_sent: u64,
}

/// Sends files or directories to a target peer until completion.
///
/// - `expected_fp`: `Some` strictly pins the discovery fingerprint. `None` is
///   direct CLI integration mode, where callers must display the actual fingerprint.
/// - `transfer_id`: `Some` uses a caller-generated task ID for preregistering
///   controls; `None` generates one internally. Both return it in [`SendSummary`].
/// - `pin`: required when the peer enables pairing PIN protection.
/// - `ignore`: optional gitignore-style transfer rules.
/// - `control`: local pause, resume, and cancel channel.
/// - `events`: progress and result event stream.
#[expect(
    clippy::too_many_arguments,
    reason = "public entry point whose parameters define the complete transfer context"
)]
pub async fn send_files(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: Option<String>,
    pin: Option<String>,
    paths: &[PathBuf],
    ignore: Option<&IgnoreRules>,
    control: watch::Receiver<ControlState>,
    events: mpsc::Sender<TransferEvent>,
) -> Result<SendSummary, TransferError> {
    let sink = EventSink::new(events);

    // Collect and number the manifest.
    let entries = collect_files_blocking(paths, ignore).await?;
    let files = build_manifest(&entries);
    let total_size: u64 = files.iter().map(|f| f.size).sum();
    let transfer_id = transfer_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Open the control connection and handshake.
    let (mut ctrl, peer) = connect_and_hello(identity, addrs, port, expected_fp.clone()).await?;

    let accepted = negotiate_offer(&mut ctrl, &transfer_id, files.clone(), total_size, pin).await?;

    // Every accepted file starts at offset zero for a new transfer.
    let plan: Vec<SendItem> = files
        .iter()
        .filter(|f| accepted.contains(&f.file_id))
        .map(|f| SendItem {
            file_id: f.file_id,
            abs_path: entries[f.file_id as usize].0.clone(),
            rel_path: f.rel_path.clone(),
            size: f.size,
            offset: 0,
        })
        .collect();

    run_data_phase(
        identity,
        addrs,
        port,
        expected_fp,
        transfer_id,
        ctrl,
        peer,
        &plan,
        control,
        &sink,
    )
    .await
}

/// Collects the manifest in the blocking pool because directory traversal is synchronous I/O.
async fn collect_files_blocking(
    paths: &[PathBuf],
    ignore: Option<&IgnoreRules>,
) -> Result<Vec<(PathBuf, String, u64)>, TransferError> {
    let owned = paths.to_vec();
    let rules = ignore.cloned();
    tokio::task::spawn_blocking(move || collect_files(&owned, rules.as_ref()))
        .await
        .map_err(|e| TransferError::Io(std::io::Error::other(e)))?
}

/// Numbers collected files in order and converts them to protocol metadata.
fn build_manifest(entries: &[(PathBuf, String, u64)]) -> Vec<FileMeta> {
    entries
        .iter()
        .enumerate()
        .map(|(i, (_, rel, size))| FileMeta {
            file_id: u32::try_from(i).unwrap_or(u32::MAX),
            rel_path: rel.clone(),
            size: *size,
        })
        .collect()
}

/// Sends a transfer request and returns the accepted file IDs.
///
/// Uses a long timeout for a human decision. PIN failures and full rejection map
/// to dedicated errors.
async fn negotiate_offer(
    ctrl: &mut TlsStream<TcpStream>,
    transfer_id: &str,
    files: Vec<FileMeta>,
    total_size: u64,
    pin: Option<String>,
) -> Result<HashSet<u32>, TransferError> {
    write_frame(
        ctrl,
        &ControlMessage::TransferRequest {
            transfer_id: transfer_id.to_string(),
            files,
            total_size,
            pin,
        },
    )
    .await?;
    let resp = tokio::time::timeout(OFFER_TIMEOUT, read_frame(ctrl))
        .await
        .map_err(|_| TransferError::Timeout("receiver decision"))??;
    let ControlMessage::TransferResponse {
        accepted_files,
        reason,
        pin_required,
        reason_code,
        ..
    } = resp
    else {
        return Err(unexpected("transfer_response", &resp));
    };
    if pin_required {
        return Err(TransferError::PinRequired);
    }
    if accepted_files.is_empty() {
        return Err(TransferError::Rejected {
            reason,
            reason_code,
        });
    }
    Ok(accepted_files.into_iter().collect())
}

/// Resumes an interrupted task by negotiating offsets and sending only missing ranges.
///
/// `transfer_id` and `paths` must match the original send. The receiver aligns by
/// relative path and size. Renamed or resized sources cannot resume because the
/// whole-file hash would no longer continue.
#[expect(
    clippy::too_many_arguments,
    reason = "public entry point whose parameters define the complete resume context"
)]
pub async fn resume_send(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: &str,
    paths: &[PathBuf],
    ignore: Option<&IgnoreRules>,
    control: watch::Receiver<ControlState>,
    events: mpsc::Sender<TransferEvent>,
) -> Result<SendSummary, TransferError> {
    let sink = EventSink::new(events);

    // Index the local manifest by relative path because traversal order is unstable
    // across runs. The caller supplies the ignore-rule snapshot from the original send.
    let entries = collect_files_blocking(paths, ignore).await?;
    let by_rel: HashMap<&str, (&PathBuf, u64)> = entries
        .iter()
        .map(|(abs, rel, size)| (rel.as_str(), (abs, *size)))
        .collect();

    // Negotiate resume offsets.
    let (mut ctrl, peer) = connect_and_hello(identity, addrs, port, expected_fp.clone()).await?;
    write_frame(
        &mut ctrl,
        &ControlMessage::ResumeQuery {
            transfer_id: transfer_id.to_string(),
        },
    )
    .await?;
    let resp = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut ctrl))
        .await
        .map_err(|_| TransferError::Timeout("resume response"))??;
    let ControlMessage::ResumeInfo { files, .. } = resp else {
        return Err(unexpected("resume_info", &resp));
    };
    if files.is_empty() {
        return Err(TransferError::ResumeUnavailable(
            "peer has no resume data for this task; it may be complete or metadata may be missing"
                .to_string(),
        ));
    }

    // Validate local alignment and build the remaining send plan.
    let mut plan = Vec::with_capacity(files.len());
    for state in &files {
        let Some((abs, size)) = by_rel.get(state.rel_path.as_str()) else {
            return Err(TransferError::ResumeUnavailable(format!(
                "local file is missing: {}",
                state.rel_path
            )));
        };
        if *size != state.size {
            return Err(TransferError::ResumeUnavailable(format!(
                "source file changed: {}",
                state.rel_path
            )));
        }
        plan.push(SendItem {
            file_id: state.file_id,
            abs_path: (*abs).clone(),
            rel_path: state.rel_path.clone(),
            size: state.size,
            offset: state.received.min(state.size),
        });
    }

    run_data_phase(
        identity,
        addrs,
        port,
        expected_fp,
        transfer_id.to_string(),
        ctrl,
        peer,
        &plan,
        control,
        &sink,
    )
    .await
}

/// Runs the data phase by splitting control I/O, pushing data, and reporting final state.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the session context"
)]
async fn run_data_phase(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: String,
    ctrl: TlsStream<TcpStream>,
    peer: PeerInfo,
    plan: &[SendItem],
    control: watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<SendSummary, TransferError> {
    // The read half listens for peer commands; the write half forwards local
    // commands and sends Bye during cleanup.
    let (ctrl_read, ctrl_write) = split(ctrl);
    let (remote_tx, remote_rx) = watch::channel(ControlState::Running);
    let listen_task = tokio::spawn(listen_remote_control(
        ctrl_read,
        transfer_id.clone(),
        remote_tx,
        sink.clone(),
    ));
    let (stop_tx, stop_rx) = oneshot::channel();
    let forward_task = tokio::spawn(forward_local_control(
        ctrl_write,
        control.clone(),
        transfer_id.clone(),
        stop_rx,
    ));

    let result = push_data(
        identity,
        addrs,
        port,
        expected_fp,
        &transfer_id,
        plan,
        control,
        remote_rx,
        sink,
    )
    .await;

    // Stop listening; the forwarder sends any unsynchronized final state and Bye.
    listen_task.abort();
    let _ = stop_tx.send(());
    let _ = forward_task.await;
    match &result {
        Ok(_) => {
            sink.notify(TransferEvent::Completed {
                transfer_id: transfer_id.clone(),
            })
            .await;
        }
        Err(TransferError::Cancelled) => {
            sink.notify(TransferEvent::Cancelled {
                transfer_id: transfer_id.clone(),
            })
            .await;
        }
        Err(e) => {
            sink.notify(TransferEvent::Interrupted {
                transfer_id: transfer_id.clone(),
                reason: e.to_string(),
                code: e.code(),
                detail: e.detail(),
            })
            .await;
        }
    }

    let (files_sent, bytes_sent) = result?;
    Ok(SendSummary {
        transfer_id,
        peer,
        files_sent,
        bytes_sent,
    })
}

/// Sends byte-exact text and returns peer device information.
///
/// `pin` is required when the peer enables pairing PIN protection.
pub async fn send_text(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    pin: Option<String>,
    text: &str,
) -> Result<PeerInfo, TransferError> {
    let (mut ctrl, peer) = connect_and_hello(identity, addrs, port, expected_fp).await?;
    write_frame(
        &mut ctrl,
        &ControlMessage::Text {
            text: text.to_string(),
            pin,
        },
    )
    .await?;
    let resp = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut ctrl))
        .await
        .map_err(|_| TransferError::Timeout("text acknowledgment"))??;
    match resp {
        ControlMessage::TextAck => {}
        ControlMessage::TextRejected { pin_required: true } => {
            return Err(TransferError::PinRequired);
        }
        other => return Err(unexpected("text_ack", &other)),
    }
    let _ = write_frame(&mut ctrl, &ControlMessage::Bye).await;
    Ok(peer)
}

/// Fetches a peer avatar as `(hash, image bytes)`, or `None` when unset.
///
/// Called when discovery advertises `img:<hash>` and the local cache misses.
/// Response bytes must match the declared hash or they are discarded as poisoned.
pub async fn fetch_avatar(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
) -> Result<Option<(String, Vec<u8>)>, TransferError> {
    let (mut ctrl, _peer) = connect_and_hello(identity, addrs, port, expected_fp).await?;
    write_frame(&mut ctrl, &ControlMessage::AvatarRequest).await?;
    let resp = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut ctrl))
        .await
        .map_err(|_| TransferError::Timeout("avatar response"))??;
    let ControlMessage::AvatarResponse { hash, size } = resp else {
        return Err(unexpected("avatar_response", &resp));
    };
    if size == 0 {
        let _ = write_frame(&mut ctrl, &ControlMessage::Bye).await;
        return Ok(None);
    }
    if size > MAX_AVATAR_SIZE {
        return Err(TransferError::AvatarTooLarge { size });
    }
    // Raw image bytes follow the frame.
    let mut data = vec![0u8; usize::try_from(size).unwrap_or(0)];
    tokio::time::timeout(REPLY_TIMEOUT, ctrl.read_exact(&mut data))
        .await
        .map_err(|_| TransferError::Timeout("avatar data"))??;
    if blake3::hash(&data).to_hex().to_string() != hash {
        return Err(TransferError::HashMismatch {
            rel_path: "avatar".to_string(),
        });
    }
    let _ = write_frame(&mut ctrl, &ControlMessage::Bye).await;
    Ok(Some((hash, data)))
}

/// Tries candidate addresses in order and returns the first successful TCP connection.
async fn connect_first(addrs: &[IpAddr], port: u16) -> Result<TcpStream, TransferError> {
    let mut last_err: Option<std::io::Error> = None;
    for addr in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((*addr, port))).await {
            Ok(Ok(tcp)) => return Ok(tcp),
            Ok(Err(e)) => {
                tracing::debug!(%addr, "candidate address connection failed: {e}");
                last_err = Some(e);
            }
            Err(_) => {
                tracing::debug!(%addr, "candidate address connection timed out");
                last_err = Some(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("connection to {addr}:{port} timed out"),
                ));
            }
        }
    }
    Err(TransferError::Io(last_err.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "candidate address list is empty",
        )
    })))
}

/// Establishes a TLS connection with optional fingerprint pinning.
async fn connect_tls(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
) -> Result<TlsStream<TcpStream>, TransferError> {
    let config = Arc::new(client_config(identity, expected_fp)?);
    let tcp = connect_first(addrs, port).await?;
    tcp.set_nodelay(true)?;
    super::io_tuning::tune_socket(&tcp);
    // SNI uses a fixed name because certificate verification relies only on the fingerprint.
    let name = ServerName::try_from("deskmate")
        .map_err(|e| TransferError::Io(std::io::Error::other(e)))?;
    Ok(TlsConnector::from(config).connect(name, tcp).await?)
}

/// Opens a control connection, completes Hello, and verifies version and peer identity.
async fn connect_and_hello(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
) -> Result<(TlsStream<TcpStream>, PeerInfo), TransferError> {
    let mut tls = connect_tls(identity, addrs, port, expected_fp).await?;
    write_frame(
        &mut tls,
        &ControlMessage::Hello {
            version: PROTOCOL_VERSION.to_string(),
            info: identity.peer_info(),
        },
    )
    .await?;
    let ack = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut tls))
        .await
        .map_err(|_| TransferError::Timeout("handshake response"))??;
    let ControlMessage::HelloAck { version, info } = ack else {
        return Err(unexpected("hello_ack", &ack));
    };
    check_version(&version)?;
    // The declared fingerprint must match the TLS certificate to prevent impersonation.
    let tls_fp = peer_fingerprint(tls.get_ref().1.peer_certificates());
    if tls_fp.as_deref() != Some(info.fingerprint.as_str()) {
        return Err(TransferError::PeerMismatch);
    }
    Ok((tls, info))
}

/// Per-file data-phase item with zero offset for new sends or peer progress for resume.
struct SendItem {
    /// File index aligned with the receiver manifest.
    file_id: u32,
    /// Local absolute path.
    abs_path: PathBuf,
    /// Relative path displayed in progress events.
    rel_path: String,
    /// Total file size in bytes.
    size: u64,
    /// Starting offset.
    offset: u64,
}

/// Opens the data connection and sends each planned file.
#[expect(
    clippy::too_many_arguments,
    reason = "internal assembly function whose parameters are the session context"
)]
async fn push_data(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: &str,
    plan: &[SendItem],
    mut local: watch::Receiver<ControlState>,
    mut remote: watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<(usize, u64), TransferError> {
    let mut data = connect_tls(identity, addrs, port, expected_fp).await?;
    write_frame(
        &mut data,
        &ControlMessage::DataHello {
            transfer_id: transfer_id.to_string(),
        },
    )
    .await?;

    let mut files_sent = 0usize;
    let mut bytes_sent = 0u64;
    // Reuse the chunk buffer and task ID throughout the data phase.
    let mut buf = vec![0u8; super::CHUNK_SIZE];
    let tid: Arc<str> = Arc::from(transfer_id);
    for item in plan {
        write_frame(
            &mut data,
            &ControlMessage::FileHeader {
                file_id: item.file_id,
                offset: item.offset,
            },
        )
        .await?;

        let (tid, rel, fid, size) = (
            Arc::clone(&tid),
            Arc::<str>::from(item.rel_path.as_str()),
            item.file_id,
            item.size,
        );
        let progress_sink = sink.clone();
        let hash = send_file_body(
            &mut data,
            &item.abs_path,
            item.size,
            item.offset,
            &mut buf,
            move |done| {
                progress_sink.progress(TransferEvent::Progress {
                    transfer_id: Arc::clone(&tid),
                    file_id: fid,
                    rel_path: Arc::clone(&rel),
                    done,
                    size,
                });
            },
            &mut local,
            &mut remote,
        )
        .await?;

        write_frame(
            &mut data,
            &ControlMessage::FileFooter {
                file_id: item.file_id,
                hash,
            },
        )
        .await?;
        sink.notify(TransferEvent::FileCompleted {
            transfer_id: transfer_id.to_string(),
            file_id: item.file_id,
            path: item.abs_path.clone(),
        })
        .await;
        files_sent += 1;
        bytes_sent += item.size.saturating_sub(item.offset);
    }

    write_frame(&mut data, &ControlMessage::DataDone).await?;
    // Wait for the peer to drain and close so process exit does not RST in-flight frames.
    graceful_close(&mut data).await;
    Ok((files_sent, bytes_sent))
}

/// Listens for peer pause, resume, and cancel commands and updates remote state.
///
/// Pause and resume emit UI events. Cancel does not because the data pump
/// immediately reports the unified `Cancelled` final state.
async fn listen_remote_control(
    mut ctrl_read: ReadHalf<TlsStream<TcpStream>>,
    transfer_id: String,
    remote: watch::Sender<ControlState>,
    sink: EventSink,
) {
    loop {
        match read_frame(&mut ctrl_read).await {
            Ok(ControlMessage::Pause { transfer_id: id }) if id == transfer_id => {
                let _ = remote.send(ControlState::Paused);
                sink.notify(TransferEvent::Paused {
                    transfer_id: transfer_id.clone(),
                })
                .await;
            }
            Ok(ControlMessage::Resume { transfer_id: id }) if id == transfer_id => {
                let _ = remote.send(ControlState::Running);
                sink.notify(TransferEvent::Resumed {
                    transfer_id: transfer_id.clone(),
                })
                .await;
            }
            Ok(ControlMessage::Cancel { transfer_id: id }) if id == transfer_id => {
                let _ = remote.send(ControlState::Cancelled);
                return;
            }
            // Stop on peer closure; the data phase observes its own I/O result.
            Ok(ControlMessage::Bye) | Err(_) => return,
            Ok(other) => {
                tracing::debug!(kind = other.kind(), "control channel ignored message");
            }
        }
    }
}

/// Forwards local control changes to the receiver as Pause, Resume, or Cancel.
///
/// The peer engine synchronizes semantics and updates its UI. A broken connection
/// exits silently, with data-channel idle timeout as fallback. When the data phase
/// stops or the control source closes, this task sends any unsynchronized terminal
/// cancellation and Bye. It owns the write half, so only it can send the farewell.
async fn forward_local_control(
    mut ctrl_write: WriteHalf<TlsStream<TcpStream>>,
    mut local: watch::Receiver<ControlState>,
    transfer_id: String,
    mut stop: oneshot::Receiver<()>,
) {
    // The peer initially sees Running; synchronize differences before waiting again.
    let mut synced = ControlState::Running;
    loop {
        let now = *local.borrow();
        if now != synced {
            let msg = match now {
                ControlState::Paused => ControlMessage::Pause {
                    transfer_id: transfer_id.clone(),
                },
                ControlState::Running => ControlMessage::Resume {
                    transfer_id: transfer_id.clone(),
                },
                ControlState::Cancelled => ControlMessage::Cancel {
                    transfer_id: transfer_id.clone(),
                },
            };
            if write_frame(&mut ctrl_write, &msg).await.is_err() {
                return; // The control connection is gone, so Bye cannot be sent.
            }
            synced = now;
        }
        if synced == ControlState::Cancelled {
            break; // Cancellation is terminal and the data phase will finish.
        }
        tokio::select! {
            changed = local.changed() => {
                if changed.is_err() {
                    break; // The caller released the control source during cleanup.
                }
            }
            _ = &mut stop => break,
        }
    }
    // Stop and a state change may become ready together. Send a missed terminal
    // cancellation so the peer does not treat it as an unexpected resumable disconnect.
    let last = *local.borrow();
    if last == ControlState::Cancelled && synced != last {
        let _ = write_frame(
            &mut ctrl_write,
            &ControlMessage::Cancel {
                transfer_id: transfer_id.clone(),
            },
        )
        .await;
    }
    let _ = write_frame(&mut ctrl_write, &ControlMessage::Bye).await;
}

/// Builds an unexpected-message protocol error.
fn unexpected(expected: &'static str, got: &ControlMessage) -> TransferError {
    TransferError::Protocol(ProtocolError::Unexpected {
        expected,
        got: got.kind().to_string(),
    })
}
