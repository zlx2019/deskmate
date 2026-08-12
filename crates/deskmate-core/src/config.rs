//! Engine tuning constants for heartbeats, timeouts, buffers, and related settings.
//!
//! These values were previously scattered across modules, making tuning require
//! cross-file searches. Centralizing them provides one place to inspect and adjust.
//! If runtime configuration is needed later through settings or CLI arguments,
//! this module can become the field list for an injected configuration structure,
//! with these constants serving as defaults.
//!
//! Port defaults such as `DEFAULT_TCP_PORT` remain at the crate root. Protocol
//! limits such as frame and avatar sizes are shared contracts between both ends
//! and are defined in [`crate::protocol`].

use std::time::Duration;

// ---- Discovery layer ----

/// Interval between UDP multicast announcements.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Marks a peer offline after this long without a heartbeat.
///
/// This tolerates two consecutive lost heartbeats.
pub const PEER_TIMEOUT: Duration = Duration::from_secs(15);

/// TCP probe interval for peers visible only through mDNS and silent over UDP.
///
/// Time alone cannot expire these peers because mDNS has no periodic heartbeat
/// and a crashed peer remains until its SRV TTL expires after about two minutes.
/// A failed probe triggers mDNS cache verification, which takes 10 seconds, so
/// total detection takes roughly this interval plus verification, or 40 seconds.
/// Keep this at least [`PEER_TIMEOUT`] because mDNS-only operation is degraded
/// and should not be more aggressive than the primary channel, while remaining
/// well below the 120-second mDNS TTL.
pub const PEER_PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// Total timeout per probe address, including the TCP connection and TLS handshake.
///
/// A crashed process is normally detected quickly through RST after the OS
/// releases its listening port. This timeout covers silent failures such as
/// unplugged cables or power loss. LAN TLS handshakes take milliseconds, so two
/// seconds provides enough margin for both stages.
pub const PEER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Multicast receive silence timeout.
///
/// If multicast was received before but then remains silent this long, the local
/// IGMP membership may have been lost across an interface down/up cycle because
/// kernels do not always restore it. Rebuild membership with leave plus join.
///
/// In active mode, loopback announcements must arrive every
/// [`HEARTBEAT_INTERVAL`], so silence confirms a broken receive path. This value
/// only needs to be much longer than the heartbeat interval to tolerate occasional
/// packet loss. Stealth mode has no self-announcements, so an empty network may
/// trigger harmless, idempotent periodic rebuilds.
pub const MULTICAST_SILENCE_TIMEOUT: Duration = Duration::from_secs(60);

/// Peer event channel capacity.
///
/// Events are dropped when full; consumers can recover from a snapshot.
pub const EVENT_CHANNEL_CAP: usize = 64;

// ---- Transfer layer ----

/// Data-channel read/write block size.
///
/// One MiB is enough to saturate 2.5 GbE; see `docs/PLAN.md` section 4.4.
pub const CHUNK_SIZE: usize = 1024 * 1024;

/// Timeout while waiting for a receiver decision.
///
/// This is intentionally long because a person is in the loop.
pub const OFFER_TIMEOUT: Duration = Duration::from_secs(300);

/// Timeout for handshake and response messages.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// TCP connection timeout for each candidate address.
///
/// Multiple interfaces are attempted sequentially, so this should remain short.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Kernel send and receive buffer limit for data connections.
///
/// Platform defaults are often 128-256 KiB, which can restrict the in-flight
/// window on high-bandwidth or unstable Wi-Fi links. The limit grows on demand
/// and does not allocate all memory immediately.
pub const SOCKET_BUFFER_SIZE: usize = 4 * 1024 * 1024;

/// File-size threshold for bypassing the page cache.
///
/// Large sequential transfers use `F_NOCACHE` on macOS so they do not evict hot
/// pages used by other applications. Small files remain cached because they are
/// likely to be opened immediately after receipt.
pub const NOCACHE_THRESHOLD: u64 = 64 * 1024 * 1024;

/// Maximum data-channel idle time before interrupting and retaining resume state.
///
/// Pauses are explicitly sent over the control connection using bidirectional
/// Pause/Resume frames since protocol 1.4. Both pumps suspend while paused, so
/// this timeout does not need extra allowance for invisible pauses. It is not
/// shorter because both ends replay existing data into their hashers during
/// resume, and different disk speeds can leave the faster side waiting. This is
/// also the resource retention limit for a malicious half-open connection.
pub const DATA_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

// ---- Receiver connection management ----

/// Maximum number of concurrent connections.
///
/// New connections are rejected beyond this limit to prevent slow-loris attacks
/// from exhausting file descriptors or memory. Normal operation uses one or two
/// connections per peer, so 128 leaves ample headroom.
pub const MAX_CONCURRENT_CONNECTIONS: usize = 128;

/// Timeout for the unauthenticated phase: TLS handshake plus first frame.
///
/// This prevents clients from occupying slots without sending data.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Lifetime of an accepted task whose sender never opens a data connection.
///
/// Expired tasks are removed to prevent leaks.
pub const PENDING_TTL: Duration = Duration::from_secs(300);

/// Sweep interval for expired tasks.
pub const PENDING_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

// ---- Receiver PIN gate ----

/// Rate-limit window for PIN brute-force attempts.
pub const PIN_WINDOW: Duration = Duration::from_secs(60);

/// Maximum failed attempts per window.
///
/// Once reached, the source is rejected for the rest of the window.
pub const PIN_MAX_FAILURES: u32 = 5;

/// Maximum number of sources, keyed by TLS fingerprint, tracked for PIN failures.
///
/// New sources are conservatively rejected beyond this limit to bound the table.
pub const PIN_TRACK_CAP: usize = 1024;
