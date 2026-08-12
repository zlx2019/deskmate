//! Discovery layer: makes Deskmate peers visible to each other on a LAN.
//!
//! Dual-channel design:
//! - Primary: mDNS/DNS-SD registration and browsing for `_deskmate._tcp.local.`
//! - Fallback: periodic UDP multicast announcements for networks that block mDNS
//!
//! Either channel is sufficient; initialization fails only when both are unavailable.
//! Peer lifecycle: heartbeat every 5 seconds, offline after a 15-second timeout,
//! and a goodbye packet on graceful exit. A watchdog repairs multicast membership
//! after sleep or network reconnection, both of which can silently drop IGMP
//! membership in the kernel. Discovery packets contain only small fields; larger
//! data such as avatars is fetched on demand after a TCP connection is established.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::identity::DeviceIdentity;
use crate::protocol::PeerInfo;
use crate::tls;

use crate::config::{
    EVENT_CHANNEL_CAP, HEARTBEAT_INTERVAL, MULTICAST_SILENCE_TIMEOUT, PEER_PROBE_INTERVAL,
    PEER_PROBE_TIMEOUT, PEER_TIMEOUT,
};

/// mDNS service type.
pub const MDNS_SERVICE_TYPE: &str = "_deskmate._tcp.local.";
/// UDP multicast group in 224.0.0.0/24 for broad router compatibility.
const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 168);

/// Discovery layer errors.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// Both mDNS and UDP multicast failed to initialize.
    #[error("discovery unavailable: both mDNS and UDP multicast failed to initialize")]
    AllChannelsFailed,
    /// UDP socket operation failed.
    #[error("UDP multicast channel error: {0}")]
    Io(#[from] std::io::Error),
    /// mDNS daemon operation failed.
    #[error("mDNS channel error: {0}")]
    Mdns(#[from] mdns_sd::Error),
}

/// An online peer on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// Device information: ID, name, fingerprint, and platform.
    pub info: PeerInfo,
    /// Candidate addresses, with non-loopback IPv4 first and multiple interfaces retained.
    pub addrs: Vec<IpAddr>,
    /// TCP port shared by control and data channels.
    pub port: u16,
}

/// Peer availability event.
#[derive(Debug, Clone)]
pub enum PeerEvent {
    /// A peer came online or changed its information.
    Up(Peer),
    /// A peer went offline; the value is its certificate fingerprint.
    Down(String),
}

/// UDP multicast packet type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AnnounceKind {
    /// Periodic online announcement.
    Announce,
    /// Unicast response to an announcement so a new peer sees existing peers immediately.
    Response,
    /// Graceful departure.
    Goodbye,
}

/// UDP multicast packet; the IP comes from the UDP source address.
#[derive(Debug, Serialize, Deserialize)]
struct AnnouncePacket {
    /// Packet type.
    kind: AnnounceKind,
    /// Device information.
    info: PeerInfo,
    /// TCP listening port.
    tcp_port: u16,
}

/// Serializes a UDP discovery packet.
///
/// Returns empty bytes on failure, which is not expected for these simple fields.
fn encode_packet(kind: AnnounceKind, info: &PeerInfo, tcp_port: u16) -> Vec<u8> {
    serde_json::to_vec(&AnnouncePacket {
        kind,
        info: info.clone(),
        tcp_port,
    })
    .unwrap_or_default()
}

/// Pre-serialized UDP packet set replaced atomically when identity changes.
struct UdpPackets {
    /// Periodic announcement.
    announce: Vec<u8>,
    /// Unicast response to an announcement.
    response: Vec<u8>,
    /// Graceful departure.
    goodbye: Vec<u8>,
}

impl UdpPackets {
    /// Encodes all packets for the current identity.
    ///
    /// Passive stealth mode receives only, so all outgoing packets are empty.
    fn encode(info: &PeerInfo, tcp_port: u16, passive: bool) -> Self {
        if passive {
            return Self {
                announce: Vec::new(),
                response: Vec::new(),
                goodbye: Vec::new(),
            };
        }
        Self {
            announce: encode_packet(AnnounceKind::Announce, info, tcp_port),
            response: encode_packet(AnnounceKind::Response, info, tcp_port),
            goodbye: encode_packet(AnnounceKind::Goodbye, info, tcp_port),
        }
    }
}

/// Shared packet-set handle.
type SharedPackets = Arc<std::sync::RwLock<UdpPackets>>;

/// Locks the packet set for reading, recovering poisoned data directly.
fn read_packets(packets: &SharedPackets) -> std::sync::RwLockReadGuard<'_, UdpPackets> {
    packets.read().unwrap_or_else(PoisonError::into_inner)
}

/// Multicast receive pulse: time of the latest multicast announcement or goodbye.
///
/// The receive loop updates this and the membership watchdog reads it to detect
/// a multicast path that worked before but has gone silent. Unicast responses do
/// not refresh the pulse because they bypass multicast and remain reachable after
/// membership loss, which would mask the failure.
type MulticastPulse = Arc<Mutex<Option<Instant>>>;

/// Updates the multicast pulse to the current time, recovering poisoned data directly.
fn mark_pulse(pulse: &MulticastPulse) {
    *pulse.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
}

/// Reads the multicast pulse, recovering poisoned data directly.
fn read_pulse(pulse: &MulticastPulse) -> Option<Instant> {
    *pulse.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Peer registry that merges mDNS and UDP sources, tracks liveness, and emits events.
struct Registry {
    /// Local fingerprint used to ignore self-discovery.
    self_fingerprint: String,
    /// Online peers keyed by certificate fingerprint.
    peers: Mutex<HashMap<String, PeerState>>,
    /// Event sender; events are dropped when full and consumers can use snapshots.
    events: mpsc::Sender<PeerEvent>,
}

/// Source channel for peer information.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PeerSource {
    /// Event-driven mDNS browsing, which does not repeat resolved events while stable.
    Mdns,
    /// Heartbeat-driven UDP multicast, refreshed every heartbeat interval.
    Udp,
}

/// Peer state stored in the registry.
///
/// Liveness is tracked separately per source. mDNS can only expire through
/// `ServiceRemoved` after a goodbye or TTL expiration because it has no periodic
/// heartbeat. UDP expires after its heartbeat timeout. A peer goes offline only
/// when both channels are dead, preventing false removal on networks where
/// multicast is blocked but mDNS still works.
struct PeerState {
    /// Peer information.
    peer: Peer,
    /// Latest UDP heartbeat, or `None` if never seen through UDP.
    last_udp: Option<Instant>,
    /// mDNS liveness set by `ServiceResolved` and cleared by `ServiceRemoved`.
    mdns_alive: bool,
    /// Most recent TCP probe start time for throttling; joining counts as a probe.
    last_probe: Option<Instant>,
}

impl Registry {
    /// Locks the registry, recovering poisoned data because it has no cross-thread invariants.
    fn lock_peers(&self) -> MutexGuard<'_, HashMap<String, PeerState>> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Inserts or refreshes a peer, emitting `Up` only when information changes.
    ///
    /// mDNS contributes multiple addresses while UDP contributes one source address.
    /// They are merged, deduplicated, normalized, and stably sorted with IPv4 first
    /// so alternating channels do not reorder the list. Updates with no usable
    /// address are ignored until a later event provides one. Stale addresses are
    /// not removed individually; the list is rebuilt after the peer goes offline.
    fn upsert(&self, mut peer: Peer, source: PeerSource) {
        if peer.info.fingerprint == self.self_fingerprint {
            return;
        }
        let mut peers = self.lock_peers();
        let fingerprint = peer.info.fingerprint.clone();
        // Preserve liveness from the other channel; a new source only strengthens it.
        let (changed, mut last_udp, mut mdns_alive, last_probe) = match peers.get(&fingerprint) {
            Some(state) => {
                let mut merged = state.peer.addrs.clone();
                for addr in &peer.addrs {
                    if !merged.contains(addr) {
                        merged.push(*addr);
                    }
                }
                peer.addrs = normalize_addrs(merged);
                (
                    state.peer != peer,
                    state.last_udp,
                    state.mdns_alive,
                    state.last_probe,
                )
            }
            None => {
                peer.addrs = normalize_addrs(peer.addrs);
                // Joining proves liveness, so wait a full interval before the first probe.
                (true, None, false, Some(Instant::now()))
            }
        };
        if peer.addrs.is_empty() {
            return;
        }
        match source {
            PeerSource::Udp => last_udp = Some(Instant::now()),
            PeerSource::Mdns => mdns_alive = true,
        }
        peers.insert(
            fingerprint,
            PeerState {
                peer: peer.clone(),
                last_udp,
                mdns_alive,
                last_probe,
            },
        );
        drop(peers);
        if changed {
            self.emit(PeerEvent::Up(peer));
        }
    }

    /// Removes a peer by fingerprint and emits `Down`.
    fn remove(&self, fingerprint: &str) {
        let existed = self.lock_peers().remove(fingerprint).is_some();
        if existed {
            self.emit(PeerEvent::Down(fingerprint.to_string()));
        }
    }

    /// Clears mDNS liveness after a goodbye or TTL expiration.
    ///
    /// A fresh UDP heartbeat keeps the peer online so single-channel degradation
    /// does not cause flicker; otherwise the peer is removed immediately.
    /// `ServiceRemoved` provides only the instance name, which is the device ID.
    fn mdns_removed(&self, device_id: &str, udp_timeout: Duration) {
        let mut peers = self.lock_peers();
        let Some((fp, state)) = peers
            .iter_mut()
            .find(|(_, s)| s.peer.info.device_id == device_id)
        else {
            return;
        };
        state.mdns_alive = false;
        let udp_dead = state.last_udp.is_none_or(|t| t.elapsed() > udp_timeout);
        let fp = fp.clone();
        drop(peers);
        if udp_dead {
            self.remove(&fp);
        }
    }

    /// Removes dead peers and returns suspicious peers that need a TCP probe.
    ///
    /// - Removal: peers with no mDNS liveness and an expired or missing UDP
    ///   heartbeat are removed immediately. mDNS-live peers are removed only by
    ///   `ServiceRemoved`.
    /// - Probing: mDNS-only peers with silent UDP cannot expire by time because
    ///   mDNS has no heartbeat and a crashed peer remains for roughly the two-minute
    ///   SRV TTL. They are returned for connection probing. The probe timestamp is
    ///   updated here to enforce [`PEER_PROBE_INTERVAL`].
    fn sweep(&self, timeout: Duration) -> Vec<Peer> {
        let now = Instant::now();
        let mut probes = Vec::new();
        let expired: Vec<String> = {
            let mut peers = self.lock_peers();
            for state in peers.values_mut() {
                let udp_silent = state.last_udp.is_none_or(|t| t.elapsed() > timeout);
                let probe_due = state
                    .last_probe
                    .is_none_or(|t| t.elapsed() > PEER_PROBE_INTERVAL);
                if state.mdns_alive && udp_silent && probe_due {
                    state.last_probe = Some(now);
                    probes.push(state.peer.clone());
                }
            }
            peers
                .iter()
                .filter(|(_, s)| !s.mdns_alive && s.last_udp.is_none_or(|t| t.elapsed() > timeout))
                .map(|(fp, _)| fp.clone())
                .collect()
        };
        for fp in expired {
            tracing::debug!(fingerprint = %fp, "peer heartbeat timed out; marking offline");
            self.remove(&fp);
        }
        probes
    }

    /// Returns whether a peer still has a fresh UDP heartbeat.
    ///
    /// Fresh evidence overrides a failed probe.
    fn udp_fresh(&self, fingerprint: &str, timeout: Duration) -> bool {
        self.lock_peers()
            .get(fingerprint)
            .is_some_and(|s| s.last_udp.is_some_and(|t| t.elapsed() <= timeout))
    }

    /// Directly handles a failed probe by clearing mDNS liveness and removing the peer.
    ///
    /// This fallback is used only when the mDNS daemon is unavailable and cache
    /// verification cannot run. The normal [`probe_peer`] path lets verification
    /// trigger `ServiceRemoved` so the registry and daemon cache are cleared together.
    /// A peer may recover through UDP during the two-second probe window, such as
    /// after a brief outage. A fresh heartbeat keeps it online because the failed
    /// probe only invalidates liveness supported exclusively by mDNS.
    fn probe_failed(&self, fingerprint: &str, udp_timeout: Duration) {
        let mut peers = self.lock_peers();
        let Some(state) = peers.get_mut(fingerprint) else {
            return;
        };
        if state.last_udp.is_some_and(|t| t.elapsed() <= udp_timeout) {
            return;
        }
        state.mdns_alive = false;
        drop(peers);
        tracing::info!(fingerprint = %fingerprint, "TCP probe failed; marking crashed peer offline");
        self.remove(fingerprint);
    }

    /// Returns a snapshot of online peers.
    fn snapshot(&self) -> Vec<Peer> {
        self.lock_peers().values().map(|s| s.peer.clone()).collect()
    }

    /// Sends an event, dropping it when the channel is full.
    ///
    /// Consumers can reconcile against a snapshot at any time.
    fn emit(&self, event: PeerEvent) {
        if let Err(e) = self.events.try_send(event) {
            tracing::debug!("peer event channel is full; dropping event: {e}");
        }
    }
}

/// Mutable advertisement state shared by stealth and identity updates under one lock.
struct BroadcastState {
    /// Current advertised identity, used to re-encode packets when leaving stealth mode.
    info: PeerInfo,
    /// Stealth mode receives without transmitting.
    passive: bool,
    /// Local mDNS service fullname used for unregistering when present.
    mdns_fullname: Option<String>,
}

/// Discovery service that registers the local device, listens for peers, and emits changes.
pub struct DiscoveryService {
    /// Peer registry.
    registry: Arc<Registry>,
    /// mDNS daemon, or `None` when initialization failed and UDP is used alone.
    mdns: Option<mdns_sd::ServiceDaemon>,
    /// UDP multicast socket, or `None` when initialization failed and mDNS is used alone.
    udp: Option<Arc<UdpSocket>>,
    /// UDP target address containing the multicast group and port.
    udp_target: (Ipv4Addr, u16),
    /// UDP packet set replaced when identity changes.
    packets: SharedPackets,
    /// Advertised TCP port used for mDNS re-registration and fixed after startup.
    tcp_port: u16,
    /// Runtime-mutable identity, stealth flag, and mDNS registration name.
    state: Mutex<BroadcastState>,
    /// Background task handles aborted during shutdown.
    tasks: Vec<JoinHandle<()>>,
}

impl DiscoveryService {
    /// Starts discovery by registering local information and listening for peers.
    ///
    /// Returns the service handle and peer event stream. `identity` provides both
    /// advertised peer information and TLS credentials for probing. A suspicious
    /// peer is alive only after its certificate fingerprint is verified by the
    /// private `probe_peer` function. When `passive` is true, temporary scan and
    /// send scenarios listen without registering mDNS or sending UDP packets.
    pub async fn start(
        identity: &DeviceIdentity,
        tcp_port: u16,
        discovery_port: u16,
        passive: bool,
    ) -> Result<(Self, mpsc::Receiver<PeerEvent>), DiscoveryError> {
        let info = identity.peer_info();
        // Probe TLS accepts any certificate because liveness is established by an
        // explicit post-handshake fingerprint comparison. Each peer has a different
        // expected fingerprint, so one pinned configuration cannot be shared. A
        // configuration failure does not stop discovery; probing falls back to mDNS
        // cache verification and only loses the faster TCP path.
        let probe_tls = match tls::client_config(identity, None) {
            Ok(config) => Some(Arc::new(config)),
            Err(e) => {
                tracing::warn!(
                    "failed to build probe TLS configuration; using mDNS cache verification: {e}"
                );
                None
            }
        };
        let (events_tx, events_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let registry = Arc::new(Registry {
            self_fingerprint: info.fingerprint.clone(),
            peers: Mutex::new(HashMap::new()),
            events: events_tx,
        });
        let mut tasks = Vec::new();

        // Channel one: mDNS registration and browsing.
        let (mdns, mdns_fullname) =
            match start_mdns(&info, tcp_port, passive, &registry, &mut tasks) {
                Ok(pair) => (Some(pair.0), pair.1),
                Err(e) => {
                    tracing::warn!("mDNS initialization failed; using UDP multicast only: {e}");
                    (None, None)
                }
            };

        // Channel two: UDP multicast announcements and responses using a shared
        // packet set that is replaced when identity changes.
        let packets: SharedPackets = Arc::new(std::sync::RwLock::new(UdpPackets::encode(
            &info, tcp_port, passive,
        )));
        // The receive loop updates the multicast pulse for the membership watchdog.
        let pulse: MulticastPulse = Arc::new(Mutex::new(None));
        let udp_target = (MULTICAST_GROUP, discovery_port);
        let udp = match start_udp(
            &info,
            discovery_port,
            Arc::clone(&packets),
            Arc::clone(&pulse),
            &registry,
            &mut tasks,
        )
        .await
        {
            Ok(socket) => Some(socket),
            Err(e) => {
                // This commonly occurs when another local instance owns the port;
                // those instances can still discover each other through mDNS.
                tracing::warn!("UDP multicast initialization failed; using mDNS only: {e}");
                None
            }
        };

        if mdns.is_none() && udp.is_none() {
            return Err(DiscoveryError::AllChannelsFailed);
        }

        // Timeout cleanup and crash probing run concurrently so probes do not delay
        // sweep cadence. Cloning the daemon is cheap because it wraps a command channel.
        let sweeper = Arc::clone(&registry);
        let sweeper_mdns = mdns.clone();
        tasks.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
            loop {
                tick.tick().await;
                for peer in sweeper.sweep(PEER_TIMEOUT) {
                    tokio::spawn(probe_peer(
                        Arc::clone(&sweeper),
                        sweeper_mdns.clone(),
                        probe_tls.clone(),
                        peer,
                    ));
                }
            }
        }));

        // Repair multicast membership after sleep or network reconnection.
        if let Some(socket) = &udp {
            tasks.push(tokio::spawn(membership_watchdog(
                Arc::clone(socket),
                udp_target,
                Arc::clone(&packets),
                pulse,
            )));
        }

        Ok((
            Self {
                registry,
                mdns,
                udp,
                udp_target,
                packets,
                tcp_port,
                state: Mutex::new(BroadcastState {
                    info,
                    passive,
                    mdns_fullname,
                }),
                tasks,
            },
            events_rx,
        ))
    }

    /// Toggles stealth mode immediately without restarting discovery.
    ///
    /// Enabling sends goodbye first, clears UDP packets to stop heartbeats, and
    /// unregisters mDNS so peers receive `ServiceRemoved`. Disabling re-encodes
    /// packets, re-registers mDNS, and announces immediately. Calling with the
    /// current value is a no-op. Like `update_info`, this may run on a synchronous
    /// Tauri IPC thread without a Tokio runtime, so it uses only synchronous,
    /// non-blocking interfaces.
    pub fn set_passive(&self, passive: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.passive == passive {
            return;
        }
        state.passive = passive;
        if passive {
            // Read goodbye before clearing the packet set. Send it twice because
            // UDP is unreliable; peer heartbeat timeout remains the fallback.
            let goodbye = read_packets(&self.packets).goodbye.clone();
            if let Some(udp) = &self.udp
                && !goodbye.is_empty()
            {
                for _ in 0..2 {
                    let _ = udp.try_send_to(&goodbye, self.udp_target.into());
                }
            }
            *self.packets.write().unwrap_or_else(PoisonError::into_inner) =
                UdpPackets::encode(&state.info, self.tcp_port, true);
            if let Some(daemon) = &self.mdns
                && let Some(fullname) = state.mdns_fullname.take()
                && let Err(e) = daemon.unregister(&fullname)
            {
                tracing::warn!(
                    "failed to unregister mDNS; peers will wait for TTL expiration: {e}"
                );
            }
            tracing::info!("entered stealth mode; receiving without advertising");
        } else {
            *self.packets.write().unwrap_or_else(PoisonError::into_inner) =
                UdpPackets::encode(&state.info, self.tcp_port, false);
            if let Some(daemon) = &self.mdns {
                match build_mdns_service(&state.info, self.tcp_port) {
                    Ok(service) => {
                        let fullname = service.get_fullname().to_string();
                        match daemon.register(service) {
                            Ok(()) => state.mdns_fullname = Some(fullname),
                            Err(e) => tracing::warn!("failed to re-register mDNS: {e}"),
                        }
                    }
                    Err(e) => tracing::warn!("failed to build mDNS service information: {e}"),
                }
            }
            // Announce immediately instead of waiting for the next heartbeat.
            if let Some(udp) = &self.udp {
                let announce = read_packets(&self.packets).announce.clone();
                if !announce.is_empty()
                    && let Err(e) = udp.try_send_to(&announce, self.udp_target.into())
                {
                    tracing::debug!(
                        "immediate announcement after leaving stealth failed; heartbeat will retry: {e}"
                    );
                }
            }
            tracing::info!("left stealth mode and resumed advertising");
        }
    }

    /// Updates the advertised identity without interrupting discovery.
    ///
    /// Fingerprint and port are identity foundations and cannot change; only
    /// display fields such as name and avatar are updated:
    /// - UDP re-encodes all packets so heartbeat, response, and goodbye use the
    ///   new content, then sends an immediate announcement.
    /// - mDNS registers the same name again. The daemon replaces it and broadcasts
    ///   new TXT records without an offline/online transition.
    pub fn update_info(&self, info: &PeerInfo) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.info = info.clone();
        *self.packets.write().unwrap_or_else(PoisonError::into_inner) =
            UdpPackets::encode(info, self.tcp_port, state.passive);
        if state.passive {
            return;
        }
        drop(state);
        if let Some(udp) = &self.udp {
            // This may run on a synchronous Tauri IPC thread without a Tokio runtime,
            // where tokio::spawn would panic. UDP try_send_to completes immediately,
            // and the five-second heartbeat covers occasional failures.
            let announce = read_packets(&self.packets).announce.clone();
            if !announce.is_empty()
                && let Err(e) = udp.try_send_to(&announce, self.udp_target.into())
            {
                tracing::debug!(
                    "immediate identity-update announcement failed; heartbeat will retry: {e}"
                );
            }
        }
        if let Some(daemon) = &self.mdns {
            match build_mdns_service(info, self.tcp_port) {
                Ok(service) => {
                    if let Err(e) = daemon.register(service) {
                        tracing::warn!("failed to update mDNS identity: {e}");
                    }
                }
                Err(e) => tracing::warn!("failed to build mDNS service information: {e}"),
            }
        }
    }

    /// Returns a current peer snapshot as a fallback outside the event stream.
    pub fn peers(&self) -> Vec<Peer> {
        self.registry.snapshot()
    }

    /// Looks up one online peer by certificate fingerprint without cloning the full table.
    pub fn peer_by_fingerprint(&self, fingerprint: &str) -> Option<Peer> {
        self.registry
            .lock_peers()
            .get(fingerprint)
            .map(|s| s.peer.clone())
    }

    /// Gracefully sends goodbye, unregisters mDNS, and stops background tasks.
    ///
    /// This operation is idempotent and may be called multiple times.
    pub async fn shutdown(&self) {
        if let Some(udp) = &self.udp {
            // Send goodbye twice because UDP is unreliable; stealth packets are empty.
            let goodbye = read_packets(&self.packets).goodbye.clone();
            if !goodbye.is_empty() {
                for _ in 0..2 {
                    let _ = udp.send_to(&goodbye, self.udp_target).await;
                }
            }
        }
        if let Some(mdns) = &self.mdns {
            // The fullname is absent while stealth registration is disabled, but
            // daemon browsing still needs to shut down.
            let fullname = self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .mdns_fullname
                .take();
            if let Some(fullname) = fullname {
                let _ = mdns.unregister(&fullname);
            }
            let _ = mdns.shutdown();
        }
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Probes a suspicious mDNS-only peer by TLS handshaking each candidate address.
///
/// **Liveness requires verified identity; a bare connection is insufficient.**
/// This rule comes from a real-device lanecho issue on August 4, 2026. Address
/// lists only grow during an online session, so DHCP may reassign a stale address
/// to another Deskmate device listening on the same port. A TCP handshake then
/// succeeds and can keep the disconnected peer permanently online, while transfer
/// fingerprint pinning rejects every address. A peer is alive only when the TLS
/// handshake succeeds and its certificate fingerprint matches. A mismatched peer
/// is ignored while remaining addresses are tried. Crashed-process ports usually
/// return RST immediately; [`PEER_PROBE_TIMEOUT`] covers silent failures such as
/// power loss or unplugging and includes both TCP and TLS.
///
/// If no matching identity is found, the peer is **not removed directly**.
/// Instead, mDNS cache verification from RFC 6762 section 10.4 queries the
/// instance. No response within 10 seconds flushes the cache and emits
/// `ServiceRemoved`, removing the peer through the existing `mdns_removed` path.
/// This clears registry and daemon state together, so any later reconnection is a
/// new discovery with an `Up` event. Direct removal previously caused a real
/// two-device failure: reconnecting before the roughly two-minute cache TTL only
/// refreshed unchanged records without an event, leaving the peer invisible.
/// False negatives are harmless because a live peer answers verification, stays
/// cached, and succeeds on a later probe.
async fn probe_peer(
    registry: Arc<Registry>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    tls_config: Option<Arc<rustls::ClientConfig>>,
    peer: Peer,
) {
    if let Some(config) = &tls_config {
        for addr in &peer.addrs {
            let probed =
                tokio::time::timeout(PEER_PROBE_TIMEOUT, probe_identity(config, *addr, peer.port))
                    .await;
            match probed {
                // A matching fingerprint proves identity; the stream is dropped here.
                Ok(Some(fp)) if fp == peer.info.fingerprint => return,
                Ok(Some(_)) => {
                    tracing::debug!(
                        %addr, name = %peer.info.name,
                        "probe connected but fingerprint mismatched; stale address points elsewhere"
                    );
                }
                // Connection, handshake, or timeout failures move to the next address.
                _ => {}
            }
        }
    }
    // A UDP heartbeat during the probe window is newer evidence, so leave the peer alone.
    if registry.udp_fresh(&peer.info.fingerprint, PEER_TIMEOUT) {
        return;
    }
    match &mdns {
        Some(daemon) => {
            let fullname = format!("{}.{MDNS_SERVICE_TYPE}", peer.info.device_id);
            tracing::info!(name = %peer.info.name, "TCP probe failed; starting mDNS cache verification");
            if let Err(e) = daemon.verify(fullname, mdns_sd::VERIFY_TIMEOUT_DEFAULT) {
                tracing::warn!("failed to start mDNS cache verification; removing directly: {e}");
                registry.probe_failed(&peer.info.fingerprint, PEER_TIMEOUT);
            }
        }
        // An mDNS-live peer should not exist without the mDNS channel; keep this defensively.
        None => registry.probe_failed(&peer.info.fingerprint, PEER_TIMEOUT),
    }
}

/// Completes a TLS handshake to one address and returns the peer certificate fingerprint.
///
/// Connection or handshake failure returns `None`. The stream is dropped after
/// the handshake without application data, so the peer accepts a clean EOF rather
/// than retaining a half-open connection.
async fn probe_identity(
    config: &Arc<rustls::ClientConfig>,
    addr: IpAddr,
    port: u16,
) -> Option<String> {
    let tcp = tokio::net::TcpStream::connect((addr, port)).await.ok()?;
    let connector = tokio_rustls::TlsConnector::from(Arc::clone(config));
    // ServerName is an API requirement; identity is verified by fingerprint.
    let name = rustls_pki_types::ServerName::try_from("deskmate").ok()?;
    let stream = connector.connect(name, tcp).await.ok()?;
    tls::peer_fingerprint(stream.get_ref().1.peer_certificates())
}

/// Returns whether multicast reception has been silent beyond the threshold.
///
/// Never having received multicast is not silence: a lone or stealth peer on an
/// empty network has nothing to recover from, and startup should not rebuild needlessly.
fn multicast_silent(last_seen: Option<Instant>, threshold: Duration) -> bool {
    last_seen.is_some_and(|t| t.elapsed() >= threshold)
}

/// Rebuilds multicast membership and announces immediately unless in stealth mode.
///
/// Leave before joining because joining with stale socket state can fail. A leave
/// failure is expected when membership disappeared with the interface and is
/// ignored. The announcement speeds peer visibility and its multicast loopback
/// verifies the rebuild by refreshing the pulse before the next watchdog cycle.
async fn rejoin_multicast(udp: &UdpSocket, target: (Ipv4Addr, u16), packets: &SharedPackets) {
    let _ = udp.leave_multicast_v4(target.0, Ipv4Addr::UNSPECIFIED);
    if let Err(e) = udp.join_multicast_v4(target.0, Ipv4Addr::UNSPECIFIED) {
        tracing::warn!("failed to rejoin multicast group; will retry next cycle: {e}");
        return;
    }
    let announce = read_packets(packets).announce.clone();
    if !announce.is_empty() {
        let _ = udp.send_to(&announce, target).await;
    }
}

/// Multicast membership watchdog that repairs two kinds of silent disconnection.
///
/// Systems can silently clear IGMP membership without kernel recovery, stopping
/// all multicast reception including loopback without reporting an error:
///
/// 1. **System sleep:** the monotonic clock may stop while wall time advances.
///    Their delta is the stall duration, and crossing the threshold indicates a
///    wake. NTP adjustments are much smaller. Windows monotonic time includes
///    sleep, so the second path covers it.
/// 2. **Network reconnection:** interface shutdown can clear membership without
///    stopping clocks. If multicast worked before and then remains silent beyond
///    [`MULTICAST_SILENCE_TIMEOUT`], the receive path is considered broken and
///    rebuilt every cycle until traffic resumes. Joining may fail while the
///    interface remains down, so one attempt is insufficient.
///
/// The mDNS daemon monitors interfaces and handles its own recovery.
async fn membership_watchdog(
    udp: Arc<UdpSocket>,
    target: (Ipv4Addr, u16),
    packets: SharedPackets,
    pulse: MulticastPulse,
) {
    /// Detection interval.
    const TICK: Duration = Duration::from_secs(30);
    /// Clock-stall threshold indicating a wake from sleep.
    const STALL_JUMP: Duration = Duration::from_secs(60);
    let mut wall = std::time::SystemTime::now();
    let mut mono = Instant::now();
    let mut tick = tokio::time::interval(TICK);
    // Log the first silent cycle and recovery at info, retries at debug, avoiding
    // repeated info logs when stealth mode on an empty network cannot prove recovery.
    let mut silence_logged = false;
    loop {
        tick.tick().await;
        let wall_gap = std::time::SystemTime::now()
            .duration_since(wall)
            .unwrap_or_default();
        let mono_gap = mono.elapsed();
        wall = std::time::SystemTime::now();
        mono = Instant::now();
        let stalled = wall_gap.saturating_sub(mono_gap);
        let silent = multicast_silent(read_pulse(&pulse), MULTICAST_SILENCE_TIMEOUT);
        if stalled >= STALL_JUMP {
            tracing::info!(
                stalled_secs = stalled.as_secs(),
                "detected wake from system sleep; rebuilding multicast membership"
            );
        } else if silent && !silence_logged {
            silence_logged = true;
            tracing::info!("multicast reception exceeded silence threshold; rebuilding membership");
        } else if silent {
            tracing::debug!("multicast reception remains silent; retrying membership rebuild");
        } else {
            if silence_logged {
                silence_logged = false;
                tracing::info!("multicast reception recovered");
            }
            continue;
        }
        rejoin_multicast(&udp, target, &packets).await;
    }
}

/// Builds local mDNS service information for initial registration and identity updates.
///
/// The instance uses `device_id` for uniqueness and fullname stability across
/// display-name changes. The host uses the same value to avoid real hostname conflicts.
fn build_mdns_service(
    info: &PeerInfo,
    tcp_port: u16,
) -> Result<mdns_sd::ServiceInfo, mdns_sd::Error> {
    let mut props: HashMap<String, String> = [
        ("id".to_string(), info.device_id.clone()),
        ("name".to_string(), info.name.clone()),
        ("fp".to_string(), info.fingerprint.clone()),
        ("platform".to_string(), info.platform.clone()),
    ]
    .into();
    // Advertise the optional avatar only when set; it adds only a few TXT bytes.
    if let Some(avatar) = &info.avatar {
        props.insert("avatar".to_string(), avatar.clone());
    }
    // Optional OS version added in protocol 1.3.
    if let Some(osv) = &info.os_version {
        props.insert("osv".to_string(), osv.clone());
    }
    Ok(mdns_sd::ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &info.device_id,
        &format!("{}.local.", info.device_id),
        "",
        tcp_port,
        props,
    )?
    .enable_addr_auto())
}

/// Initializes mDNS, optionally registers the local service, and starts browsing.
fn start_mdns(
    info: &PeerInfo,
    tcp_port: u16,
    passive: bool,
    registry: &Arc<Registry>,
    tasks: &mut Vec<JoinHandle<()>>,
) -> Result<(mdns_sd::ServiceDaemon, Option<String>), DiscoveryError> {
    let daemon = mdns_sd::ServiceDaemon::new()?;

    let fullname = if passive {
        None
    } else {
        let service = build_mdns_service(info, tcp_port)?;
        let name = service.get_fullname().to_string();
        daemon.register(service)?;
        Some(name)
    };

    let receiver = daemon.browse(MDNS_SERVICE_TYPE)?;
    let reg = Arc::clone(registry);
    tasks.push(tokio::spawn(async move {
        while let Ok(event) = receiver.recv_async().await {
            match event {
                mdns_sd::ServiceEvent::ServiceResolved(svc) => {
                    if let Some(peer) = peer_from_mdns(&svc) {
                        reg.upsert(peer, PeerSource::Mdns);
                    }
                }
                mdns_sd::ServiceEvent::ServiceRemoved(_ty, fullname) => {
                    if let Some(device_id) = instance_of(&fullname) {
                        reg.mdns_removed(device_id, PEER_TIMEOUT);
                    }
                }
                _ => {}
            }
        }
    }));

    Ok((daemon, fullname))
}

/// Builds a peer from an mDNS result, returning `None` when required fields are missing.
fn peer_from_mdns(svc: &mdns_sd::ResolvedService) -> Option<Peer> {
    let info = PeerInfo {
        device_id: svc.get_property_val_str("id")?.to_string(),
        name: svc.get_property_val_str("name")?.to_string(),
        fingerprint: svc.get_property_val_str("fp")?.to_string(),
        platform: svc.get_property_val_str("platform")?.to_string(),
        avatar: svc.get_property_val_str("avatar").map(str::to_string),
        os_version: svc.get_property_val_str("osv").map(str::to_string),
    };
    // Preserve normalized candidates from multiple interfaces. ScopedIp includes
    // a scope ID, but this protocol currently retains only the bare address.
    let addrs = normalize_addrs(svc.addresses.iter().map(|ip| ip.to_ip_addr()).collect());
    if addrs.is_empty() {
        return None;
    }
    Some(Peer {
        info,
        addrs,
        port: svc.port,
    })
}

/// Normalizes candidate addresses by removing unusable link-local IPv6 and
/// placing non-loopback IPv4 first.
///
/// The result may be empty when an early mDNS event contains only AAAA records.
/// Callers must discard that update and wait for a later usable address instead
/// of adding unreachable candidates.
fn normalize_addrs(all: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut addrs: Vec<IpAddr> = all.into_iter().filter(|ip| !is_link_local_v6(ip)).collect();
    // Stable sorting preserves arrival order within the same priority.
    addrs.sort_by_key(|ip| (ip.is_loopback(), !ip.is_ipv4()));
    addrs
}

/// Returns whether an IPv6 address is link-local (`fe80::/10`) and unusable without a scope ID.
fn is_link_local_v6(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        IpAddr::V4(_) => false,
    }
}

/// Extracts the instance name, which is the device ID, from an mDNS fullname.
fn instance_of(fullname: &str) -> Option<&str> {
    fullname
        .strip_suffix(MDNS_SERVICE_TYPE)
        .map(|s| s.trim_end_matches('.'))
}

/// Builds a multicast UDP socket with address reuse and binds the discovery port.
///
/// `SO_REUSEADDR`, plus `SO_REUSEPORT` on Unix, lets multiple local instances
/// receive multicast and avoids restart delays. Every reused multicast socket
/// receives its own packet copy, so they do not compete for delivery.
fn bind_multicast_socket(discovery_port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&std::net::SocketAddr::from((Ipv4Addr::UNSPECIFIED, discovery_port)).into())?;
    UdpSocket::from_std(sock.into())
}

/// Initializes UDP multicast, joins the group, and starts heartbeat and receive tasks.
///
/// Packets are read from the shared set so identity updates take effect immediately.
/// In passive mode the set is empty, naturally silencing heartbeats and responses.
/// The receive side refreshes the pulse for multicast-origin packets so the
/// membership watchdog can detect silence.
async fn start_udp(
    info: &PeerInfo,
    discovery_port: u16,
    packets: SharedPackets,
    pulse: MulticastPulse,
    registry: &Arc<Registry>,
    tasks: &mut Vec<JoinHandle<()>>,
) -> Result<Arc<UdpSocket>, DiscoveryError> {
    let socket = bind_multicast_socket(discovery_port)?;
    socket.join_multicast_v4(MULTICAST_GROUP, Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_loop_v4(true)?;
    let socket = Arc::new(socket);

    let sock = Arc::clone(&socket);
    let reg = Arc::clone(registry);
    let self_fp = info.fingerprint.clone();
    tasks.push(tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let announce = read_packets(&packets).announce.clone();
                    if announce.is_empty() {
                        continue;
                    }
                    if let Err(e) = sock.send_to(&announce, (MULTICAST_GROUP, discovery_port)).await {
                        tracing::debug!("failed to send UDP announcement: {e}");
                    }
                }
                recv = sock.recv_from(&mut buf) => {
                    let Ok((n, src)) = recv else { continue };
                    let Ok(packet) = serde_json::from_slice::<AnnouncePacket>(&buf[..n]) else {
                        continue;
                    };
                    // Announcements and goodbyes arrive only through multicast, so
                    // receiving one proves membership is alive; unicast responses do
                    // not count. Self-announcement loopback arrives every heartbeat in
                    // active mode, so record it before filtering the local fingerprint.
                    if packet.kind != AnnounceKind::Response {
                        mark_pulse(&pulse);
                    }
                    if packet.info.fingerprint == self_fp {
                        continue;
                    }
                    match packet.kind {
                        AnnounceKind::Goodbye => reg.remove(&packet.info.fingerprint),
                        kind => {
                            reg.upsert(
                                Peer {
                                    info: packet.info,
                                    addrs: vec![src.ip()],
                                    port: packet.tcp_port,
                                },
                                PeerSource::Udp,
                            );
                            // Reply to an announcement by unicast so the new peer sees us immediately.
                            if kind == AnnounceKind::Announce {
                                let response = read_packets(&packets).response.clone();
                                if !response.is_empty() {
                                    let _ = sock.send_to(&response, src).await;
                                }
                            }
                        }
                    }
                }
            }
        }
    }));

    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a test registry and event receiver.
    fn test_registry(self_fp: &str) -> (Arc<Registry>, mpsc::Receiver<PeerEvent>) {
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        (
            Arc::new(Registry {
                self_fingerprint: self_fp.to_string(),
                peers: Mutex::new(HashMap::new()),
                events: tx,
            }),
            rx,
        )
    }

    /// Builds a test peer.
    fn test_peer(fp: &str, name: &str) -> Peer {
        Peer {
            info: PeerInfo {
                device_id: format!("dev-{fp}"),
                name: name.to_string(),
                fingerprint: fp.to_string(),
                platform: "macos".to_string(),
                avatar: None,
                os_version: None,
            },
            addrs: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2))],
            port: 42424,
        }
    }

    /// Isolated temporary directory removed automatically on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("deskmate-probe-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Starts a TLS listener with the specified identity and returns its address.
    async fn spawn_tls_listener(identity: &DeviceIdentity) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor =
            tokio_rustls::TlsAcceptor::from(Arc::new(tls::server_config(identity).unwrap()));
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                // The probing side disconnects after the handshake; the result is irrelevant.
                tokio::spawn(async move {
                    let _ = acceptor.accept(tcp).await;
                });
            }
        });
        addr
    }

    /// Builds probe TLS configuration with an independent identity and no server pin.
    fn probe_config(dir: &TempDir) -> Arc<rustls::ClientConfig> {
        let id = DeviceIdentity::load_or_create(&dir.0).unwrap();
        Arc::new(tls::client_config(&id, None).unwrap())
    }

    /// Repeated heartbeats do not emit `Up`, while information changes do.
    #[tokio::test]
    async fn upsert_emits_only_on_change() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "old"), PeerSource::Udp);
        reg.upsert(test_peer("aaa", "old"), PeerSource::Udp); // Unchanged heartbeat.
        reg.upsert(test_peer("aaa", "new"), PeerSource::Udp); // Renamed peer.
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.info.name == "old"));
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.info.name == "new"));
        assert!(rx.try_recv().is_err());
    }

    /// Self-discovery packets are filtered.
    #[tokio::test]
    async fn self_is_filtered() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("self", "me"), PeerSource::Udp);
        assert!(rx.try_recv().is_err());
        assert!(reg.snapshot().is_empty());
    }

    /// Cross-channel address merging emits once for new addresses and keeps stable order.
    #[tokio::test]
    async fn upsert_merges_addrs_stably() {
        let (reg, mut rx) = test_registry("self");
        let addr_a = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));
        let addr_b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        // UDP first reports address A.
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.addrs == vec![addr_a]));

        // mDNS reports [B, A], which merges into stable [A, B] and emits for B.
        let mut peer = test_peer("aaa", "n");
        peer.addrs = vec![addr_b, addr_a];
        reg.upsert(peer, PeerSource::Mdns);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.addrs == vec![addr_a, addr_b]));

        // Another UDP heartbeat with A leaves the merged result unchanged.
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        assert!(rx.try_recv().is_err());
    }

    /// Removal emits `Down` only for an existing peer.
    #[tokio::test]
    async fn remove_emits_down_once() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("bbb", "b"), PeerSource::Udp);
        let _ = rx.try_recv();
        reg.remove("bbb");
        reg.remove("bbb"); // Already absent, so no second event.
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "bbb"));
        assert!(rx.try_recv().is_err());
    }

    /// Regression: an mDNS-only peer survives heartbeat sweeps when UDP is blocked.
    #[tokio::test]
    async fn mdns_peer_survives_sweep() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Mdns);
        let _ = rx.try_recv();
        // A zero timeout removes anything governed by time, but not mDNS-live peers.
        reg.sweep(Duration::ZERO);
        assert!(rx.try_recv().is_err());
        assert_eq!(reg.snapshot().len(), 1);
    }

    /// A UDP-only peer is removed after its heartbeat timeout.
    #[tokio::test]
    async fn udp_peer_swept_after_timeout() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        let _ = rx.try_recv();
        reg.sweep(Duration::ZERO);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "aaa"));
        assert!(reg.snapshot().is_empty());
    }

    /// An mDNS removal without a UDP heartbeat takes the peer offline immediately.
    #[tokio::test]
    async fn mdns_removed_downs_peer_without_udp() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Mdns);
        let _ = rx.try_recv();
        reg.mdns_removed("dev-aaa", PEER_TIMEOUT);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "aaa"));
        assert!(reg.snapshot().is_empty());
    }

    /// A fresh UDP heartbeat keeps a peer online after mDNS removal until UDP expires.
    #[tokio::test]
    async fn mdns_removed_keeps_peer_with_live_udp() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Mdns);
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        let _ = rx.try_recv();
        // The UDP heartbeat is still within its timeout window.
        reg.mdns_removed("dev-aaa", PEER_TIMEOUT);
        assert!(rx.try_recv().is_err());
        assert_eq!(reg.snapshot().len(), 1);
        // A zero-timeout sweep removes it after UDP also expires.
        reg.sweep(Duration::ZERO);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "aaa"));
    }

    /// UDP announcement packets survive serialization round trips.
    #[test]
    fn announce_packet_roundtrip() {
        let packet = AnnouncePacket {
            kind: AnnounceKind::Announce,
            info: test_peer("ccc", "c").info,
            tcp_port: 42424,
        };
        let bytes = serde_json::to_vec(&packet).unwrap();
        let back: AnnouncePacket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.kind, AnnounceKind::Announce);
        assert_eq!(back.info.fingerprint, "ccc");
        assert_eq!(back.tcp_port, 42424);
    }

    /// Address normalization removes link-local IPv6, prioritizes IPv4, and may return empty.
    #[test]
    fn normalize_addrs_filters_and_sorts() {
        use std::net::Ipv6Addr;
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));
        let lo4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let ll6 = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
        let lo6 = IpAddr::V6(Ipv6Addr::LOCALHOST);

        // Link-local is removed; non-loopback IPv4 precedes loopback IPv4 and IPv6.
        assert_eq!(normalize_addrs(vec![ll6, lo6, lo4, v4]), vec![v4, lo4, lo6]);
        // Link-local-only input is empty so callers discard the update.
        assert_eq!(normalize_addrs(vec![ll6]), Vec::<IpAddr>::new());
    }

    /// Reproduces a production issue where an initial link-local-only mDNS event
    /// must be deferred until IPv4 arrives and must not retain the link-local address.
    #[tokio::test]
    async fn link_local_only_peer_is_deferred() {
        use std::net::Ipv6Addr;
        let (reg, mut rx) = test_registry("self");
        let ll6 = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));

        // A link-local-only peer is neither stored nor emitted.
        let mut peer = test_peer("aaa", "n");
        peer.addrs = vec![ll6];
        reg.upsert(peer, PeerSource::Mdns);
        assert!(rx.try_recv().is_err());
        assert!(reg.snapshot().is_empty());

        // When IPv4 arrives, emit only usable addresses.
        let mut peer = test_peer("aaa", "n");
        peer.addrs = vec![ll6, v4];
        reg.upsert(peer, PeerSource::Mdns);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.addrs == vec![v4]));
    }

    /// Extracts the instance device ID from an mDNS fullname.
    #[test]
    fn instance_extraction() {
        assert_eq!(
            instance_of("uuid-1234._deskmate._tcp.local."),
            Some("uuid-1234")
        );
        assert_eq!(instance_of("._deskmate._tcp.local."), Some(""));
    }

    /// Sweeps probe only throttled mDNS-only peers with silent UDP.
    #[tokio::test]
    async fn sweep_flags_stale_mdns_only_peers_for_probe() {
        let (reg, _rx) = test_registry("self");
        // aaa is mDNS-only. Clear its join-time probe stamp to make it due.
        reg.upsert(test_peer("aaa", "a"), PeerSource::Mdns);
        // bbb has a fresh UDP heartbeat and is not probed.
        reg.upsert(test_peer("bbb", "b"), PeerSource::Udp);
        reg.lock_peers().get_mut("aaa").unwrap().last_probe = None;

        let probes = reg.sweep(PEER_TIMEOUT);
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].info.fingerprint, "aaa");
        // The updated timestamp throttles the next sweep.
        assert!(reg.sweep(PEER_TIMEOUT).is_empty());
        // Probed peers remain present while mDNS is alive.
        assert_eq!(reg.snapshot().len(), 2);
    }

    /// Failed probes preserve peers with fresh UDP evidence and remove mDNS-only peers.
    #[tokio::test]
    async fn probe_failed_respects_fresh_udp() {
        let (reg, mut rx) = test_registry("self");
        // Dual-channel liveness represents UDP recovery during the probe window.
        reg.upsert(test_peer("aaa", "a"), PeerSource::Mdns);
        reg.upsert(test_peer("aaa", "a"), PeerSource::Udp);
        let _ = rx.try_recv();
        reg.probe_failed("aaa", PEER_TIMEOUT);
        assert_eq!(reg.snapshot().len(), 1);
        assert!(rx.try_recv().is_err());

        // An mDNS-only peer goes offline after a failed probe.
        reg.upsert(test_peer("ccc", "c"), PeerSource::Mdns);
        let _ = rx.try_recv();
        reg.probe_failed("ccc", PEER_TIMEOUT);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "ccc"));
        assert_eq!(reg.snapshot().len(), 1);
    }

    /// End-to-end TCP/TLS probing preserves a listener with the matching
    /// certificate and removes a peer whose listener disappeared.
    #[tokio::test]
    async fn probe_keeps_live_and_removes_dead() {
        let (reg, mut rx) = test_registry("self");
        let (live_dir, prober_dir) = (TempDir::new(), TempDir::new());

        // The live peer listens with its own certificate and matching registry fingerprint.
        let live_id = DeviceIdentity::load_or_create(&live_dir.0).unwrap();
        let live_addr = spawn_tls_listener(&live_id).await;
        let mut live = test_peer(&live_id.fingerprint, "live");
        live.addrs = vec![live_addr.ip()];
        live.port = live_addr.port();
        reg.upsert(live.clone(), PeerSource::Mdns);

        // Bind and immediately close a port to simulate OS cleanup after a crash.
        let dead_port = {
            let l = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            l.local_addr().unwrap().port()
        };
        let mut dead = test_peer("bbb", "dead");
        dead.addrs = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        dead.port = dead_port;
        reg.upsert(dead.clone(), PeerSource::Mdns);
        while rx.try_recv().is_ok() {}

        // A missing daemon exercises direct-removal fallback without mDNS.
        let config = probe_config(&prober_dir);
        probe_peer(Arc::clone(&reg), None, Some(Arc::clone(&config)), live).await;
        probe_peer(Arc::clone(&reg), None, Some(config), dead).await;

        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "bbb"));
        let snapshot = reg.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].info.fingerprint, live_id.fingerprint);
    }

    /// Regression from a lanecho device test on August 4, 2026: a stale address
    /// reassigned to an imposter accepts TCP but presents the wrong certificate.
    /// It must not count as alive, or discovery stays online while pinned transfers fail.
    #[tokio::test]
    async fn probe_rejects_imposter_listener() {
        let (reg, mut rx) = test_registry("self");
        let (imposter_dir, prober_dir) = (TempDir::new(), TempDir::new());

        // The imposter listens with a different identity, as when DHCP reassigns
        // a disconnected peer's address to another Deskmate device.
        let imposter_id = DeviceIdentity::load_or_create(&imposter_dir.0).unwrap();
        let addr = spawn_tls_listener(&imposter_id).await;

        // The registry entry claims that address but retains its own fingerprint.
        let mut victim = test_peer("victim-fp", "victim");
        victim.addrs = vec![addr.ip()];
        victim.port = addr.port();
        reg.upsert(victim.clone(), PeerSource::Mdns);
        while rx.try_recv().is_ok() {}

        probe_peer(
            Arc::clone(&reg),
            None,
            Some(probe_config(&prober_dir)),
            victim,
        )
        .await;

        // Connecting to the imposter is not liveness; no daemon means direct removal.
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "victim-fp"));
        assert!(reg.snapshot().is_empty());
    }

    /// With an mDNS daemon, failed probes defer removal to verification-triggered
    /// `ServiceRemoved` so the registry and daemon cache stay synchronized.
    #[tokio::test]
    async fn probe_failure_with_daemon_defers_to_verify() {
        let (reg, mut rx) = test_registry("self");
        let prober_dir = TempDir::new();
        let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
            eprintln!("skipped: this environment cannot create an mDNS daemon");
            return;
        };

        // Bind and immediately close a dead port.
        let dead_port = {
            let l = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            l.local_addr().unwrap().port()
        };
        let mut dead = test_peer("aaa", "dead");
        dead.addrs = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        dead.port = dead_port;
        reg.upsert(dead.clone(), PeerSource::Mdns);
        while rx.try_recv().is_ok() {}

        probe_peer(
            Arc::clone(&reg),
            Some(daemon.clone()),
            Some(probe_config(&prober_dir)),
            dead,
        )
        .await;

        // The peer remains without a Down event; only ServiceRemoved may remove it.
        assert!(rx.try_recv().is_err());
        assert_eq!(reg.snapshot().len(), 1);
        let _ = daemon.shutdown();
    }

    /// No prior multicast reception does not count as silence.
    #[test]
    fn multicast_silent_requires_prior_reception() {
        assert!(!multicast_silent(None, Duration::ZERO));
    }

    /// A fresh pulse below the threshold is not silent.
    #[test]
    fn multicast_silent_fresh_pulse_is_not_silent() {
        assert!(!multicast_silent(
            Some(Instant::now()),
            Duration::from_secs(3600)
        ));
    }

    /// Prior multicast reception becomes silent after the threshold.
    #[test]
    fn multicast_silent_after_threshold() {
        assert!(multicast_silent(Some(Instant::now()), Duration::ZERO));
    }

    /// A pulse starts empty and contains a timestamp after marking.
    #[test]
    fn pulse_mark_roundtrip() {
        let pulse: MulticastPulse = Arc::new(Mutex::new(None));
        assert!(read_pulse(&pulse).is_none());
        mark_pulse(&pulse);
        assert!(read_pulse(&pulse).is_some());
    }
}
