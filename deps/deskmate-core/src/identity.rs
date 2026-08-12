//! Device identity layer: who this device is and how it proves its identity.
//!
//! - The first launch creates a persistent UUID and self-signed X.509 certificate
//!   with rcgen and stores them in the data directory.
//! - The certificate's BLAKE3 fingerprint is the unique network identity; MAC
//!   addresses are not used because modern systems randomize them, restrict access,
//!   and expose privacy concerns.
//! - The display name defaults to the hostname. LAN IP addresses are informational
//!   only, so address changes do not affect identity.
//! - The trust model uses mutual TLS 1.3 authentication and trust on first use
//!   (TOFU); see [`crate::tls`].

use std::fs;
use std::path::Path;

use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Metadata filename for the UUID and display name.
const META_FILE: &str = "identity.json";
/// Filename of the DER-encoded self-signed certificate.
const CERT_FILE: &str = "cert.der";
/// Filename of the DER-encoded PKCS#8 private key.
const KEY_FILE: &str = "key.der";

/// Identity layer errors.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// Failed to read or write identity files.
    #[error("failed to read or write identity files: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to generate a certificate or key.
    #[error("failed to generate certificate: {0}")]
    CertGen(#[from] rcgen::Error),
    /// Failed to parse `identity.json`.
    #[error("failed to parse identity metadata: {0}")]
    Meta(#[from] serde_json::Error),
}

/// Metadata persisted in `identity.json`.
#[derive(Debug, Serialize, Deserialize)]
struct IdentityMeta {
    /// Unique device ID (UUID v4).
    device_id: String,
    /// User-defined display name; `None` follows the hostname.
    display_name: Option<String>,
}

/// Device identity: unique identifiers and TLS certificate material.
#[derive(Debug)]
pub struct DeviceIdentity {
    /// Unique device ID generated as a UUID v4 on first launch.
    pub device_id: String,
    /// Public display name, user-editable and defaulting to the hostname.
    pub display_name: String,
    /// Built-in emoji avatar injected by the application at runtime, not persisted here.
    pub avatar: Option<String>,
    /// Lowercase hexadecimal BLAKE3 certificate fingerprint used as the network identity.
    pub fingerprint: String,
    /// DER-encoded self-signed certificate presented during the TLS handshake.
    pub cert_der: CertificateDer<'static>,
    /// DER-encoded PKCS#8 private key, exposed only as a copy through [`Self::key_der`].
    key_der: PrivateKeyDer<'static>,
}

impl DeviceIdentity {
    /// Loads the identity from the data directory.
    ///
    /// A new identity is generated and persisted if any of the three files is missing.
    pub fn load_or_create(dir: &Path) -> Result<Self, IdentityError> {
        let complete = [META_FILE, CERT_FILE, KEY_FILE]
            .iter()
            .all(|f| dir.join(f).exists());
        if complete {
            Self::load(dir)
        } else {
            Self::create(dir)
        }
    }

    /// Loads an existing identity from the data directory.
    fn load(dir: &Path) -> Result<Self, IdentityError> {
        let meta: IdentityMeta = serde_json::from_slice(&fs::read(dir.join(META_FILE))?)?;
        let cert_der = CertificateDer::from(fs::read(dir.join(CERT_FILE))?);
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(fs::read(dir.join(KEY_FILE))?));
        Ok(Self::from_parts(
            meta.device_id,
            meta.display_name.unwrap_or_else(default_display_name),
            cert_der,
            key_der,
        ))
    }

    /// Generates a new UUID and self-signed certificate and writes them to the data directory.
    fn create(dir: &Path) -> Result<Self, IdentityError> {
        let key_pair = rcgen::KeyPair::generate()?;
        let params = rcgen::CertificateParams::new(vec!["deskmate".to_string()])?;
        let cert = params.self_signed(&key_pair)?;
        let cert_der = cert.der().clone();
        let key_bytes = key_pair.serialize_der();
        let device_id = uuid::Uuid::new_v4().to_string();

        let meta = IdentityMeta {
            device_id: device_id.clone(),
            display_name: None,
        };
        fs::create_dir_all(dir)?;
        fs::write(dir.join(META_FILE), serde_json::to_vec_pretty(&meta)?)?;
        fs::write(dir.join(CERT_FILE), cert_der.as_ref())?;
        fs::write(dir.join(KEY_FILE), &key_bytes)?;
        tracing::info!(%device_id, "generated a new device identity");

        Ok(Self::from_parts(
            device_id,
            default_display_name(),
            cert_der,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bytes)),
        ))
    }

    /// Builds an identity from existing material.
    ///
    /// The fingerprint is calculated here, while the application injects the
    /// non-persistent avatar at runtime.
    fn from_parts(
        device_id: String,
        display_name: String,
        cert_der: CertificateDer<'static>,
        key_der: PrivateKeyDer<'static>,
    ) -> Self {
        Self {
            fingerprint: fingerprint_of(&cert_der),
            avatar: None,
            device_id,
            display_name,
            cert_der,
            key_der,
        }
    }

    /// Returns a private-key copy because rustls configuration takes ownership.
    pub fn key_der(&self) -> PrivateKeyDer<'static> {
        self.key_der.clone_key()
    }

    /// Builds the device information exchanged during handshakes and discovery.
    pub fn peer_info(&self) -> crate::protocol::PeerInfo {
        crate::protocol::PeerInfo {
            device_id: self.device_id.clone(),
            name: self.display_name.clone(),
            fingerprint: self.fingerprint.clone(),
            platform: platform(),
            avatar: self.avatar.clone(),
            os_version: Some(os_version().to_string()),
        }
    }
}

/// Returns the local platform identifier: `macos`, `windows`, or `linux`.
pub fn platform() -> String {
    std::env::consts::OS.to_string()
}

/// Returns the local operating-system version, for example `"macOS 15.3.1"`.
///
/// Detection requires system calls, so `OnceLock` caches the result for the
/// process lifetime because `peer_info` runs frequently on heartbeat and handshake paths.
pub fn os_version() -> &'static str {
    static OS_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OS_VERSION.get_or_init(|| {
        let info = os_info::get();
        // Normalize os_info's "Mac OS" label to Apple's official spelling.
        let name = match info.os_type() {
            os_info::Type::Macos => "macOS".to_string(),
            t => t.to_string(),
        };
        format!("{} {}", name, info.version())
    })
}

/// Calculates the lowercase hexadecimal `BLAKE3(cert_der)` certificate fingerprint.
///
/// The fingerprint is only an internal Deskmate device identifier and does not
/// need cross-tool interoperability, so the existing, faster BLAKE3 dependency
/// is used instead of adding SHA-256.
pub fn fingerprint_of(cert: &CertificateDer<'_>) -> String {
    blake3::hash(cert.as_ref()).to_hex().to_string()
}

/// Returns the hostname as the default display name, falling back to `"deskmate"`.
fn default_display_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "deskmate".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Isolated temporary directory removed automatically on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("deskmate-id-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Reloading a generated identity preserves its persisted values.
    #[test]
    fn create_then_load_is_stable() {
        let dir = TempDir::new();
        let a = DeviceIdentity::load_or_create(&dir.0).unwrap();
        let b = DeviceIdentity::load_or_create(&dir.0).unwrap();
        assert_eq!(a.device_id, b.device_id);
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    /// The BLAKE3 fingerprint is 64 lowercase hexadecimal characters.
    #[test]
    fn fingerprint_is_hex64() {
        let dir = TempDir::new();
        let id = DeviceIdentity::load_or_create(&dir.0).unwrap();
        assert_eq!(id.fingerprint.len(), 64);
        assert!(
            id.fingerprint
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// Identities generated in different directories must be unique.
    #[test]
    fn identities_are_unique() {
        let (d1, d2) = (TempDir::new(), TempDir::new());
        let a = DeviceIdentity::load_or_create(&d1.0).unwrap();
        let b = DeviceIdentity::load_or_create(&d2.0).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
        assert_ne!(a.device_id, b.device_id);
    }

    /// OS version detection returns a non-empty description for identity broadcasts.
    #[test]
    fn os_version_is_detected() {
        let v = os_version();
        println!("detected os version: {v}");
        assert!(!v.trim().is_empty());
    }
}
