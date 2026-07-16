//! deskmate-core: 局域网传输核心引擎
//!
//! 与 UI 完全解耦的纯库, 供 CLI(联调验证)与 Tauri 应用(桌面端)复用。
//! 整体分层设计(详见 docs/PLAN.md 第 4 节):
//!
//! ```text
//! ┌─ discovery ─ 节点发现: mDNS 主通道 + UDP 组播兜底
//! ├─ identity  ─ 设备身份: UUID + 自签证书指纹(不依赖 MAC 地址)
//! ├─ tls       ─ TLS 1.3 双向认证: 指纹 pin + TOFU, 不走 CA 体系
//! ├─ protocol  ─ 控制协议: TCP + TLS 1.3, 长度前缀 JSON 帧
//! └─ transfer  ─ 数据传输: 分块流式收发, 支持暂停/继续/取消/断点续传
//! ```

pub mod config;
pub mod discovery;
pub mod identity;
pub mod protocol;
pub mod tls;
pub mod transfer;

/// 协议版本号(major.minor), 握手阶段协商, major 不同则拒绝通信
/// (1.1: 头像拉取 AvatarRequest/AvatarResponse; 1.2: PIN 配对与 TextRejected;
/// 1.3: PeerInfo 可选 os_version 字段; 1.4: 双向主动推送 Pause/Resume/Cancel
/// 帧(对端暂停可感知 —— 帧本身 1.0 起即有定义, 旧版本能正确响应只是不发)
/// 与 TransferResponse 可选 reason_code(结构化拒因, 发送端按本机语言渲染);
/// minor 演进均向后兼容)
pub const PROTOCOL_VERSION: &str = "1.4";

/// 默认 TCP 监听端口(控制通道与数据通道复用, 握手时区分), 可在设置中修改
pub const DEFAULT_TCP_PORT: u16 = 42424;

/// 默认 UDP 组播发现端口(mDNS 之外的兜底通道), 可在设置中修改
pub const DEFAULT_DISCOVERY_PORT: u16 = 42425;

#[cfg(test)]
mod tests {
    use super::*;

    /// 协议版本必须是 major.minor 两段式, 保证握手协商逻辑可解析
    #[test]
    fn protocol_version_format() {
        let parts: Vec<&str> = PROTOCOL_VERSION.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()));
    }

    /// 控制端口与发现端口不能冲突
    #[test]
    fn default_ports_distinct() {
        assert_ne!(DEFAULT_TCP_PORT, DEFAULT_DISCOVERY_PORT);
    }
}
