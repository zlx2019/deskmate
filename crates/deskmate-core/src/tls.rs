//! TLS 层: 自签证书 + 指纹校验(TOFU)的 TLS 1.3 双向认证
//!
//! 不走 CA 体系:
//! - 客户端按"指纹 pin"校验服务端证书(期望指纹来自发现层或用户确认)
//! - 服务端要求客户端出示证书但不做 CA 校验, 握手后由上层比对指纹执行 TOFU
//! - CLI 直连 IP 的联调场景允许"接受任意证书", 但上层必须向用户展示对端指纹

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

/// TLS 层错误
#[derive(Debug, Error)]
pub enum TlsError {
    /// rustls 配置构建失败(证书/私钥非法等)
    #[error("TLS 配置构建失败: {0}")]
    Config(#[from] rustls::Error),
}

/// 构建服务端 TLS 配置: 出示本机证书, 要求客户端出示证书(不做 CA 校验, 交上层 TOFU)
pub fn server_config(identity: &DeviceIdentity) -> Result<ServerConfig, TlsError> {
    let config = ServerConfig::builder()
        .with_client_cert_verifier(Arc::new(AcceptAnyClientCert::new()))
        .with_single_cert(vec![identity.cert_der.clone()], identity.key_der())?;
    Ok(config)
}

/// 构建客户端 TLS 配置, 同时出示本机证书完成双向认证
///
/// `expected_fingerprint` 为 Some 时严格 pin 对端证书指纹;
/// None 表示接受任意证书 —— 仅限 CLI 直连联调, 上层必须展示实际指纹供用户核对
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

/// 从 TLS 连接的对端证书链计算指纹(取 end-entity 证书)
pub fn peer_fingerprint(certs: Option<&[CertificateDer<'_>]>) -> Option<String> {
    certs.and_then(|c| c.first()).map(fingerprint_of)
}

/// 用指定 provider 校验 TLS 1.2 握手签名(两个 verifier 的公共实现)
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

/// 用指定 provider 校验 TLS 1.3 握手签名(两个 verifier 的公共实现)
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

/// provider 支持的签名方案列表(两个 verifier 的公共实现)
fn supported_schemes(provider: &CryptoProvider) -> Vec<SignatureScheme> {
    provider
        .signature_verification_algorithms
        .supported_schemes()
}

/// 进程级共享的加密算法 Provider
///
/// 算法表构造有固定开销, 而每次建连(控制/数据/头像拉取)都要装配
/// verifier 与 rustls 配置, 共享一份避免重复构造。
fn shared_provider() -> Arc<CryptoProvider> {
    static PROVIDER: std::sync::OnceLock<Arc<CryptoProvider>> = std::sync::OnceLock::new();
    Arc::clone(PROVIDER.get_or_init(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider())))
}

/// 客户端侧校验器: 按指纹 pin 服务端证书
#[derive(Debug)]
struct PinnedServerCert {
    /// 期望的对端证书指纹; None 表示接受任意证书(联调模式)
    expected: Option<String>,
    /// 加密算法提供者(TLS 签名校验用)
    provider: Arc<CryptoProvider>,
}

impl PinnedServerCert {
    /// 创建校验器, 复用进程级共享 provider
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
                        "对端证书指纹不匹配: {actual}"
                    )))
                }
            }
            // 联调模式: 不校验指纹, 上层负责展示给用户核对
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

/// 服务端侧校验器: 接受任意客户端证书, 指纹交由上层做 TOFU 判定
#[derive(Debug)]
struct AcceptAnyClientCert {
    /// 加密算法提供者(TLS 签名校验用)
    provider: Arc<CryptoProvider>,
}

impl AcceptAnyClientCert {
    /// 创建校验器, 复用进程级共享 provider
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
        // 证书合法性不在此校验, 上层通过指纹 + TOFU 决定信任
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

    /// 独立临时目录, Drop 时自动清理
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

    /// localhost 回环: pin 正确指纹时握手成功, 且双方能互取对端证书指纹
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
            // 服务端应能取到客户端证书并算出与其身份一致的指纹(TOFU 依据)
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

    /// pin 错误指纹时, 客户端握手必须失败
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
                // 握手预期失败, 忽略结果
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
