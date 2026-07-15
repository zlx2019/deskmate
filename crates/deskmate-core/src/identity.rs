//! 设备身份层: 我是谁, 以及如何证明我是我
//!
//! - 首次启动生成持久化 UUID + 自签 X.509 证书(rcgen), 存于数据目录
//! - 证书 BLAKE3 指纹作为设备唯一网络身份 —— 不使用 MAC 地址
//!   (现代系统默认 MAC 随机化, 读取需额外权限, 且有隐私争议)
//! - 展示名默认取 hostname, 内网 IP 仅作展示用途, IP 变化不影响身份
//! - 信任模型: TLS 1.3 双向认证 + TOFU(首次连接确认), 见 [`crate::tls`]

use std::fs;
use std::path::Path;

use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 元数据文件名(UUID、昵称)
const META_FILE: &str = "identity.json";
/// DER 编码自签证书文件名
const CERT_FILE: &str = "cert.der";
/// DER 编码 PKCS#8 私钥文件名
const KEY_FILE: &str = "key.der";

/// 身份层错误
#[derive(Debug, Error)]
pub enum IdentityError {
    /// 身份文件读写失败
    #[error("身份文件读写失败: {0}")]
    Io(#[from] std::io::Error),
    /// 证书或密钥生成失败
    #[error("证书生成失败: {0}")]
    CertGen(#[from] rcgen::Error),
    /// 身份元数据(identity.json)解析失败
    #[error("身份元数据解析失败: {0}")]
    Meta(#[from] serde_json::Error),
}

/// 持久化在 identity.json 中的元数据
#[derive(Debug, Serialize, Deserialize)]
struct IdentityMeta {
    /// 设备唯一 ID(UUID v4)
    device_id: String,
    /// 用户自定义昵称, None 表示跟随 hostname
    display_name: Option<String>,
}

/// 设备身份: 唯一标识 + TLS 证书材料
#[derive(Debug)]
pub struct DeviceIdentity {
    /// 设备唯一 ID(首次启动生成的 UUID v4)
    pub device_id: String,
    /// 对外展示名(用户可改, 默认跟随 hostname)
    pub display_name: String,
    /// 内置头像(emoji); 运行时由上层应用设置注入, 不在此持久化
    pub avatar: Option<String>,
    /// 证书 BLAKE3 指纹(hex 小写), 作为网络中的设备身份
    pub fingerprint: String,
    /// DER 编码的自签证书(TLS 握手时出示)
    pub cert_der: CertificateDer<'static>,
    /// DER 编码的 PKCS#8 私钥(不对外暴露, 经 [`Self::key_der`] 取副本)
    key_der: PrivateKeyDer<'static>,
}

impl DeviceIdentity {
    /// 从数据目录加载身份; 三个身份文件不齐全则生成新身份并落盘
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

    /// 加载数据目录中的既有身份
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

    /// 生成新身份(UUID + 自签证书)并写入数据目录
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
        tracing::info!(%device_id, "已生成新设备身份");

        Ok(Self::from_parts(
            device_id,
            default_display_name(),
            cert_der,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bytes)),
        ))
    }

    /// 由既有材料组装身份: 指纹在此统一计算, 头像由上层运行时注入(不持久化)
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

    /// 取私钥副本(rustls 构建配置需要所有权)
    pub fn key_der(&self) -> PrivateKeyDer<'static> {
        self.key_der.clone_key()
    }

    /// 构造握手与发现时交换的设备信息
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

/// 本机平台标识(macos / windows / linux)
pub fn platform() -> String {
    std::env::consts::OS.to_string()
}

/// 本机操作系统版本描述(如 "Mac OS 15.3.1")
///
/// 检测有系统调用开销, OnceLock 缓存进程生命周期内的结果
/// (peer_info 在心跳/握手路径被频繁调用)。
pub fn os_version() -> &'static str {
    static OS_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OS_VERSION.get_or_init(|| {
        let info = os_info::get();
        format!("{} {}", info.os_type(), info.version())
    })
}

/// 计算证书指纹: BLAKE3(cert_der) 的 hex 小写
///
/// 指纹仅作为 deskmate 内部设备标识, 无跨工具互操作需求,
/// 故选用已引入且更快的 BLAKE3 而非 SHA-256, 避免额外依赖。
pub fn fingerprint_of(cert: &CertificateDer<'_>) -> String {
    blake3::hash(cert.as_ref()).to_hex().to_string()
}

/// 默认展示名: 取 hostname, 失败时退回固定名 "deskmate"
fn default_display_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "deskmate".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立临时目录, Drop 时自动清理
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

    /// 首次生成后再次加载, 身份信息应保持一致(持久化正确)
    #[test]
    fn create_then_load_is_stable() {
        let dir = TempDir::new();
        let a = DeviceIdentity::load_or_create(&dir.0).unwrap();
        let b = DeviceIdentity::load_or_create(&dir.0).unwrap();
        assert_eq!(a.device_id, b.device_id);
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    /// 指纹应为 BLAKE3 的 64 位 hex 小写
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

    /// 不同目录生成的两个身份, 指纹必须不同
    #[test]
    fn identities_are_unique() {
        let (d1, d2) = (TempDir::new(), TempDir::new());
        let a = DeviceIdentity::load_or_create(&d1.0).unwrap();
        let b = DeviceIdentity::load_or_create(&d2.0).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
        assert_ne!(a.device_id, b.device_id);
    }

    /// OS 版本检测应产出非空描述并随身份广播
    #[test]
    fn os_version_is_detected() {
        let v = os_version();
        println!("detected os version: {v}");
        assert!(!v.trim().is_empty());
    }
}
