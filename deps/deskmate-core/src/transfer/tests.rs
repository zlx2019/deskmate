//! End-to-end transfer engine tests over localhost loopback.

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

use crate::identity::DeviceIdentity;
use crate::transfer::{
    ConflictPolicy, ControlState, IgnoreRules, OfferDecision, ReceiverOptions, TransferError,
    TransferEvent, collect_files, dedup_path, fetch_avatar, resume_send, sanitize_rel_path,
    sanitize_rel_path_for, send_files, send_text, spawn_receiver,
};

/// Isolated temporary directory removed automatically on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("deskmate-tx-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Generates deterministic test data without a random source.
fn pattern_data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// Test environment with two identities and a running receiver.
struct Harness {
    sender_id: Arc<DeviceIdentity>,
    receiver_fp: String,
    target: (IpAddr, u16),
    download_dir: PathBuf,
    events: mpsc::Receiver<TransferEvent>,
    handle: crate::transfer::ReceiverHandle,
    _dirs: (TempDir, TempDir, TempDir),
}

/// Builds a loopback environment whose receiver accepts or rejects all offers.
async fn harness(accept_all: bool) -> Harness {
    harness_with(accept_all, ConflictPolicy::default(), None, None).await
}

/// Like [`harness`], with configurable conflict policy, avatar, and pairing PIN.
async fn harness_with(
    accept_all: bool,
    conflict: ConflictPolicy,
    avatar_image: Option<Vec<u8>>,
    pin: Option<String>,
) -> Harness {
    let (d_send, d_recv, d_down) = (TempDir::new(), TempDir::new(), TempDir::new());
    let sender_id = Arc::new(DeviceIdentity::load_or_create(d_send.path()).unwrap());
    let receiver_id = Arc::new(DeviceIdentity::load_or_create(d_recv.path()).unwrap());

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (offers_tx, mut offers_rx) = mpsc::channel(8);
    let (events_tx, events_rx) = mpsc::channel(256);
    let handle = spawn_receiver(
        Arc::clone(&receiver_id),
        listener,
        ReceiverOptions {
            download_dir: d_down.path().to_path_buf(),
            avatar_image,
            resume_dir: d_recv.path().join("resume"),
            pin,
        },
        offers_tx,
        events_tx,
    )
    .unwrap();
    let target = (IpAddr::V4(Ipv4Addr::LOCALHOST), handle.local_addr().port());

    // The decision task accepts everything or rejects everything.
    tokio::spawn(async move {
        while let Some(offer) = offers_rx.recv().await {
            let decision = if accept_all {
                OfferDecision::Accept {
                    accepted_files: offer.files.iter().map(|f| f.file_id).collect(),
                    save_dir: None,
                    conflict,
                }
            } else {
                OfferDecision::Reject {
                    reason: Some("test rejection".to_string()),
                }
            };
            let _ = offer.reply.send(decision);
        }
    });

    Harness {
        sender_id,
        receiver_fp: receiver_id.fingerprint.clone(),
        target,
        download_dir: d_down.path().to_path_buf(),
        events: events_rx,
        handle,
        _dirs: (d_send, d_recv, d_down),
    }
}

/// Waits up to 10 seconds for an event matching the predicate.
async fn wait_event(
    events: &mut mpsc::Receiver<TransferEvent>,
    mut pred: impl FnMut(&TransferEvent) -> bool,
) -> TransferEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ev = events
                .recv()
                .await
                .expect("event channel closed unexpectedly");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("timed out waiting for event")
}

/// Full loopback sends a file and nested directory with byte-exact contents and events.
#[tokio::test]
async fn full_roundtrip_files_and_dir() {
    let mut h = harness(true).await;

    // Sources include a multi-chunk 3 MiB file, nested small files, and an empty file.
    let src = TempDir::new();
    let big = src.path().join("big.bin");
    std::fs::write(&big, pattern_data(3 * 1024 * 1024 + 123, 7)).unwrap();
    let dir = src.path().join("bundle");
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("a.txt"), b"hello deskmate").unwrap();
    std::fs::write(dir.join("nested/b.bin"), pattern_data(4096, 42)).unwrap();
    std::fs::write(dir.join("empty.dat"), b"").unwrap();

    let (_tx, control) = watch::channel(ControlState::Running);
    let (events_tx, _keep) = mpsc::channel(256);
    let summary = send_files(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        None,
        &[big.clone(), dir.clone()],
        false,
        // Tests use no ignore rules by default.
        None,
        control,
        events_tx,
    )
    .await
    .unwrap();
    assert_eq!(summary.files_sent, 4);

    // Receiver completion event.
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;

    // Compare each file's contents.
    let check = |rel: &str, src_path: &Path| {
        let got = std::fs::read(h.download_dir.join(rel)).unwrap();
        let want = std::fs::read(src_path).unwrap();
        assert_eq!(
            blake3::hash(&got),
            blake3::hash(&want),
            "content mismatch: {rel}"
        );
    };
    check("big.bin", &big);
    check("bundle/a.txt", &dir.join("a.txt"));
    check("bundle/nested/b.bin", &dir.join("nested/b.bin"));
    check("bundle/empty.dat", &dir.join("empty.dat"));

    // No `.part` files may remain.
    let leftover = walkdir_names(&h.download_dir);
    assert!(
        !leftover.iter().any(|n| n.ends_with(super::PART_SUFFIX)),
        "temporary files remain: {leftover:?}"
    );
}

/// The inline-image marker travels through the manifest to the receiver's
/// FileCompleted event, while regular sends stay unmarked.
#[tokio::test]
async fn inline_image_marker_reaches_receiver() {
    let mut h = harness(true).await;
    let src = TempDir::new();
    let shot = src.path().join("screenshot.png");
    std::fs::write(&shot, pattern_data(2048, 5)).unwrap();

    let (_tx, control) = watch::channel(ControlState::Running);
    let (events_tx, _keep) = mpsc::channel(64);
    send_files(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        None,
        &[shot],
        true,
        // Tests use no ignore rules by default.
        None,
        control,
        events_tx,
    )
    .await
    .unwrap();

    let ev = wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::FileCompleted { .. })
    })
    .await;
    let TransferEvent::FileCompleted {
        path, inline_image, ..
    } = ev
    else {
        unreachable!()
    };
    assert!(inline_image, "receiver must see the inline marker");
    assert!(path.exists(), "the image is still saved as a normal file");
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;
}

/// Peer rejection returns `Rejected` and its reason to the sender.
#[tokio::test]
async fn rejection_propagates_reason() {
    let h = harness(false).await;
    let src = TempDir::new();
    let f = src.path().join("x.bin");
    std::fs::write(&f, b"data").unwrap();

    let (_tx, control) = watch::channel(ControlState::Running);
    let (events_tx, _keep) = mpsc::channel(16);
    let err = send_files(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        None,
        &[f],
        false,
        // Tests use no ignore rules by default.
        None,
        control,
        events_tx,
    )
    .await
    .unwrap_err();
    match err {
        TransferError::Rejected {
            reason,
            reason_code,
        } => {
            assert_eq!(reason.as_deref(), Some("test rejection"));
            // Protocol 1.4 structured rejection can be rendered in the sender's locale.
            assert_eq!(reason_code.as_deref(), Some("declined"));
        }
        other => panic!("expected Rejected, got: {other}"),
    }
}

/// Text transfer preserves surrounding whitespace and control characters byte for byte.
#[tokio::test]
async fn text_is_delivered_verbatim() {
    let mut h = harness(true).await;
    let raw = "  please reply when received\n\tdo not trim me  ";
    let peer = send_text(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        raw,
    )
    .await
    .unwrap();
    assert_eq!(peer.fingerprint, h.receiver_fp);

    let ev = wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::TextReceived { .. })
    })
    .await;
    let TransferEvent::TextReceived { from, text } = ev else {
        unreachable!()
    };
    assert_eq!(text, raw);
    assert_eq!(from.fingerprint, h.sender_id.fingerprint);
}

/// The PIN gate rejects missing or wrong values for files and text and accepts the correct PIN.
#[tokio::test]
async fn pin_gate_blocks_and_admits() {
    let mut h = harness_with(true, ConflictPolicy::default(), None, Some("1234".into())).await;
    let src = TempDir::new();
    let f = src.path().join("x.bin");
    std::fs::write(&f, b"data").unwrap();

    let addrs = [h.target.0];
    let send_with = |pin: Option<&str>| {
        let (_tx, control) = watch::channel(ControlState::Running);
        // Sender events are not asserted; drop the receiver because the sink ignores closure.
        let (events_tx, _rx) = mpsc::channel(64);
        send_files(
            &h.sender_id,
            &addrs,
            h.target.1,
            Some(h.receiver_fp.clone()),
            None,
            pin.map(str::to_string),
            std::slice::from_ref(&f),
            false,
            // Tests use no ignore rules by default.
            None,
            control,
            events_tx,
        )
    };

    // Missing and incorrect PINs are rejected before the decision UI.
    assert!(matches!(
        send_with(None).await,
        Err(TransferError::PinRequired)
    ));
    assert!(matches!(
        send_with(Some("0000")).await,
        Err(TransferError::PinRequired)
    ));

    // The correct PIN completes normal decision and transfer.
    send_with(Some("1234")).await.unwrap();
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;

    // Text uses the same gate: reject the wrong PIN and deliver the correct one.
    let text_err = send_text(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        "hi",
    )
    .await;
    assert!(matches!(text_err, Err(TransferError::PinRequired)));
    send_text(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        Some("1234".into()),
        "hi",
    )
    .await
    .unwrap();
}

/// An incorrect fingerprint pin rejects the send as man-in-the-middle protection.
#[tokio::test]
async fn wrong_fingerprint_is_refused() {
    let h = harness(true).await;
    let src = TempDir::new();
    let f = src.path().join("x.bin");
    std::fs::write(&f, b"data").unwrap();

    let (_tx, control) = watch::channel(ControlState::Running);
    let (events_tx, _keep) = mpsc::channel(16);
    let bogus = "0".repeat(64);
    let result = send_files(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(bogus),
        None,
        None,
        &[f],
        false,
        // Tests use no ignore rules by default.
        None,
        control,
        events_tx,
    )
    .await;
    assert!(result.is_err());
}

/// A second same-name file is renamed instead of overwriting the first.
#[tokio::test]
async fn duplicate_name_gets_suffixed() {
    let mut h = harness(true).await;
    let src = TempDir::new();
    let f = src.path().join("dup.txt");
    std::fs::write(&f, b"round-1").unwrap();

    for round in 1..=2 {
        let (_tx, control) = watch::channel(ControlState::Running);
        let (events_tx, _keep) = mpsc::channel(64);
        std::fs::write(&f, format!("round-{round}")).unwrap();
        send_files(
            &h.sender_id,
            &[h.target.0],
            h.target.1,
            Some(h.receiver_fp.clone()),
            None,
            None,
            std::slice::from_ref(&f),
            false,
            // Tests use no ignore rules by default.
            None,
            control,
            events_tx,
        )
        .await
        .unwrap();
        wait_event(&mut h.events, |e| {
            matches!(e, TransferEvent::Completed { .. })
        })
        .await;
    }

    assert_eq!(
        std::fs::read(h.download_dir.join("dup.txt")).unwrap(),
        b"round-1"
    );
    assert_eq!(
        std::fs::read(h.download_dir.join("dup (1).txt")).unwrap(),
        b"round-2"
    );
}

/// Avatar fetch preserves bytes and hash, while an unset avatar returns `None`.
#[tokio::test]
async fn avatar_fetch_roundtrip() {
    // A receiver with an avatar returns the original bytes.
    let img = pattern_data(9 * 1024 + 5, 77);
    let h = harness_with(true, ConflictPolicy::default(), Some(img.clone()), None).await;
    let got = fetch_avatar(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
    )
    .await
    .unwrap()
    .expect("expected an avatar");
    assert_eq!(got.0, blake3::hash(&img).to_hex().to_string());
    assert_eq!(got.1, img);

    // A receiver without an avatar returns None.
    let h2 = harness(true).await;
    let none = fetch_avatar(
        &h2.sender_id,
        &[h2.target.0],
        h2.target.1,
        Some(h2.receiver_fp.clone()),
    )
    .await
    .unwrap();
    assert!(none.is_none());
}

/// Resume after interruption sends only missing ranges and restores byte-exact content.
#[tokio::test]
async fn resume_after_interrupt() {
    use rustls_pki_types::ServerName;
    use tokio::io::AsyncWriteExt;
    use tokio_rustls::TlsConnector;

    use crate::PROTOCOL_VERSION;
    use crate::protocol::{ControlMessage, FileMeta, read_frame, write_frame};
    use crate::tls::client_config;

    /// Test-only direct TLS connection because production `connect_tls` is private.
    async fn tls_connect(
        cfg: &Arc<rustls::ClientConfig>,
        target: (IpAddr, u16),
    ) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
        let tcp = tokio::net::TcpStream::connect(target).await.unwrap();
        TlsConnector::from(Arc::clone(cfg))
            .connect(ServerName::try_from("deskmate").unwrap(), tcp)
            .await
            .unwrap()
    }

    let mut h = harness(true).await;
    let src = TempDir::new();
    let path = src.path().join("resume.bin");
    let data = pattern_data(3 * 1024 * 1024 + 777, 99);
    std::fs::write(&path, &data).unwrap();
    let transfer_id = "resume-test-0001".to_string();

    // Phase one uses a manual protocol client that disconnects after half the bytes.
    {
        let config = Arc::new(client_config(&h.sender_id, Some(h.receiver_fp.clone())).unwrap());

        // Control session: handshake, request, and acceptance.
        let mut ctrl = tls_connect(&config, h.target).await;
        write_frame(
            &mut ctrl,
            &ControlMessage::Hello {
                version: PROTOCOL_VERSION.to_string(),
                info: h.sender_id.peer_info(),
            },
        )
        .await
        .unwrap();
        read_frame(&mut ctrl).await.unwrap();
        write_frame(
            &mut ctrl,
            &ControlMessage::TransferRequest {
                transfer_id: transfer_id.clone(),
                files: vec![FileMeta {
                    file_id: 0,
                    rel_path: "resume.bin".to_string(),
                    size: data.len() as u64,
                    inline_image: false,
                }],
                total_size: data.len() as u64,
                pin: None,
            },
        )
        .await
        .unwrap();
        let resp = read_frame(&mut ctrl).await.unwrap();
        assert!(matches!(
            resp,
            ControlMessage::TransferResponse { ref accepted_files, .. } if !accepted_files.is_empty()
        ));

        // Data session: drop after half so the receiver reports Interrupted and keeps `.part`.
        let mut data_conn = tls_connect(&config, h.target).await;
        write_frame(
            &mut data_conn,
            &ControlMessage::DataHello {
                transfer_id: transfer_id.clone(),
            },
        )
        .await
        .unwrap();
        write_frame(
            &mut data_conn,
            &ControlMessage::FileHeader {
                file_id: 0,
                offset: 0,
            },
        )
        .await
        .unwrap();
        data_conn.write_all(&data[..data.len() / 2]).await.unwrap();
        data_conn.flush().await.unwrap();
    }
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Interrupted { .. })
    })
    .await;

    // Phase two uses `resume_send` to negotiate and send only the missing range.
    let (_tx, control) = watch::channel(ControlState::Running);
    let (events_tx, _keep) = mpsc::channel(64);
    let summary = resume_send(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        &transfer_id,
        std::slice::from_ref(&path),
        // Tests use no ignore rules by default.
        None,
        control,
        events_tx,
    )
    .await
    .unwrap();
    assert_eq!(summary.files_sent, 1);
    // Resumed bytes must be less than the full file.
    assert!(summary.bytes_sent < data.len() as u64);

    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;
    assert_eq!(
        std::fs::read(h.download_dir.join("resume.bin")).unwrap(),
        data
    );
    // Resume appends to the original `.part` without creating a `(1)` copy.
    assert!(!h.download_dir.join("resume (1).bin").exists());
}

/// Overwrite policy replaces a same-name file without creating a `(1)` copy.
#[tokio::test]
async fn overwrite_replaces_existing() {
    let mut h = harness_with(true, ConflictPolicy::Overwrite, None, None).await;
    let src = TempDir::new();
    let f = src.path().join("dup.txt");

    for round in 1..=2 {
        let (_tx, control) = watch::channel(ControlState::Running);
        let (events_tx, _keep) = mpsc::channel(64);
        std::fs::write(&f, format!("round-{round}")).unwrap();
        send_files(
            &h.sender_id,
            &[h.target.0],
            h.target.1,
            Some(h.receiver_fp.clone()),
            None,
            None,
            std::slice::from_ref(&f),
            false,
            // Tests use no ignore rules by default.
            None,
            control,
            events_tx,
        )
        .await
        .unwrap();
        wait_event(&mut h.events, |e| {
            matches!(e, TransferEvent::Completed { .. })
        })
        .await;
    }

    assert_eq!(
        std::fs::read(h.download_dir.join("dup.txt")).unwrap(),
        b"round-2"
    );
    assert!(!h.download_dir.join("dup (1).txt").exists());
}

/// Lists all filenames recursively for tests.
fn walkdir_names(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(walkdir_names(&p));
            } else {
                out.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    out
}

/// Path sanitization rejects traversal and accepts normal relative paths.
#[test]
fn sanitize_blocks_traversal() {
    assert!(sanitize_rel_path("../evil.sh").is_err());
    assert!(sanitize_rel_path("a/../../evil").is_err());
    assert!(sanitize_rel_path("/etc/passwd").is_err());
    assert!(sanitize_rel_path("..\\win\\style").is_err());
    assert!(sanitize_rel_path("").is_err());
    assert_eq!(
        sanitize_rel_path("a/b/c.txt").unwrap(),
        PathBuf::from("a/b/c.txt")
    );
    assert_eq!(sanitize_rel_path("./a/./b").unwrap(), PathBuf::from("a/b"));
    assert_eq!(
        sanitize_rel_path(".hidden").unwrap(),
        PathBuf::from(".hidden")
    );
}

/// Windows sanitization handles invalid characters, reserved names, and trailing dots/spaces.
#[test]
fn sanitize_windows_rules() {
    // Replace NTFS alternate-data-stream colons; macOS and Linux retain them.
    assert_eq!(
        sanitize_rel_path_for("report:final.pdf", true).unwrap(),
        PathBuf::from("report_final.pdf")
    );
    assert_eq!(
        sanitize_rel_path_for("report:final.pdf", false).unwrap(),
        PathBuf::from("report:final.pdf")
    );
    // Other invalid and control characters.
    assert_eq!(
        sanitize_rel_path_for("a?b*c<d>e\"f|g.txt", true).unwrap(),
        PathBuf::from("a_b_c_d_e_f_g.txt")
    );
    // Reserved device names, including lowercase and extension forms.
    assert_eq!(
        sanitize_rel_path_for("CON.txt", true).unwrap(),
        PathBuf::from("_CON.txt")
    );
    assert_eq!(
        sanitize_rel_path_for("dir/nul", true).unwrap(),
        PathBuf::from("dir/_nul")
    );
    assert_eq!(
        sanitize_rel_path_for("lpt9.log", true).unwrap(),
        PathBuf::from("_lpt9.log")
    );
    // Remove trailing dots and spaces; names made only of dots fall back to `_`.
    assert_eq!(
        sanitize_rel_path_for("dir./file. ", true).unwrap(),
        PathBuf::from("dir/file")
    );
    assert_eq!(
        sanitize_rel_path_for("a/...", true).unwrap(),
        PathBuf::from("a/_")
    );
    // Similar non-reserved names remain unchanged, and traversal stays blocked.
    assert_eq!(
        sanitize_rel_path_for("CONS.txt", true).unwrap(),
        PathBuf::from("CONS.txt")
    );
    assert!(sanitize_rel_path_for("../evil", true).is_err());
}

/// `set_self_info` immediately updates handshake and avatar responses for new sessions.
#[tokio::test]
async fn identity_hot_update() {
    let h = harness(true).await;
    let addrs = [h.target.0];

    // Before the update, the handshake returns the original name and no avatar.
    let before = send_text(
        &h.sender_id,
        &addrs,
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        "hi",
    )
    .await
    .unwrap();
    let avatar_before = fetch_avatar(
        &h.sender_id,
        &addrs,
        h.target.1,
        Some(h.receiver_fp.clone()),
    )
    .await
    .unwrap();
    assert!(avatar_before.is_none());

    // Update name and avatar without restarting the receiver.
    let mut new_info = before.clone();
    new_info.name = "Renamed device".to_string();
    let img = b"fake-avatar-bytes".to_vec();
    new_info.avatar = Some(format!("img:{}", blake3::hash(&img).to_hex()));
    h.handle.set_self_info(new_info, Some(img.clone()));

    // New sessions receive the updated name and matching avatar hash and bytes.
    let after = send_text(
        &h.sender_id,
        &addrs,
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        "again",
    )
    .await
    .unwrap();
    assert_eq!(after.name, "Renamed device");
    assert_ne!(after.name, before.name);
    let (hash, data) = fetch_avatar(
        &h.sender_id,
        &addrs,
        h.target.1,
        Some(h.receiver_fp.clone()),
    )
    .await
    .unwrap()
    .expect("avatar should be available after the update");
    assert_eq!(data, img);
    assert_eq!(hash, blake3::hash(&img).to_hex().to_string());
}

/// PIN lockout is isolated per source so one device cannot block another.
#[tokio::test]
async fn pin_lockout_is_per_peer() {
    let h = harness_with(true, ConflictPolicy::default(), None, Some("9999".into())).await;
    let addrs = [h.target.0];
    let text_as = |id: &Arc<DeviceIdentity>, pin: Option<&str>| {
        let id = Arc::clone(id);
        let fp = h.receiver_fp.clone();
        let pin = pin.map(str::to_string);
        async move { send_text(&id, &addrs, h.target.1, Some(fp), pin, "hi").await }
    };

    // Source A fails until reaching the lockout threshold.
    for _ in 0..crate::config::PIN_MAX_FAILURES {
        assert!(matches!(
            text_as(&h.sender_id, Some("0000")).await,
            Err(TransferError::PinRequired)
        ));
    }
    // A remains rejected for the window even with the correct PIN.
    assert!(matches!(
        text_as(&h.sender_id, Some("9999")).await,
        Err(TransferError::PinRequired)
    ));

    // Source B has another identity and succeeds with the correct PIN.
    let d_b = TempDir::new();
    let sender_b = Arc::new(DeviceIdentity::load_or_create(d_b.path()).unwrap());
    text_as(&sender_b, Some("9999")).await.unwrap();
}

/// Concurrent same-name transfers preserve both complete files through renaming.
#[tokio::test]
async fn concurrent_same_name_transfers_do_not_clobber() {
    let mut h = harness(true).await;
    let d_b = TempDir::new();
    let sender_b = Arc::new(DeviceIdentity::load_or_create(d_b.path()).unwrap());

    // Use different contents and sizes so interruption or truncation is obvious.
    let (src_a, src_b) = (TempDir::new(), TempDir::new());
    let (fa, fb) = (
        src_a.path().join("clash.bin"),
        src_b.path().join("clash.bin"),
    );
    let data_a = pattern_data(2 * 1024 * 1024 + 11, 3);
    let data_b = pattern_data(1024 * 1024 + 77, 5);
    std::fs::write(&fa, &data_a).unwrap();
    std::fs::write(&fb, &data_b).unwrap();

    let send_as = |id: Arc<DeviceIdentity>, path: PathBuf| {
        let fp = h.receiver_fp.clone();
        let target = h.target;
        async move {
            let (_tx, control) = watch::channel(ControlState::Running);
            let (events_tx, _keep) = mpsc::channel(64);
            send_files(
                &id,
                &[target.0],
                target.1,
                Some(fp),
                None,
                None,
                std::slice::from_ref(&path),
                false,
                // Tests use no ignore rules by default.
                None,
                control,
                events_tx,
            )
            .await
        }
    };
    let (ra, rb) = tokio::join!(
        send_as(Arc::clone(&h.sender_id), fa),
        send_as(Arc::clone(&sender_b), fb)
    );
    ra.unwrap();
    rb.unwrap();
    for _ in 0..2 {
        wait_event(&mut h.events, |e| {
            matches!(e, TransferEvent::Completed { .. })
        })
        .await;
    }

    // Both complete contents exist; arrival order determines which keeps the base name.
    let got: Vec<Vec<u8>> = ["clash.bin", "clash (1).bin"]
        .iter()
        .map(|n| std::fs::read(h.download_dir.join(n)).unwrap())
        .collect();
    assert!(
        got.contains(&data_a),
        "clash.bin content A is missing or corrupted"
    );
    assert!(
        got.contains(&data_b),
        "clash.bin content B is missing or corrupted"
    );
    let leftover = walkdir_names(&h.download_dir);
    assert!(
        !leftover.iter().any(|n| n.ends_with(super::PART_SUFFIX)),
        "temporary files remain: {leftover:?}"
    );
}

/// Cancellation interrupts a blocked receive when the peer stops sending.
#[tokio::test]
async fn cancel_interrupts_stalled_receive() {
    use rustls_pki_types::ServerName;
    use tokio::io::AsyncWriteExt;
    use tokio_rustls::TlsConnector;

    use crate::PROTOCOL_VERSION;
    use crate::protocol::{ControlMessage, FileMeta, read_frame, write_frame};
    use crate::tls::client_config;

    let mut h = harness(true).await;
    let transfer_id = "stall-test-0001".to_string();
    let size = 4 * 1024 * 1024u64;

    // Manual client declares 4 MiB, sends 1 KiB, then stalls to simulate abuse.
    let config = Arc::new(client_config(&h.sender_id, Some(h.receiver_fp.clone())).unwrap());
    let tls_connect = |cfg: Arc<rustls::ClientConfig>| async move {
        let tcp = tokio::net::TcpStream::connect(h.target).await.unwrap();
        TlsConnector::from(cfg)
            .connect(ServerName::try_from("deskmate").unwrap(), tcp)
            .await
            .unwrap()
    };

    let mut ctrl = tls_connect(Arc::clone(&config)).await;
    write_frame(
        &mut ctrl,
        &ControlMessage::Hello {
            version: PROTOCOL_VERSION.to_string(),
            info: h.sender_id.peer_info(),
        },
    )
    .await
    .unwrap();
    read_frame(&mut ctrl).await.unwrap();
    write_frame(
        &mut ctrl,
        &ControlMessage::TransferRequest {
            transfer_id: transfer_id.clone(),
            files: vec![FileMeta {
                file_id: 0,
                rel_path: "stall.bin".to_string(),
                size,
                inline_image: false,
            }],
            total_size: size,
            pin: None,
        },
    )
    .await
    .unwrap();
    read_frame(&mut ctrl).await.unwrap();

    let mut data_conn = tls_connect(Arc::clone(&config)).await;
    write_frame(
        &mut data_conn,
        &ControlMessage::DataHello {
            transfer_id: transfer_id.clone(),
        },
    )
    .await
    .unwrap();
    write_frame(
        &mut data_conn,
        &ControlMessage::FileHeader {
            file_id: 0,
            offset: 0,
        },
    )
    .await
    .unwrap();
    data_conn.write_all(&pattern_data(1024, 1)).await.unwrap();
    data_conn.flush().await.unwrap();

    // Cancel after the receiver blocks in chunk read. The event must arrive within
    // wait_event's 10 seconds; previously read did not race control signals.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(h.handle.cancel(&transfer_id));
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Cancelled { .. })
    })
    .await;
    drop(data_conn);
    drop(ctrl);
}

/// Conflict renaming appends `(1)` and `(2)`, including names without extensions.
#[test]
fn dedup_path_appends_counter() {
    let dir = TempDir::new();
    let base = dir.path().join("f.txt");
    assert_eq!(dedup_path(&base), base);
    std::fs::write(&base, b"x").unwrap();
    assert_eq!(dedup_path(&base), dir.path().join("f (1).txt"));
    std::fs::write(dir.path().join("f (1).txt"), b"x").unwrap();
    assert_eq!(dedup_path(&base), dir.path().join("f (2).txt"));

    let noext = dir.path().join("bare");
    std::fs::write(&noext, b"x").unwrap();
    assert_eq!(dedup_path(&noext), dir.path().join("bare (1)"));
}

/// Sender pause and resume travel over control so the receiver emits visible events.
#[tokio::test]
async fn sender_pause_is_visible_to_receiver() {
    let mut h = harness(true).await;
    let src = TempDir::new();
    let file = src.path().join("pausable.bin");
    std::fs::write(&file, pattern_data(4 * 1024 * 1024, 7)).unwrap();

    // Start paused so the data pump stops before its first chunk without timing races.
    let (ctrl_tx, ctrl_rx) = watch::channel(ControlState::Paused);
    let (ev_tx, _keep) = mpsc::channel(256);
    let sender_id = Arc::clone(&h.sender_id);
    let (fp, target) = (h.receiver_fp.clone(), h.target);
    let send_task = tokio::spawn(async move {
        send_files(
            &sender_id,
            &[target.0],
            target.1,
            Some(fp),
            None,
            None,
            &[file],
            false,
            // Tests use no ignore rules by default.
            None,
            ctrl_rx,
            ev_tx,
        )
        .await
    });

    // The receiver sees pause, then resume, and the transfer completes normally.
    wait_event(&mut h.events, |e| matches!(e, TransferEvent::Paused { .. })).await;
    ctrl_tx.send(ControlState::Running).unwrap();
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Resumed { .. })
    })
    .await;
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;
    let summary = send_task.await.unwrap().unwrap();
    assert_eq!(summary.files_sent, 1);
}

/// Receiver pause and resume are pushed to the sender, which emits corresponding events.
#[tokio::test]
async fn receiver_pause_notifies_sender() {
    let h = harness(true).await;
    let src = TempDir::new();
    let file = src.path().join("recv-pause.bin");
    std::fs::write(&file, pattern_data(32 * 1024 * 1024, 11)).unwrap();

    let tid = uuid::Uuid::new_v4().to_string();
    let (_ctrl_tx, ctrl_rx) = watch::channel(ControlState::Running);
    let (ev_tx, mut sender_events) = mpsc::channel(256);
    let sender_id = Arc::clone(&h.sender_id);
    let (fp, target, tid_arg) = (h.receiver_fp.clone(), h.target, Some(tid.clone()));
    let send_task = tokio::spawn(async move {
        send_files(
            &sender_id,
            &[target.0],
            target.1,
            Some(fp),
            tid_arg,
            None,
            &[file],
            false,
            // Tests use no ignore rules by default.
            None,
            ctrl_rx,
            ev_tx,
        )
        .await
    });

    // Pause after data starts and the task is active in the registry.
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Progress { .. })
    })
    .await;
    assert!(
        h.handle.pause(&tid),
        "receiver pause failed because task was not found"
    );
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Paused { .. })
    })
    .await;
    assert!(h.handle.resume(&tid));
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Resumed { .. })
    })
    .await;
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;
    send_task.await.unwrap().unwrap();
}

/// Receiver cancellation settles the sender as `Cancelled`, not `Interrupted`.
#[tokio::test]
async fn receiver_cancel_settles_sender_as_cancelled() {
    let mut h = harness(true).await;
    let src = TempDir::new();
    let file = src.path().join("cancelme.bin");
    std::fs::write(&file, pattern_data(32 * 1024 * 1024, 13)).unwrap();

    let tid = uuid::Uuid::new_v4().to_string();
    let (_ctrl_tx, ctrl_rx) = watch::channel(ControlState::Running);
    let (ev_tx, mut sender_events) = mpsc::channel(256);
    let sender_id = Arc::clone(&h.sender_id);
    let (fp, target, tid_arg) = (h.receiver_fp.clone(), h.target, Some(tid.clone()));
    let send_task = tokio::spawn(async move {
        send_files(
            &sender_id,
            &[target.0],
            target.1,
            Some(fp),
            tid_arg,
            None,
            &[file],
            false,
            // Tests use no ignore rules by default.
            None,
            ctrl_rx,
            ev_tx,
        )
        .await
    });

    // Pause both pumps before cancellation so timing does not depend on transfer speed.
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Progress { .. })
    })
    .await;
    assert!(h.handle.pause(&tid));
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Paused { .. })
    })
    .await;
    assert!(h.handle.cancel(&tid));

    // Both endpoints settle as Cancelled rather than treating closure as interruption.
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Cancelled { .. })
    })
    .await;
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Cancelled { .. })
    })
    .await;
    assert!(matches!(
        send_task.await.unwrap(),
        Err(TransferError::Cancelled)
    ));

    // Explicit cancellation leaves no `.part` files.
    let leftover = walkdir_names(&h.download_dir);
    assert!(
        !leftover.iter().any(|n| n.ends_with(super::PART_SUFFIX)),
        "temporary files remain after cancellation: {leftover:?}"
    );
}

/// Ignore collection supports recursive globs, directory pruning, negation, and top-level filtering.
#[test]
fn ignore_rules_filter_collection() {
    let src = TempDir::new();
    let root = src.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
    std::fs::write(root.join("debug.log"), b"log").unwrap();
    std::fs::write(root.join("keep.log"), b"keep").unwrap();
    std::fs::write(root.join("node_modules/pkg/index.js"), b"js").unwrap();
    // Directly selected top-level files also pass through ignore rules.
    let top_log = src.path().join("top.log");
    std::fs::write(&top_log, b"top").unwrap();

    let rules = IgnoreRules::parse("*.log\nnode_modules/\n!keep.log").unwrap();
    let got = collect_files(&[root.clone(), top_log], Some(&rules)).unwrap();
    let mut rels: Vec<&str> = got.iter().map(|(_, rel, _)| rel.as_str()).collect();
    rels.sort_unstable();
    // The glob removes debug.log and top.log, node_modules is pruned, and negation keeps keep.log.
    assert_eq!(rels, ["proj/keep.log", "proj/src/main.rs"]);

    // Without rules, collect every file including those filtered above.
    let all = collect_files(&[root], None).unwrap();
    assert_eq!(all.len(), 4);
}

/// Fully filtered selections return `NoValidFiles` instead of an empty transfer.
#[test]
fn ignore_rules_all_filtered_is_error() {
    let src = TempDir::new();
    let f = src.path().join("secret.env");
    std::fs::write(&f, b"KEY=1").unwrap();
    let rules = IgnoreRules::parse("*.env").unwrap();
    assert!(matches!(
        collect_files(&[f], Some(&rules)),
        Err(TransferError::NoValidFiles)
    ));
}

/// End-to-end ignored directory transfer persists only unfiltered files.
#[tokio::test]
async fn ignore_rules_apply_end_to_end() {
    let mut h = harness(true).await;
    let src = TempDir::new();
    let dir = src.path().join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.jpg"), pattern_data(2048, 3)).unwrap();
    std::fs::write(dir.join(".DS_Store"), b"junk").unwrap();
    std::fs::write(dir.join("thumbs.db"), b"junk").unwrap();

    let rules = IgnoreRules::parse(".DS_Store\nthumbs.db").unwrap();
    let (_tx, control) = watch::channel(ControlState::Running);
    let (events_tx, _keep) = mpsc::channel(64);
    let summary = send_files(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        None,
        None,
        &[dir],
        false,
        Some(&rules),
        control,
        events_tx,
    )
    .await
    .unwrap();
    assert_eq!(summary.files_sent, 1);

    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;
    let names = walkdir_names(&h.download_dir);
    assert!(names.contains(&"a.jpg".to_string()));
    assert!(
        !names.iter().any(|n| n == ".DS_Store" || n == "thumbs.db"),
        "ignored files should not be persisted: {names:?}"
    );
}

/// Matching the selected top-level directory itself returns `NoValidFiles`.
#[test]
fn ignore_rules_top_level_dir_hit_is_error() {
    let src = TempDir::new();
    let dir = src.path().join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.jpg"), b"x").unwrap();
    for rule in ["photos/", "photos", "photo*", "/photos"] {
        let rules = IgnoreRules::parse(rule).unwrap();
        assert!(
            matches!(
                collect_files(std::slice::from_ref(&dir), Some(&rules)),
                Err(TransferError::NoValidFiles)
            ),
            "rule {rule} should filter the top-level directory and return NoValidFiles"
        );
    }
}
