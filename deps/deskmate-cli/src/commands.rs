//! Subcommand implementations: id, listen, scan, send, and text.

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

/// Maximum discovery wait time while resolving a target.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(8);

/// `id`: displays the local device identity.
pub async fn cmd_id(common: &CommonArgs) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    println!("Name        : {}", identity.display_name);
    println!("Device ID   : {}", identity.device_id);
    println!("Fingerprint : {}", identity.fingerprint);
    println!("Platform    : {}", deskmate_core::identity::platform());
    println!("Data dir    : {}", common.data_dir.display());
    Ok(())
}

/// `listen`: receives files and text while advertising this device.
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
    // Prefer IPv6 dual stack and fall back to IPv4 if IPv6 is unavailable.
    let listener = deskmate_core::transfer::bind_dual_stack(port)
        .await
        .with_context(|| {
            format!("failed to listen on port {port}; it may be in use, try another --port")
        })?;
    let tcp_port = listener.local_addr()?.port();

    let (offers_tx, mut offers) = mpsc::channel::<TransferOffer>(16);
    let (events_tx, mut events) = mpsc::channel::<TransferEvent>(256);
    spawn_receiver(
        Arc::clone(&identity),
        listener,
        ReceiverOptions {
            download_dir: download_dir.clone(),
            // The CLI does not support image avatars or PINs.
            avatar_image: None,
            resume_dir: common.data_dir.join("resume"),
            pin: None,
        },
        offers_tx,
        events_tx,
    )?;
    let (discovery, mut peers) =
        DiscoveryService::start(&identity, tcp_port, DEFAULT_DISCOVERY_PORT, false).await?;

    println!("deskmate receiver is ready:");
    println!("  Name: {}  Port: {tcp_port}", identity.display_name);
    println!("  Fingerprint: {}", identity.fingerprint);
    println!("  Download directory: {}", download_dir.display());
    println!(
        "  Mode: {}",
        if auto_accept {
            "accept automatically (--yes)"
        } else {
            "confirm each request"
        }
    );
    println!("Waiting for connections... (Ctrl+C to exit)\n");

    let mut bar = ProgressBar::new();
    loop {
        tokio::select! {
            Some(offer) = offers.recv() => handle_offer(offer, auto_accept, &mut bar).await,
            Some(event) = events.recv() => print_transfer_event(event, &mut bar),
            Some(peer_event) = peers.recv() => print_peer_event(peer_event, &mut bar),
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    println!("\nGoing offline...");
    discovery.shutdown().await;
    Ok(())
}

/// `scan`: passively scans for and lists online peers.
pub async fn cmd_scan(common: &CommonArgs, wait_secs: u64) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    let (discovery, mut events) =
        DiscoveryService::start(&identity, 0, DEFAULT_DISCOVERY_PORT, true).await?;

    println!("Scanning for LAN peers ({wait_secs}s)...");
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

/// `send`: sends files or directories to a target peer.
pub async fn cmd_send(common: &CommonArgs, paths: Vec<PathBuf>, target: &str) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    let (addrs, port, expected_fp) = resolve_target(&identity, target).await?;
    if expected_fp.is_none() {
        println!(
            "⚠ Direct mode: peer identity was not verified through discovery; verify the fingerprint after completion"
        );
    }

    let (events_tx, mut events) = mpsc::channel::<TransferEvent>(256);
    // The CLI has no pause controls even though the engine supports them; Ctrl+C
    // is treated as an unexpected disconnect.
    let (_control_tx, control) = watch::channel(ControlState::Running);

    let printer = tokio::spawn(async move {
        let mut bar = ProgressBar::new();
        while let Some(event) = events.recv().await {
            print_transfer_event(event, &mut bar);
        }
    });

    println!(
        "Sending {} path(s) to {}...",
        paths.len(),
        addrs_label(&addrs, port)
    );
    let summary = send_files(
        &identity,
        &addrs,
        port,
        expected_fp,
        None,
        // The CLI does not currently support PINs; a PIN-enabled peer returns a clear error.
        None,
        &paths,
        // The CLI never sends clipboard images, so nothing is marked inline.
        false,
        // The CLI does not apply ignore rules; this integration tool sends every selected item.
        None,
        control,
        events_tx,
    )
    .await
    .map_err(|e| anyhow::anyhow!("send failed: {e}"))?;
    let _ = printer.await;

    println!(
        "✅ Send completed: {} file(s), {} to {}",
        summary.files_sent,
        human_bytes(summary.bytes_sent),
        summary.peer.name
    );
    println!("   Peer fingerprint: {}", summary.peer.fingerprint);
    Ok(())
}

/// `text`: sends text exactly as provided, byte for byte.
pub async fn cmd_text(common: &CommonArgs, text: &str, target: &str) -> Result<()> {
    let identity = DeviceIdentity::load_or_create(&common.data_dir)?;
    let (addrs, port, expected_fp) = resolve_target(&identity, target).await?;
    let peer = send_text(&identity, &addrs, port, expected_fp, None, text)
        .await
        .map_err(|e| anyhow::anyhow!("send failed: {e}"))?;
    println!("✅ Text delivered to {} ({} bytes)", peer.name, text.len());
    Ok(())
}

/// Resolves a transfer target:
/// - `ip:port` connects directly without fingerprint verification; the caller prints a warning.
/// - Other forms trigger passive discovery and match by name, fingerprint prefix, or IP.
async fn resolve_target(
    identity: &DeviceIdentity,
    target: &str,
) -> Result<(Vec<IpAddr>, u16, Option<String>)> {
    if let Ok(sa) = target.parse::<SocketAddr>() {
        return Ok((vec![sa.ip()], sa.port(), None));
    }

    println!("Looking for peer \"{target}\"...");
    let (discovery, mut events) =
        DiscoveryService::start(identity, 0, DEFAULT_DISCOVERY_PORT, true).await?;

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

    // The first matching event may not contain every address because mDNS records
    // arrive separately. Wait briefly, then use the registry's merged snapshot.
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
                "peer \"{target}\" was not found within {RESOLVE_TIMEOUT:?}; currently visible: {}",
                if names.is_empty() {
                    "(none)".to_string()
                } else {
                    names.join(", ")
                }
            )
        }
    }
}

/// Handles a transfer offer by accepting automatically or prompting in the terminal.
async fn handle_offer(offer: TransferOffer, auto_accept: bool, bar: &mut ProgressBar) {
    bar.clear();
    println!(
        "\n📥 Transfer request from {} ({}):",
        offer.peer.name, offer.peer.platform
    );
    for f in &offer.files {
        println!("   {} ({})", f.rel_path, human_bytes(f.size));
    }
    println!(
        "   {} file(s), {} total",
        offer.files.len(),
        human_bytes(offer.total_size)
    );

    let accept = if auto_accept {
        println!("   Accepted automatically (--yes)");
        true
    } else {
        ask_yes_no("   Accept? [y/N] ").await
    };

    let decision = if accept {
        OfferDecision::Accept {
            accepted_files: offer.files.iter().map(|f| f.file_id).collect(),
            save_dir: None,
            // Preserve the CLI's existing behavior by renaming conflicts automatically.
            conflict: ConflictPolicy::default(),
        }
    } else {
        OfferDecision::Reject {
            reason: Some("rejected by receiver".to_string()),
        }
    };
    let _ = offer.reply.send(decision);
}

/// Prompts for y/n in the terminal by reading one line from stdin.
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

/// Renders a transfer event.
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
            println!("✅ Transfer completed");
        }
        TransferEvent::Cancelled { .. } => {
            bar.clear();
            println!("✖ Transfer cancelled; incomplete files were deleted");
        }
        TransferEvent::Interrupted { reason, .. } => {
            bar.clear();
            println!("⚠ Transfer interrupted: {reason} (incomplete data retained for resuming)");
        }
        TransferEvent::Paused { .. } => {
            bar.clear();
            println!("⏸ The peer paused the transfer");
        }
        TransferEvent::Resumed { .. } => {
            bar.clear();
            println!("▶ The peer resumed the transfer");
        }
        TransferEvent::TextReceived { from, text } => {
            bar.clear();
            println!("📋 Text from {} (verified byte for byte):", from.name);
            println!("{text}");
        }
    }
}

/// Renders a peer availability event.
fn print_peer_event(event: PeerEvent, bar: &mut ProgressBar) {
    bar.clear();
    match event {
        PeerEvent::Up(p) => {
            println!(
                "🟢 Online: {} ({}, {})",
                p.info.name,
                addrs_label(&p.addrs, p.port),
                p.info.platform
            );
        }
        PeerEvent::Down(fp) => {
            println!("⚪ Offline: {}", fp.get(..12).unwrap_or(&fp));
        }
    }
}
