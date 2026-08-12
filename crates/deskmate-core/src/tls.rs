//! TLS layer: mutual TLS 1.3 authentication with self-signed certificates and TOFU.
//!
//! This does not use a certificate authority:
//! - The client pins the server certificate to a fingerprint obtained through
//!   discovery or user confirmation.
//! - The server requires a client certificate without CA validation, then the
//!   upper layer compares its fingerprint for trust on first use.
//! - Direct-IP CLI integration may accept any certificate, but the upper layer
//!   must show the peer fingerprint to the user.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use thiserror::Error;

use crate::identity::{DeviceIdentity, fingerprint_of};

/// TLS layer errors.
#[derive(Debug, Error)]
pub enum TlsError {
    /// Failed to build rustls configuration, for example due to an invalid certificate or key.
    #[error("failed to build TLS configuration: {0}")]
    Config(#[from] rustls::Error),
}

/// Builds server TLS configuration that presents the local certificate and
/// requires a client certificate without CA validation, leaving TOFU to the upper layer.
pub fn server_config(identity: &DeviceIdentity) -> Result<ServerConfig, TlsError> {
    let config = ServerConfig::builder()
        .with_client_cert_verifier(Arc::new(AcceptAnyClientCert::new()))
        .with_single_cert(vec![identity.cert_der.clone()], identity.key_der())?;
    Ok(config)
}

/// Builds client TLS configuration and presents the local certificate for mutual authentication.
///
/// `Some(expected_fingerprint)` strictly pins the peer certificate. `None`
/// accepts any certificate and is only for direct CLI integration, where the
/// upper layer must display the actual fingerprint for verification.
pub fn client_config(
    identity: &DeviceIdentity,
    expected_fingerprint: Option<String>,
) -> Result<ClientConfig, TlsError> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServerCert::new(expected_fingerprint)))
        .with_client_auth_cert(vec![identity.cert_der.clone()], identity.key_der())?;
    Ok(config)
}

/// Calculates a fingerprint from the end-entity certificate in a TLS peer chain.
pub fn peer_fingerprint(certs: Option<&[CertificateDer<'_>]>) -> Option<String> {
    certs.and_then(|c| c.first()).map(fingerprint_of)
}

/// Verifies a TLS 1.2 handshake signature with the given provider.
fn verify_sig_tls12(
    provider: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls12_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

/// Verifies a TLS 1.3 handshake signature with the given provider.
fn verify_sig_tls13(
    provider: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

/// Returns signature schemes supported by the provider.
fn supported_schemes(provider: &CryptoProvider) -> Vec<SignatureScheme> {
    provider
        .signature_verification_algorithms
        .supported_schemes()
}

/// Process-wide shared cryptographic provider.
///
/// Building algorithm tables has a fixed cost, while every control, data, or
/// avatar connection constructs verifiers and rustls configuration. Sharing one
/// provider avoids rebuilding those tables.
fn shared_provider() -> Arc<CryptoProvider> {
    static PROVIDER: std::sync::OnceLock<Arc<CryptoProvider>> = std::sync::OnceLock::new();
    Arc::clone(PROVIDER.get_or_init(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider())))
}

/// Client-side verifier that pins the server certificate by fingerprint.
#[derive(Debug)]
struct PinnedServerCert {
    /// Expected peer certificate fingerprint; `None` accepts any certificate.
    expected: Option<String>,
    /// Cryptographic provider used for TLS signature verification.
    provider: Arc<CryptoProvider>,
}

impl PinnedServerCert {
    /// Creates a verifier using the process-wide provider.
    fn new(expected: Option<String>) -> Self {
        Self {
            expected,
            provider: shared_provider(),
        }
    }
}

impl ServerCertVerifier for PinnedServerCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.expected {
            Some(expected) => {
                let actual = fingerprint_of(end_entity);
                if &actual == expected {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(rustls::Error::General(format!(
                        "peer certificate fingerprint mismatch: {actual}"
                    )))
                }
            }
            // Integration mode skips pinning; the upper layer displays the fingerprint.
            None => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_sig_tls12(&self.provider, message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_sig_tls13(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_schemes(&self.provider)
    }
}

/// Server-side verifier that accepts any client certificate and leaves TOFU to the upper layer.
#[derive(Debug)]
struct AcceptAnyClientCert {
    /// Cryptographic provider used for TLS signature verification.
    provider: Arc<CryptoProvider>,
}

impl AcceptAnyClientCert {
    /// Creates a verifier using the process-wide provider.
    fn new() -> Self {
        Self {
            provider: shared_provider(),
        }
    }
}

impl ClientCertVerifier for AcceptAnyClientCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        // Certificate trust is decided by the upper layer through fingerprint TOFU.
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_sig_tls12(&self.provider, message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_sig_tls13(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_schemes(&self.provider)
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    use super::*;

    /// Isolated temporary directory removed automatically on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let p =
                std::env::temp_dir().join(format!("deskmate-tls-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A localhost handshake succeeds with the correct pin and exposes both peer fingerprints.
    #[tokio::test]
    async fn handshake_with_pinned_fingerprint() {
        let (d1, d2) = (TempDir::new(), TempDir::new());
        let server_id = Arc::new(DeviceIdentity::load_or_create(&d1.0).unwrap());
        let client_id = Arc::new(DeviceIdentity::load_or_create(&d2.0).unwrap());

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_config(&server_id).unwrap()));

        let client_fp = client_id.fingerprint.clone();
        let server_task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            // The server obtains the client certificate and derives its TOFU fingerprint.
            let fp = peer_fingerprint(tls.get_ref().1.peer_certificates()).unwrap();
            assert_eq!(fp, client_fp);
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
        });

        let connector = TlsConnector::from(Arc::new(
            client_config(&client_id, Some(server_id.fingerprint.clone())).unwrap(),
        ));
        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = ServerName::try_from("deskmate").unwrap();
        let mut tls = connector.connect(name, tcp).await.unwrap();
        tls.write_all(b"ping").await.unwrap();
        tls.flush().await.unwrap();
        server_task.await.unwrap();
    }

    /// The client handshake fails when the pinned fingerprint is wrong.
    #[tokio::test]
    async fn handshake_rejects_wrong_fingerprint() {
        let (d1, d2) = (TempDir::new(), TempDir::new());
        let server_id = Arc::new(DeviceIdentity::load_or_create(&d1.0).unwrap());
        let client_id = Arc::new(DeviceIdentity::load_or_create(&d2.0).unwrap());

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_config(&server_id).unwrap()));
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                // The handshake is expected to fail, so ignore the result.
                let _ = acceptor.accept(tcp).await;
            }
        });

        let wrong_fp = "0".repeat(64);
        let connector =
            TlsConnector::from(Arc::new(client_config(&client_id, Some(wrong_fp)).unwrap()));
        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = ServerName::try_from("deskmate").unwrap();
        assert!(connector.connect(name, tcp).await.is_err());
    }
}
