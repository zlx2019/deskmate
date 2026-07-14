//! 子命令实现: id / listen / scan / send / text

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use deskmate_core::DEFAULT_DISCOVERY_PORT;
use deskmate_core::discovery::{DiscoveryService, PeerEvent};
use deskmate_core::identity::DeviceIdentity;
use deskmate_core::transfer::{
    ConflictPolicy, ControlState, OfferDecision, ReceiverOptions, TransferEvent, TransferOffer,
    send_files, send_text, spawn_receiver,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};

use crate::CommonArgs;
use crate::output::{ProgressBar, addrs_label, human_bytes, print_peer_table};

/// 目标解析的扫描等待上限
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(8);

/// `id`: 显示本机身份信息
pub async fn cmd_id(common: &CommonArgs) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    println!("名称    : {}", identity.display_name);
    println!("设备 ID : {}", identity.device_id);
    println!("指纹    : {}", identity.fingerprint);
    println!("平台    : {}", deskmate_core::identity::platform());
    println!("数据目录: {}", common.data_dir.display());
    Ok(())
}

/// `listen`: 驻留接收文件与文本, 同时对外广播自己
pub async fn cmd_listen(
    common: &CommonArgs,
    download_dir: PathBuf,
    port: u16,
    name: Option<String>,
    auto_accept: bool,
) -> Result<()> {
    let mut identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    if let Some(n) = name {
        identity.display_name = n;
    }
    let identity = Arc::new(identity);

    tokio::fs::create_dir_all(&download_dir).await?;
    // IPv6 双栈优先(无 v6 栈自动回退 IPv4)
    let listener = deskmate_core::transfer::bind_dual_stack(port)
        .await
        .with_context(|| format!("端口 {port} 监听失败(被占用? 换 --port 重试)"))?;
    let tcp_port = listener.local_addr()?.port();

    let (offers_tx, mut offers) = mpsc::channel::<TransferOffer>(16);
    let (events_tx, mut events) = mpsc::channel::<TransferEvent>(256);
    spawn_receiver(
        Arc::clone(&identity),
        listener,
        ReceiverOptions {
            download_dir: download_dir.clone(),
            // CLI 不支持图片头像与 PIN
            avatar_image: None,
            resume_dir: common.data_dir.join("resume"),
            pin: None,
        },
        offers_tx,
        events_tx,
    )?;
    let (discovery, mut peers) = DiscoveryService::start(
        identity.peer_info(),
        tcp_port,
        DEFAULT_DISCOVERY_PORT,
        false,
    )
    .await?;

    println!("deskmate 接收端已就绪:");
    println!("  名称: {}  端口: {tcp_port}", identity.display_name);
    println!("  指纹: {}", identity.fingerprint);
    println!("  下载目录: {}", download_dir.display());
    println!(
        "  模式: {}",
        if auto_accept {
            "自动接受(--yes)"
        } else {
            "逐个确认"
        }
    );
    println!("等待连接... (Ctrl+C 退出)\n");

    let mut bar = ProgressBar::new();
    loop {
        tokio::select! {
            Some(offer) = offers.recv() => handle_offer(offer, auto_accept, &mut bar).await,
            Some(event) = events.recv() => print_transfer_event(event, &mut bar),
            Some(peer_event) = peers.recv() => print_peer_event(peer_event, &mut bar),
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    println!("\n正在下线...");
    discovery.shutdown().await;
    Ok(())
}

/// `scan`: 被动扫描并列出在线节点
pub async fn cmd_scan(common: &CommonArgs, wait_secs: u64) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    let (discovery, mut events) =
        DiscoveryService::start(identity.peer_info(), 0, DEFAULT_DISCOVERY_PORT, true).await?;

    println!("扫描局域网节点({wait_secs}s)...");
    let deadline = tokio::time::sleep(Duration::from_secs(wait_secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            event = events.recv() => match event {
                Some(PeerEvent::Up(p)) => {
                    println!("  + {} ({})", p.info.name, addrs_label(&p.addrs, p.port));
                }
                Some(PeerEvent::Down(_)) => {}
                None => break,
            },
        }
    }

    println!();
    print_peer_table(&discovery.peers());
    discovery.shutdown().await;
    Ok(())
}

/// `send`: 发送文件/目录到目标节点
pub async fn cmd_send(common: &CommonArgs, paths: Vec<PathBuf>, target: &str) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    let (addrs, port, expected_fp) = resolve_target(&identity, target).await?;
    if expected_fp.is_none() {
        println!("⚠ 直连模式: 未经发现层校验对端身份, 完成后请核对指纹");
    }

    let (events_tx, mut events) = mpsc::channel::<TransferEvent>(256);
    // CLI 不做暂停交互(引擎已支持, UI 里接入); Ctrl+C 中断即按意外断连处理
    let (_control_tx, control) = watch::channel(ControlState::Running);

    let printer = tokio::spawn(async move {
        let mut bar = ProgressBar::new();
        while let Some(event) = events.recv().await {
            print_transfer_event(event, &mut bar);
        }
    });

    println!(
        "正在发送 {} 个路径 → {} ...",
        paths.len(),
        addrs_label(&addrs, port)
    );
    let summary = send_files(
        &identity,
        &addrs,
        port,
        expected_fp,
        None,
        // CLI 暂不支持 PIN(对端启用 PIN 时会明确报错)
        None,
        &paths,
        control,
        events_tx,
    )
    .await
    .map_err(|e| anyhow::anyhow!("发送失败: {e}"))?;
    let _ = printer.await;

    println!(
        "✅ 发送完成: {} 个文件, {} → {}",
        summary.files_sent,
        human_bytes(summary.bytes_sent),
        summary.peer.name
    );
    println!("   对端指纹: {}", summary.peer.fingerprint);
    Ok(())
}

/// `text`: 发送一段文本(逐字节一致)
pub async fn cmd_text(common: &CommonArgs, text: &str, target: &str) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    let (addrs, port, expected_fp) = resolve_target(&identity, target).await?;
    let peer = send_text(&identity, &addrs, port, expected_fp, None, text)
        .await
        .map_err(|e| anyhow::anyhow!("发送失败: {e}"))?;
    println!("✅ 文本已送达 {} ({} 字节)", peer.name, text.len());
    Ok(())
}

/// 解析发送目标:
/// - `ip:port` → 直连(不校验指纹, 打印警告由调用方负责)
/// - 其他 → 被动扫描, 按 名称 / 指纹前缀 / IP 匹配
async fn resolve_target(
    identity: &DeviceIdentity,
    target: &str,
) -> Result<(Vec<IpAddr>, u16, Option<String>)> {
    if let Ok(sa) = target.parse::<SocketAddr>() {
        return Ok((vec![sa.ip()], sa.port(), None));
    }

    println!("正在查找节点 \"{target}\" ...");
    let (discovery, mut events) =
        DiscoveryService::start(identity.peer_info(), 0, DEFAULT_DISCOVERY_PORT, true).await?;

    let found = tokio::time::timeout(RESOLVE_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            if let PeerEvent::Up(p) = event {
                let hit = p.info.name == target
                    || p.info.fingerprint.starts_with(target)
                    || p.addrs.iter().any(|a| a.to_string() == target);
                if hit {
                    return Some(p);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten();

    // 首个匹配事件的地址可能尚未解析齐全(mDNS 记录逐条到达),
    // 短暂等待后用注册表的合并快照补全候选地址列表
    let found = match found {
        Some(p) => {
            tokio::time::sleep(Duration::from_millis(600)).await;
            Some(
                discovery
                    .peers()
                    .into_iter()
                    .find(|q| q.info.fingerprint == p.info.fingerprint)
                    .unwrap_or(p),
            )
        }
        None => None,
    };

    let online = discovery.peers();
    discovery.shutdown().await;

    match found {
        Some(p) => Ok((p.addrs, p.port, Some(p.info.fingerprint))),
        None => {
            let names: Vec<String> = online.iter().map(|p| p.info.name.clone()).collect();
            bail!(
                "{RESOLVE_TIMEOUT:?} 内未找到节点 \"{target}\"; 当前可见: {}",
                if names.is_empty() {
                    "(无)".to_string()
                } else {
                    names.join(", ")
                }
            )
        }
    }
}

/// 处理传输请求: 自动接受或终端询问
async fn handle_offer(offer: TransferOffer, auto_accept: bool, bar: &mut ProgressBar) {
    bar.clear();
    println!(
        "\n📥 来自 {} ({}) 的传输请求:",
        offer.peer.name, offer.peer.platform
    );
    for f in &offer.files {
        println!("   {} ({})", f.rel_path, human_bytes(f.size));
    }
    println!(
        "   共 {} 个文件, {}",
        offer.files.len(),
        human_bytes(offer.total_size)
    );

    let accept = if auto_accept {
        println!("   已自动接受(--yes)");
        true
    } else {
        ask_yes_no("   接受吗? [y/N] ").await
    };

    let decision = if accept {
        OfferDecision::Accept {
            accepted_files: offer.files.iter().map(|f| f.file_id).collect(),
            save_dir: None,
            // CLI 维持既有行为: 同名自动重命名
            conflict: ConflictPolicy::default(),
        }
    } else {
        OfferDecision::Reject {
            reason: Some("接收方拒绝".to_string()),
        }
    };
    let _ = offer.reply.send(decision);
}

/// 终端询问 y/n(读一行 stdin)
async fn ask_yes_no(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match BufReader::new(tokio::io::stdin())
        .read_line(&mut line)
        .await
    {
        Ok(_) => matches!(line.trim(), "y" | "Y" | "yes"),
        Err(_) => false,
    }
}

/// 渲染传输事件
fn print_transfer_event(event: TransferEvent, bar: &mut ProgressBar) {
    match event {
        TransferEvent::Progress {
            rel_path,
            done,
            size,
            ..
        } => bar.update(&rel_path, done, size),
        TransferEvent::FileCompleted { path, .. } => {
            bar.clear();
            println!("  ✓ {}", path.display());
        }
        TransferEvent::Completed { .. } => {
            bar.clear();
            println!("✅ 传输完成");
        }
        TransferEvent::Cancelled { .. } => {
            bar.clear();
            println!("✖ 传输已取消, 未完成文件已删除");
        }
        TransferEvent::Interrupted { reason, .. } => {
            bar.clear();
            println!("⚠ 传输中断: {reason}(未完成部分已保留, 待续传)");
        }
        TransferEvent::TextReceived { from, text } => {
            bar.clear();
            println!("📋 来自 {} 的文本(已逐字节校验):", from.name);
            println!("{text}");
        }
    }
}

/// 渲染节点上下线事件
fn print_peer_event(event: PeerEvent, bar: &mut ProgressBar) {
    bar.clear();
    match event {
        PeerEvent::Up(p) => {
            println!(
                "🟢 上线: {} ({}, {})",
                p.info.name,
                addrs_label(&p.addrs, p.port),
                p.info.platform
            );
        }
        PeerEvent::Down(fp) => {
            println!("⚪ 下线: {}", fp.get(..12).unwrap_or(&fp));
        }
    }
}
