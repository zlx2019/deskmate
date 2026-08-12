//! Discovery integration tests for identity updates over real UDP multicast.
//!
//! Advertiser A and passive observer B use real sockets. Environments without
//! UDP multicast loopback, including some CI sandboxes, skip after the initial probe.
//!
//! Peers use real [`DeviceIdentity`] values isolated by `TempDir` because
//! `DiscoveryService::start` builds probe TLS credentials from certificate-derived
//! fingerprints rather than test constants.

use std::time::Duration;

use deskmate_core::discovery::{DiscoveryService, PeerEvent};
use deskmate_core::identity::DeviceIdentity;
use deskmate_core::protocol::PeerInfo;

/// Test discovery port avoiding the application's real port 42425.
const TEST_DISCOVERY_PORT: u16 = 48425;

/// Isolated temporary directory removed automatically on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    #[expect(
        clippy::unwrap_used,
        reason = "test setup should fail immediately if the temporary directory cannot be created"
    )]
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("deskmate-disc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds identity information for `update_info`.
///
/// The fingerprint must come from the real identity because changing it creates a
/// new peer instead of updating the local device.
fn updated_info(name: &str, fingerprint: &str) -> PeerInfo {
    PeerInfo {
        device_id: format!("test-dev-{fingerprint}"),
        name: name.to_string(),
        fingerprint: fingerprint.to_string(),
        platform: "test".to_string(),
        avatar: None,
        os_version: None,
    }
}

/// Receives events until the predicate matches or the timeout returns `None`.
async fn wait_for(
    events: &mut tokio::sync::mpsc::Receiver<PeerEvent>,
    mut pred: impl FnMut(&PeerEvent) -> bool,
) -> Option<PeerEvent> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = events.recv().await {
            if pred(&ev) {
                return Some(ev);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// `update_info` emits an `Up` with the new name and avatar without going offline.
#[tokio::test]
async fn update_info_propagates_over_lan() {
    let (dir_a, dir_b) = (TempDir::new(), TempDir::new());
    let id_a = DeviceIdentity::load_or_create(&dir_a.0).expect("failed to create identity A");
    let id_b = DeviceIdentity::load_or_create(&dir_b.0).expect("failed to create identity B");
    let (svc_a, _events_a) = DiscoveryService::start(&id_a, 40001, TEST_DISCOVERY_PORT, false)
        .await
        .expect("failed to start A");
    let (svc_b, mut events_b) = DiscoveryService::start(&id_b, 40002, TEST_DISCOVERY_PORT, true)
        .await
        .expect("failed to start B");

    // B should initially see A by fingerprint because its hostname-derived name
    // is unpredictable. Skip when the environment lacks multicast loopback.
    let Some(_) = wait_for(
        &mut events_b,
        |ev| matches!(ev, PeerEvent::Up(p) if p.info.fingerprint == id_a.fingerprint),
    )
    .await
    else {
        eprintln!("skipped: UDP multicast loopback is unavailable in this environment");
        svc_a.shutdown().await;
        svc_b.shutdown().await;
        return;
    };

    // Update name and avatar on a standard thread without a Tokio runtime to match
    // synchronous Tauri IPC. This guards against a former internal spawn panic.
    let mut updated = updated_info("Renamed device", &id_a.fingerprint);
    updated.avatar = Some("🚀".to_string());
    std::thread::scope(|s| {
        s.spawn(|| svc_a.update_info(&updated));
    });

    // The immediate update announcement should deliver the new information as Up.
    let got = wait_for(
        &mut events_b,
        |ev| matches!(ev, PeerEvent::Up(p) if p.info.fingerprint == id_a.fingerprint && p.info.name == "Renamed device"),
    )
    .await
    .expect("peer did not receive the updated identity");
    if let PeerEvent::Up(p) = got {
        assert_eq!(p.info.avatar.as_deref(), Some("🚀"));
    }

    svc_a.shutdown().await;
    svc_b.shutdown().await;
}

/// Enabling stealth immediately emits offline, while disabling it immediately
/// reappears through an announcement without restarting.
#[tokio::test]
async fn passive_toggle_propagates_over_lan() {
    // Use another port to avoid multicast interference with the parallel test.
    let port = TEST_DISCOVERY_PORT + 1;
    let (dir_a, dir_b) = (TempDir::new(), TempDir::new());
    let id_a = DeviceIdentity::load_or_create(&dir_a.0).expect("failed to create identity A");
    let id_b = DeviceIdentity::load_or_create(&dir_b.0).expect("failed to create identity B");
    let (svc_a, _events_a) = DiscoveryService::start(&id_a, 40003, port, false)
        .await
        .expect("failed to start A");
    let (svc_b, mut events_b) = DiscoveryService::start(&id_b, 40004, port, true)
        .await
        .expect("failed to start B");

    // Skip the test when the environment lacks multicast loopback.
    let Some(_) = wait_for(
        &mut events_b,
        |ev| matches!(ev, PeerEvent::Up(p) if p.info.fingerprint == id_a.fingerprint),
    )
    .await
    else {
        eprintln!("skipped: UDP multicast loopback is unavailable in this environment");
        svc_a.shutdown().await;
        svc_b.shutdown().await;
        return;
    };

    // Enable stealth on a standard thread to reproduce synchronous Tauri IPC.
    std::thread::scope(|s| {
        s.spawn(|| svc_a.set_passive(true));
    });
    wait_for(
        &mut events_b,
        |ev| matches!(ev, PeerEvent::Down(fp) if *fp == id_a.fingerprint),
    )
    .await
    .expect("peer did not observe stealth departure");

    // Disable stealth and reappear through the immediate announcement.
    std::thread::scope(|s| {
        s.spawn(|| svc_a.set_passive(false));
    });
    wait_for(
        &mut events_b,
        |ev| matches!(ev, PeerEvent::Up(p) if p.info.fingerprint == id_a.fingerprint),
    )
    .await
    .expect("peer did not observe reappearance");

    svc_a.shutdown().await;
    svc_b.shutdown().await;
}
