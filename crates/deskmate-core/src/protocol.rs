//! 会话层: 控制通道帧协议(自研, 方案决策 #2)
//!
//! 帧格式: 4 字节大端长度前缀 + JSON body。
//! 控制通道与数据通道复用同一 TCP 监听端口, 由连接首帧区分:
//! `Hello` 开启控制会话, `DataHello` 开启数据流。
//!
//! 传输状态机:
//! ```text
//! Idle → Requested → Accepted | Rejected
//! Accepted → Transferring ⇄ Paused
//! Transferring → Completed
//!              | Cancelled   (主动取消: 删除 .part 临时文件)
//!              | Interrupted (意外断连: 保留 .part, 重连后询问续传)
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::PROTOCOL_VERSION;

/// 单帧最大长度(1 MiB), 防御恶意超长帧打爆内存
pub const MAX_FRAME_LEN: u32 = 1024 * 1024;

/// 头像图片字节数上限(256 KiB; 128×128 JPEG 正常在 20 KiB 内)
pub const MAX_AVATAR_SIZE: u64 = 256 * 1024;

/// 协议层错误
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// 底层 IO 失败(含对端断连)
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    /// 帧长度超出 [`MAX_FRAME_LEN`]
    #[error("帧长度 {0} 字节超过上限 {MAX_FRAME_LEN}")]
    FrameTooLarge(u32),
    /// 消息 JSON 编解码失败
    #[error("消息编解码失败: {0}")]
    Codec(#[from] serde_json::Error),
    /// 协议 major 版本不一致, 拒绝通信
    #[error("协议版本不兼容: 对端 {peer}, 本机 {local}")]
    VersionMismatch {
        /// 对端版本
        peer: String,
        /// 本机版本
        local: String,
    },
    /// 收到不符合当前会话状态的消息
    #[error("意外的消息: 期望 {expected}, 收到 {got}")]
    Unexpected {
        /// 期望的消息类型
        expected: &'static str,
        /// 实际收到的消息描述
        got: String,
    },
}

/// 设备信息(握手阶段交换)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// 设备唯一 ID(UUID)
    pub device_id: String,
    /// 展示名
    pub name: String,
    /// 证书 BLAKE3 指纹(hex)
    pub fingerprint: String,
    /// 平台标识(macos/windows/linux)
    pub platform: String,
    /// 内置头像(emoji); None 时 UI 回退首字母样式。
    /// serde default 保证与不带该字段的旧版本协议兼容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// 操作系统版本描述(如 "Mac OS 15.3.1"); 协议 1.3 起, 旧版本可缺省
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
}

/// 文件元数据(TransferRequest 清单项)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    /// 传输内文件序号, 数据通道以它对应文件
    pub file_id: u32,
    /// 相对路径(统一 `/` 分隔; 接收端拼接前必须 sanitize)
    pub rel_path: String,
    /// 文件字节数
    pub size: u64,
}

/// 单文件断点状态(ResumeInfo 清单项)
///
/// `rel_path`/`size` 供发送端与本地清单比对: 路径或大小不匹配
/// 说明源文件已变化, 整文件哈希无法衔接, 该任务不可续传。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeFileState {
    /// 文件序号(沿用原 TransferRequest 的编号)
    pub file_id: u32,
    /// 相对路径
    pub rel_path: String,
    /// 文件总字节数
    pub size: u64,
    /// 接收端已落盘的字节数(从此偏移续传)
    pub received: u64,
}

/// 控制/数据通道消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// 控制会话握手(发起方 → 接收方)
    Hello {
        /// 协议版本(major.minor)
        version: String,
        /// 发起方设备信息
        info: PeerInfo,
    },
    /// 握手应答(接收方设备信息)
    HelloAck {
        /// 协议版本(major.minor)
        version: String,
        /// 接收方设备信息
        info: PeerInfo,
    },
    /// 传输请求: 只发元数据清单, 等待接收方决策
    TransferRequest {
        /// 传输任务 ID(UUID)
        transfer_id: String,
        /// 文件清单
        files: Vec<FileMeta>,
        /// 总字节数
        total_size: u64,
        /// 配对 PIN(接收方启用 PIN 时必须携带且正确, 否则直接拒绝)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<String>,
    },
    /// 传输应答: `accepted_files` 为空即拒绝
    TransferResponse {
        /// 对应的传输任务 ID
        transfer_id: String,
        /// 接受的文件序号列表(支持部分接受)
        accepted_files: Vec<u32>,
        /// 拒绝原因(可选)
        reason: Option<String>,
        /// 拒因是 PIN 缺失或错误(发送端据此弹 PIN 输入重试)
        #[serde(default)]
        pin_required: bool,
    },
    /// 文本消息: 逐字节一致送达, 不 trim、不转义
    Text {
        /// 文本内容(UTF-8)
        text: String,
        /// 配对 PIN(同 TransferRequest)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<String>,
    },
    /// 文本已收到
    TextAck,
    /// 文本被拒(1.2 新增; 目前唯一拒因是 PIN 校验失败)
    TextRejected {
        /// 拒因是 PIN 缺失或错误
        pin_required: bool,
    },
    /// 暂停指定传输(双端均可发起)
    Pause {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 恢复指定传输
    Resume {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 取消指定传输(接收端删除 .part 临时文件)
    Cancel {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 数据通道首帧: 声明本连接承载哪个传输任务
    DataHello {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 数据通道文件头: 其后紧跟 `size - offset` 字节的原始文件内容
    FileHeader {
        /// 文件序号(对应 TransferRequest 清单)
        file_id: u32,
        /// 起始偏移(M1 恒为 0, 断点续传时为已收字节数)
        offset: u64,
    },
    /// 数据通道文件尾: 整文件 BLAKE3 校验值
    FileFooter {
        /// 文件序号
        file_id: u32,
        /// 整文件 BLAKE3 哈希(hex)
        hash: String,
    },
    /// 断点协商: 意外断连后发送方重连询问指定任务的已收进度(方案决策 #3)
    ResumeQuery {
        /// 原传输任务 ID
        transfer_id: String,
    },
    /// 断点应答: 未完成文件的已收字节清单; 任务不可续传(元数据丢失/
    /// 已完成/身份不符)时 `files` 为空
    ResumeInfo {
        /// 对应的传输任务 ID
        transfer_id: String,
        /// 可续传文件清单(已完整落盘的文件不在其中)
        files: Vec<ResumeFileState>,
    },
    /// 请求对端的头像图片(对端广播 `img:<hash>` 且本地缓存未命中时发起)
    AvatarRequest,
    /// 头像应答: 帧后紧跟 `size` 字节的图片原始数据
    AvatarResponse {
        /// 图片数据的 BLAKE3 哈希(hex 小写; 未设置头像时为空串)
        hash: String,
        /// 图片字节数(0 表示未设置头像, 帧后无数据)
        size: u64,
    },
    /// 数据通道全部文件发送完毕
    DataDone,
    /// 优雅关闭会话
    Bye,
}

impl ControlMessage {
    /// 消息类型短名(日志与错误信息用)
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::HelloAck { .. } => "hello_ack",
            Self::TransferRequest { .. } => "transfer_request",
            Self::TransferResponse { .. } => "transfer_response",
            Self::Text { .. } => "text",
            Self::TextAck => "text_ack",
            Self::TextRejected { .. } => "text_rejected",
            Self::Pause { .. } => "pause",
            Self::Resume { .. } => "resume",
            Self::Cancel { .. } => "cancel",
            Self::DataHello { .. } => "data_hello",
            Self::FileHeader { .. } => "file_header",
            Self::FileFooter { .. } => "file_footer",
            Self::ResumeQuery { .. } => "resume_query",
            Self::ResumeInfo { .. } => "resume_info",
            Self::AvatarRequest => "avatar_request",
            Self::AvatarResponse { .. } => "avatar_response",
            Self::DataDone => "data_done",
            Self::Bye => "bye",
        }
    }
}

/// 写一帧: 4 字节大端长度 + JSON body, 随后 flush
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    msg: &ControlMessage,
) -> Result<(), ProtocolError> {
    let body = serde_json::to_vec(msg)?;
    let len = u32::try_from(body.len()).map_err(|_| ProtocolError::FrameTooLarge(u32::MAX))?;
    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// 读一帧并解码; 超长帧直接报错断开, 不读取其内容
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<ControlMessage, ProtocolError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

/// 校验对端协议版本: major 相同即视为兼容
pub fn check_version(peer_version: &str) -> Result<(), ProtocolError> {
    if major_of(peer_version) == major_of(PROTOCOL_VERSION) {
        Ok(())
    } else {
        Err(ProtocolError::VersionMismatch {
            peer: peer_version.to_string(),
            local: PROTOCOL_VERSION.to_string(),
        })
    }
}

/// 取版本号的 major 段
fn major_of(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 帧编解码往返: 各类消息经 duplex 管道后应原样还原
    #[tokio::test]
    async fn frame_roundtrip() {
        let samples = vec![
            ControlMessage::Hello {
                version: PROTOCOL_VERSION.to_string(),
                info: PeerInfo {
                    device_id: "d1".into(),
                    name: "n1".into(),
                    fingerprint: "f".repeat(64),
                    platform: "macos".into(),
                    avatar: Some("🦊".into()),
                    os_version: Some("Mac OS 15.3".into()),
                },
            },
            ControlMessage::TransferRequest {
                transfer_id: "t1".into(),
                files: vec![FileMeta {
                    file_id: 0,
                    rel_path: "a/b.txt".into(),
                    size: 42,
                }],
                total_size: 42,
                pin: Some("1234".into()),
            },
            // 文本必须逐字节一致: 刻意包含首尾空白与控制字符
            ControlMessage::Text {
                text: "  你好\n\t emoji🚀 \0 尾巴  ".into(),
                pin: None,
            },
            ControlMessage::DataDone,
        ];
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        for msg in &samples {
            write_frame(&mut a, msg).await.unwrap();
            let got = read_frame(&mut b).await.unwrap();
            assert_eq!(
                serde_json::to_string(&got).unwrap(),
                serde_json::to_string(msg).unwrap()
            );
        }
    }

    /// 旧版本(无 avatar/os_version 字段)的设备信息必须能解析;
    /// 未设置的可选字段不得序列化(旧对端见到未知字段也能忽略, 但少一事)
    #[test]
    fn peer_info_optional_fields_are_backward_compatible() {
        let legacy = r#"{"device_id":"d","name":"n","fingerprint":"f","platform":"macos"}"#;
        let info: PeerInfo = serde_json::from_str(legacy).unwrap();
        assert_eq!(info.avatar, None);
        assert_eq!(info.os_version, None);
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("avatar"));
        assert!(!json.contains("os_version"));
    }

    /// 文本消息序列化后内容字段不得被改动(逐字节一致性的护栏)
    #[test]
    fn text_is_byte_exact() {
        let raw = "  space  \u{7f} 中文 ";
        let msg = ControlMessage::Text {
            text: raw.into(),
            pin: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        match serde_json::from_str(&json).unwrap() {
            ControlMessage::Text { text, .. } => assert_eq!(text, raw),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 读取侧必须在解析 body 前就拒绝超长帧
    #[tokio::test]
    async fn oversized_frame_rejected() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let bogus_len = (MAX_FRAME_LEN + 1).to_be_bytes();
        tokio::io::AsyncWriteExt::write_all(&mut a, &bogus_len)
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(ProtocolError::FrameTooLarge(_))
        ));
    }

    /// major 相同兼容, 不同则拒绝
    #[test]
    fn version_compat() {
        assert!(check_version("1.0").is_ok());
        assert!(check_version("1.9").is_ok());
        assert!(matches!(
            check_version("2.0"),
            Err(ProtocolError::VersionMismatch { .. })
        ));
    }
}
