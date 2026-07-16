//! 传输引擎端到端测试: localhost 回环上跑完整收发流程

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

use crate::identity::DeviceIdentity;
use crate::transfer::{
    ConflictPolicy, ControlState, OfferDecision, ReceiverOptions, TransferError, TransferEvent,
    dedup_path, fetch_avatar, resume_send, sanitize_rel_path, sanitize_rel_path_for, send_files,
    send_text, spawn_receiver,
};

/// 独立临时目录, Drop 时自动清理
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

/// 生成确定性测试数据(无需随机源)
fn pattern_data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// 测试环境: 双身份 + 已启动的接收服务
struct Harness {
    sender_id: Arc<DeviceIdentity>,
    receiver_fp: String,
    target: (IpAddr, u16),
    download_dir: PathBuf,
    events: mpsc::Receiver<TransferEvent>,
    handle: crate::transfer::ReceiverHandle,
    _dirs: (TempDir, TempDir, TempDir),
}

/// 搭建回环收发环境; `accept_all` 决定接收方自动接受还是拒绝
async fn harness(accept_all: bool) -> Harness {
    harness_with(accept_all, ConflictPolicy::default(), None, None).await
}

/// 同 [`harness`], 可指定接收方的同名冲突策略、头像图片与配对 PIN
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

    // 决策方: 自动接受全部或一律拒绝
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
                    reason: Some("测试拒绝".to_string()),
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

/// 从事件流等待指定谓词命中(10s 超时)
async fn wait_event(
    events: &mut mpsc::Receiver<TransferEvent>,
    mut pred: impl FnMut(&TransferEvent) -> bool,
) -> TransferEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ev = events.recv().await.expect("事件通道意外关闭");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("等待事件超时")
}

/// 完整回环: 单文件 + 目录(含子目录), 内容逐字节一致, 双端事件齐全
#[tokio::test]
async fn full_roundtrip_files_and_dir() {
    let mut h = harness(true).await;

    // 数据源: 3MiB 大文件(跨多个 chunk)+ 目录内嵌套小文件 + 空文件
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
        control,
        events_tx,
    )
    .await
    .unwrap();
    assert_eq!(summary.files_sent, 4);

    // 接收端 Completed 事件
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;

    // 逐文件比对内容
    let check = |rel: &str, src_path: &Path| {
        let got = std::fs::read(h.download_dir.join(rel)).unwrap();
        let want = std::fs::read(src_path).unwrap();
        assert_eq!(blake3::hash(&got), blake3::hash(&want), "内容不一致: {rel}");
    };
    check("big.bin", &big);
    check("bundle/a.txt", &dir.join("a.txt"));
    check("bundle/nested/b.bin", &dir.join("nested/b.bin"));
    check("bundle/empty.dat", &dir.join("empty.dat"));

    // 不允许残留 .part 临时文件
    let leftover = walkdir_names(&h.download_dir);
    assert!(
        !leftover.iter().any(|n| n.ends_with(super::PART_SUFFIX)),
        "残留临时文件: {leftover:?}"
    );
}

/// 对端拒绝时, 发送方得到 Rejected 错误与原因
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
        control,
        events_tx,
    )
    .await
    .unwrap_err();
    match err {
        TransferError::Rejected { reason } => assert_eq!(reason.as_deref(), Some("测试拒绝")),
        other => panic!("期望 Rejected, 得到: {other}"),
    }
}

/// 文本传输逐字节一致(含首尾空白与控制字符)
#[tokio::test]
async fn text_is_delivered_verbatim() {
    let mut h = harness(true).await;
    let raw = "  收到请回复\n\t不要 trim 我  ";
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

/// PIN 门禁: 缺失/错误一律 PinRequired(文件与文本同门), 正确 PIN 放行
#[tokio::test]
async fn pin_gate_blocks_and_admits() {
    let mut h = harness_with(true, ConflictPolicy::default(), None, Some("1234".into())).await;
    let src = TempDir::new();
    let f = src.path().join("x.bin");
    std::fs::write(&f, b"data").unwrap();

    let addrs = [h.target.0];
    let send_with = |pin: Option<&str>| {
        let (_tx, control) = watch::channel(ControlState::Running);
        // 发送侧事件不参与断言, 通道接收端直接丢弃(sink 对关闭通道静默忽略)
        let (events_tx, _rx) = mpsc::channel(64);
        send_files(
            &h.sender_id,
            &addrs,
            h.target.1,
            Some(h.receiver_fp.clone()),
            None,
            pin.map(str::to_string),
            std::slice::from_ref(&f),
            control,
            events_tx,
        )
    };

    // 不带 PIN / 错 PIN: 门口即拒, 不弹决策
    assert!(matches!(
        send_with(None).await,
        Err(TransferError::PinRequired)
    ));
    assert!(matches!(
        send_with(Some("0000")).await,
        Err(TransferError::PinRequired)
    ));

    // 正确 PIN: 正常走完决策与传输
    send_with(Some("1234")).await.unwrap();
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;

    // 文本同门: 错误拒绝, 正确送达
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

/// 指纹 pin 错误时发送必须失败(中间人防护)
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
        control,
        events_tx,
    )
    .await;
    assert!(result.is_err());
}

/// 同名文件二次接收自动重命名, 不覆盖已有文件
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

/// 头像拉取: 字节逐一一致且哈希匹配; 未设置头像返回 None
#[tokio::test]
async fn avatar_fetch_roundtrip() {
    // 有头像的接收方: 拉到的数据与源一致
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
    .expect("应拉取到头像");
    assert_eq!(got.0, blake3::hash(&img).to_hex().to_string());
    assert_eq!(got.1, img);

    // 未设置头像的接收方: 返回 None
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

/// 意外断连后续传: 半途夭折的传输凭断点元数据补发缺失段, 最终逐字节一致
#[tokio::test]
async fn resume_after_interrupt() {
    use rustls_pki_types::ServerName;
    use tokio::io::AsyncWriteExt;
    use tokio_rustls::TlsConnector;

    use crate::PROTOCOL_VERSION;
    use crate::protocol::{ControlMessage, FileMeta, read_frame, write_frame};
    use crate::tls::client_config;

    /// 测试专用 TLS 直连(生产路径的 connect_tls 未公开)
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

    // 第一阶段(手写协议客户端): 发一半字节后直接断开, 模拟网络意外中断
    {
        let config = Arc::new(client_config(&h.sender_id, Some(h.receiver_fp.clone())).unwrap());

        // 控制会话: 握手 + 请求 + 等待接受
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

        // 数据会话: 发一半后 drop 连接(接收端读到 EOF → Interrupted, .part 保留)
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

    // 第二阶段: 标准 resume_send 协商断点后仅补发缺失段
    let (_tx, control) = watch::channel(ControlState::Running);
    let (events_tx, _keep) = mpsc::channel(64);
    let summary = resume_send(
        &h.sender_id,
        &[h.target.0],
        h.target.1,
        Some(h.receiver_fp.clone()),
        &transfer_id,
        std::slice::from_ref(&path),
        control,
        events_tx,
    )
    .await
    .unwrap();
    assert_eq!(summary.files_sent, 1);
    // 补发字节数应少于全量(只发缺失段)
    assert!(summary.bytes_sent < data.len() as u64);

    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Completed { .. })
    })
    .await;
    assert_eq!(
        std::fs::read(h.download_dir.join("resume.bin")).unwrap(),
        data
    );
    // 续传直接续写原 .part, 不产生 (1) 副本
    assert!(!h.download_dir.join("resume (1).bin").exists());
}

/// Overwrite 策略: 二次接收同名文件直接覆盖, 不生成 (1) 副本
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

/// 列出目录下所有文件名(含子目录, 仅测试用)
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

/// 路径穿越防护: 越权路径一律拒绝, 正常相对路径放行
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

/// Windows 落盘净化: 非法字符/保留名/尾部点空格逐段处理, 其他平台原样保留
#[test]
fn sanitize_windows_rules() {
    // `:` 在 NTFS 是备用数据流分隔符, 必须替换; mac/Linux 接收端保持原名
    assert_eq!(
        sanitize_rel_path_for("报告:终版.pdf", true).unwrap(),
        PathBuf::from("报告_终版.pdf")
    );
    assert_eq!(
        sanitize_rel_path_for("报告:终版.pdf", false).unwrap(),
        PathBuf::from("报告:终版.pdf")
    );
    // 其余非法字符与控制字符
    assert_eq!(
        sanitize_rel_path_for("a?b*c<d>e\"f|g.txt", true).unwrap(),
        PathBuf::from("a_b_c_d_e_f_g.txt")
    );
    // 保留设备名(含带扩展名与小写形式)
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
    // 尾部点与空格被 Windows 静默剥离, 主动去除; 全点名兜底为 `_`
    assert_eq!(
        sanitize_rel_path_for("dir./file. ", true).unwrap(),
        PathBuf::from("dir/file")
    );
    assert_eq!(
        sanitize_rel_path_for("a/...", true).unwrap(),
        PathBuf::from("a/_")
    );
    // 类似保留名但不等(CONS)不受影响; 穿越防护不因净化放松
    assert_eq!(
        sanitize_rel_path_for("CONS.txt", true).unwrap(),
        PathBuf::from("CONS.txt")
    );
    assert!(sanitize_rel_path_for("../evil", true).is_err());
}

/// 身份热更新: set_self_info 后新会话的握手应答与头像应答立即使用新值
#[tokio::test]
async fn identity_hot_update() {
    let h = harness(true).await;
    let addrs = [h.target.0];

    // 变更前: 握手返回原始昵称, 无头像
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

    // 热更新昵称与头像(不重启接收服务)
    let mut new_info = before.clone();
    new_info.name = "改名后的设备".to_string();
    let img = b"fake-avatar-bytes".to_vec();
    new_info.avatar = Some(format!("img:{}", blake3::hash(&img).to_hex()));
    h.handle.set_self_info(new_info, Some(img.clone()));

    // 变更后: 新会话的 HelloAck 带新昵称, AvatarRequest 应答新图(哈希一致)
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
    assert_eq!(after.name, "改名后的设备");
    assert_ne!(after.name, before.name);
    let (hash, data) = fetch_avatar(
        &h.sender_id,
        &addrs,
        h.target.1,
        Some(h.receiver_fp.clone()),
    )
    .await
    .unwrap()
    .expect("热更新后应返回头像");
    assert_eq!(data, img);
    assert_eq!(hash, blake3::hash(&img).to_hex().to_string());
}

/// PIN 锁定按来源隔离: 一台设备乱试锁死自己, 不影响他人正常配对
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

    // 来源 A 连续失败到锁定阈值
    for _ in 0..crate::config::PIN_MAX_FAILURES {
        assert!(matches!(
            text_as(&h.sender_id, Some("0000")).await,
            Err(TransferError::PinRequired)
        ));
    }
    // A 已锁定: 窗口内即便 PIN 正确也拒(防在线试探)
    assert!(matches!(
        text_as(&h.sender_id, Some("9999")).await,
        Err(TransferError::PinRequired)
    ));

    // 来源 B(另一身份)不受 A 连累, 正确 PIN 直接通过
    let d_b = TempDir::new();
    let sender_b = Arc::new(DeviceIdentity::load_or_create(d_b.path()).unwrap());
    text_as(&sender_b, Some("9999")).await.unwrap();
}

/// 两个发送方并发发同名文件: 互不截断, 各自完整落盘(自动重命名)
#[tokio::test]
async fn concurrent_same_name_transfers_do_not_clobber() {
    let mut h = harness(true).await;
    let d_b = TempDir::new();
    let sender_b = Arc::new(DeviceIdentity::load_or_create(d_b.path()).unwrap());

    // 同名不同内容(大小也错开, 中断/截断更易暴露)
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

    // 两份内容都完整存在(先到者占原名, 后到者重命名, 顺序不定)
    let got: Vec<Vec<u8>> = ["clash.bin", "clash (1).bin"]
        .iter()
        .map(|n| std::fs::read(h.download_dir.join(n)).unwrap())
        .collect();
    assert!(got.contains(&data_a), "clash.bin 的 A 内容缺失或损坏");
    assert!(got.contains(&data_b), "clash.bin 的 B 内容缺失或损坏");
    let leftover = walkdir_names(&h.download_dir);
    assert!(
        !leftover.iter().any(|n| n.ends_with(super::PART_SUFFIX)),
        "残留临时文件: {leftover:?}"
    );
}

/// 取消能打断阻塞中的接收: 对端停发时 read 卡住, 取消不再静默失效
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

    // 手写协议客户端: 声明 4MiB 只发 1KiB, 连接保持不动(模拟停发/恶意占用)
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

    // 等接收端进入 chunk read 阻塞后取消; 事件须在 wait_event 的 10s 内到达
    // (改造前 read 不与控制信号竞速, 取消静默失效, 此处会超时失败)
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(h.handle.cancel(&transfer_id));
    wait_event(&mut h.events, |e| {
        matches!(e, TransferEvent::Cancelled { .. })
    })
    .await;
    drop(data_conn);
    drop(ctrl);
}

/// 冲突重命名: 依次生成 (1)、(2) 后缀, 无扩展名亦可
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

/// 发送端本地暂停/恢复经控制连接转告接收端, 接收端上报 Paused/Resumed
/// 事件驱动 UI(改造前对端暂停完全不可见, 界面停在"传输中")
#[tokio::test]
async fn sender_pause_is_visible_to_receiver() {
    let mut h = harness(true).await;
    let src = TempDir::new();
    let file = src.path().join("pausable.bin");
    std::fs::write(&file, pattern_data(4 * 1024 * 1024, 7)).unwrap();

    // 发送前即置 Paused: 数据泵在首个 chunk 前挂起, 暂停窗口稳定无竞态
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
            ctrl_rx,
            ev_tx,
        )
        .await
    });

    // 接收端先看到"对端已暂停", 恢复后看到"对端已恢复"并正常跑完
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

/// 接收端暂停/恢复经控制会话推给发送端, 发送端上报 Paused/Resumed
/// (改造前接收端暂停对发送端只是"写不动", 唯有等空闲超时中断)
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
            ctrl_rx,
            ev_tx,
        )
        .await
    });

    // 数据开始流动(任务表已登记、数据阶段进行中)后接收端按下暂停
    wait_event(&mut sender_events, |e| {
        matches!(e, TransferEvent::Progress { .. })
    })
    .await;
    assert!(h.handle.pause(&tid), "接收端暂停失败(任务不在表中)");
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

/// 接收端主动取消要以 Cancelled(而非 Interrupted)终结发送端:
/// 取消指令经控制会话送达, 对端不再把主动取消误判为意外断连
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
            ctrl_rx,
            ev_tx,
        )
        .await
    });

    // 先暂停钉住双方数据泵, 再取消 —— 取消时序稳定不依赖传输速度
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

    // 双端都以 Cancelled 终结(改造前发送端只能感知为连接断开 → Interrupted)
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

    // 主动取消不留 .part 临时文件
    let leftover = walkdir_names(&h.download_dir);
    assert!(
        !leftover.iter().any(|n| n.ends_with(super::PART_SUFFIX)),
        "取消后残留临时文件: {leftover:?}"
    );
}
