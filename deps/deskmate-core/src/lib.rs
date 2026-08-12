//! deskmate-core: LAN transfer engine.
//!
//! This UI-independent library is shared by the integration CLI and the Tauri
//! desktop application. The layered design is described in `docs/PLAN.md` section 4:
//!
//! ```text
//! |- discovery - peer discovery: primary mDNS plus UDP multicast fallback
//! |- identity  - device identity: UUID plus self-signed certificate fingerprint
//! |- tls       - mutual TLS 1.3 authentication: fingerprint pinning and TOFU
//! |- protocol  - control protocol: length-prefixed JSON frames over TCP and TLS 1.3
//! `- transfer  - chunked streaming with pause, resume, cancel, and recovery support
//! ```

pub mod config;
pub mod discovery;
pub mod identity;
pub mod protocol;
pub mod tls;
pub mod transfer;

/// Protocol version (`major.minor`) negotiated during the handshake.
///
/// Different major versions cannot communicate. Version 1.1 added avatar
/// requests, 1.2 added PIN pairing and `TextRejected`, 1.3 added the optional
/// `PeerInfo::os_version`, and 1.4 added bidirectional Pause/Resume/Cancel pushes
/// plus the optional structured `TransferResponse::reason_code`. The control
/// frames existed in 1.0, so older versions can respond but do not initiate them.
/// 1.5 added the optional `FileMeta::inline_image` clipboard-image marker.
/// Minor-version evolution remains backward compatible.
pub const PROTOCOL_VERSION: &str = "1.5";

/// Default TCP listening port shared by control and data channels and configurable in settings.
pub const DEFAULT_TCP_PORT: u16 = 42424;

/// Default configurable UDP multicast discovery port used as an mDNS fallback.
pub const DEFAULT_DISCOVERY_PORT: u16 = 42425;

#[cfg(test)]
mod tests {
    use super::*;

    /// The protocol version uses a parseable `major.minor` format.
    #[test]
    fn protocol_version_format() {
        let parts: Vec<&str> = PROTOCOL_VERSION.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()));
    }

    /// The control and discovery ports must not conflict.
    #[test]
    fn default_ports_distinct() {
        assert_ne!(DEFAULT_TCP_PORT, DEFAULT_DISCOVERY_PORT);
    }
}
