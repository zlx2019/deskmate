//! 传输层: 数据通道与文件收发引擎
//!
//! v1 策略(足以跑满 2.5GbE, 详见 docs/PLAN.md 第 4.4 节):
//! - tokio 流式读写, 1 MiB 大块 chunk, 边传边算 BLAKE3
//! - 数据通道形态: `FileHeader` 帧 + 原始字节流 + `FileFooter` 帧, 顺序传输
//! - 暂停/继续/取消经 watch 通道下发, 本端与对端状态合并生效
//! - 断连语义(方案决策 #3): 主动取消删除 `.part`; 意外断连保留以待续传
//!
//! v2 预留: 抽象平台特定后端(Linux io_uring + kTLS / Windows IOCP /
//! macOS F_NOCACHE)与多流并行, 接口形态不变。

mod io_tuning;
mod receiver;
mod sender;

pub use receiver::{OfferDecision, ReceiverHandle, ReceiverOptions, TransferOffer, spawn_receiver};
pub use sender::{SendSummary, fetch_avatar, resume_send, send_files, send_text};

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use crate::protocol::{PeerInfo, ProtocolError};
use crate::tls::TlsError;

/// 数据块大小与决策超时见集中调优模块; re-export 保持既有引用路径
pub use crate::config::CHUNK_SIZE;
pub(crate) use crate::config::{DATA_IDLE_TIMEOUT, OFFER_TIMEOUT};

/// 未完成文件的临时后缀
pub const PART_SUFFIX: &str = ".deskmate.part";

/// 传输层错误
#[derive(Debug, Error)]
pub enum TransferError {
    /// 底层 IO 失败
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    /// 协议帧错误
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// TLS 配置或握手错误
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// 对端声明的身份与其 TLS 证书指纹不一致(疑似冒充)
    #[error("对端身份与证书指纹不一致")]
    PeerMismatch,
    /// 相对路径不安全(绝对路径 / 盘符 / `..` 穿越)
    #[error("不安全的文件路径: {path}")]
    InvalidPath {
        /// 原始路径串
        path: String,
    },
    /// 发送源文件不存在或不可读
    #[error("源文件不可用: {0}")]
    SourceNotFound(PathBuf),
    /// 对端拒绝了传输请求
    #[error("对端拒绝: {}", reason.as_deref().unwrap_or("未给出原因"))]
    Rejected {
        /// 拒绝原因(对端语言的展示文本)
        reason: Option<String>,
        /// 结构化拒因(协议 1.4 起, 供 UI 按本机语言渲染; 旧对端为 None)
        reason_code: Option<String>,
    },
    /// 文件完整性校验失败
    #[error("文件校验失败: {rel_path}")]
    HashMismatch {
        /// 相对路径
        rel_path: String,
    },
    /// 传输被主动取消
    #[error("传输已取消")]
    Cancelled,
    /// 等待超时(握手应答 / 接收方决策)
    #[error("等待超时: {0}")]
    Timeout(&'static str),
    /// 数据通道引用了未知或未被接受的文件序号
    #[error("非法的文件序号: {0}")]
    BadFileId(u32),
    /// 数据通道引用了不存在的传输任务
    #[error("未知的传输任务: {0}")]
    UnknownTransfer(String),
    /// 同一任务已有进行中的数据会话(拒绝并发连接, 防 .part 互写)
    #[error("任务已有进行中的数据会话: {0}")]
    DuplicateDataSession(String),
    /// 断点续传不可用(对端元数据丢失 / 源文件已变化)
    #[error("无法续传: {0}")]
    ResumeUnavailable(String),
    /// 续传声明的断点与接收端 .part 文件实际长度不一致
    #[error("续传断点不一致: 声明 {offset}, .part 实际 {part_len} 字节")]
    ResumeOffsetMismatch {
        /// 发送端声明的断点
        offset: u64,
        /// 接收端 .part 文件当前长度
        part_len: u64,
    },
    /// 对端头像数据超出协议上限
    #[error("头像超出大小上限: {size} 字节")]
    AvatarTooLarge {
        /// 对端声明的字节数
        size: u64,
    },
    /// 对端启用了配对 PIN, 本次请求未提供或不正确
    #[error("对端要求配对 PIN")]
    PinRequired,
}

impl TransferError {
    /// 稳定错误码: 跨语言 UI 按码查本地化文案(Display 的中文面向日志与 CLI)
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Protocol(_) => "protocol",
            Self::Tls(_) => "tls",
            Self::PeerMismatch => "peer_mismatch",
            Self::InvalidPath { .. } => "invalid_path",
            Self::SourceNotFound(_) => "source_not_found",
            Self::Rejected { .. } => "rejected",
            Self::HashMismatch { .. } => "hash_mismatch",
            Self::Cancelled => "cancelled",
            Self::Timeout(_) => "timeout",
            Self::BadFileId(_) => "bad_file_id",
            Self::UnknownTransfer(_) => "unknown_transfer",
            Self::DuplicateDataSession(_) => "duplicate_data_session",
            Self::ResumeUnavailable(_) => "resume_unavailable",
            Self::ResumeOffsetMismatch { .. } => "resume_offset_mismatch",
            Self::AvatarTooLarge { .. } => "avatar_too_large",
            Self::PinRequired => "pin_required",
        }
    }

    /// 关键细节参数(可选): 附在本地化主句之后展示, 内容不做翻译
    /// (底层 IO/协议错误跟随其来源语言, 路径与数字则语言无关)
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Io(e) => Some(e.to_string()),
            Self::Protocol(e) => Some(e.to_string()),
            Self::Tls(e) => Some(e.to_string()),
            Self::InvalidPath { path } => Some(path.clone()),
            Self::SourceNotFound(p) => Some(p.display().to_string()),
            Self::Rejected { reason, .. } => reason.clone(),
            Self::HashMismatch { rel_path } => Some(rel_path.clone()),
            Self::Timeout(scene) => Some((*scene).to_string()),
            Self::BadFileId(id) => Some(id.to_string()),
            Self::UnknownTransfer(id) | Self::DuplicateDataSession(id) => Some(id.clone()),
            Self::ResumeUnavailable(why) => Some(why.clone()),
            Self::ResumeOffsetMismatch { offset, part_len } => {
                Some(format!("{offset} != {part_len}"))
            }
            Self::AvatarTooLarge { size } => Some(size.to_string()),
            Self::PeerMismatch | Self::Cancelled | Self::PinRequired => None,
        }
    }
}

/// 同名冲突处理策略(接收端落盘时生效)
///
/// "每次询问"是 UI 层概念: UI 在接收决策时把用户的选择折算成本枚举再下发。
/// serde 用于断点元数据持久化(续传时沿用原任务的策略)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// 自动重命名: `file.txt` → `file (1).txt`
    #[default]
    Rename,
    /// 覆盖同名旧文件
    Overwrite,
}

/// 传输控制状态。声明顺序即合并优先级: 本端与对端取较大者生效
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ControlState {
    /// 正常传输
    Running,
    /// 暂停(可恢复)
    Paused,
    /// 已取消(终态, 接收端删除 .part)
    Cancelled,
}

/// 传输过程事件(引擎 → 上层 UI/CLI)
#[derive(Debug, Clone)]
pub enum TransferEvent {
    /// 单文件进度更新(允许丢弃, 后续事件覆盖)
    ///
    /// 字符串字段用 `Arc<str>`: 每个 chunk(1 MiB)都会发一次,
    /// 共享引用避免热路径上的重复堆分配。
    Progress {
        /// 传输任务 ID
        transfer_id: Arc<str>,
        /// 文件序号
        file_id: u32,
        /// 相对路径
        rel_path: Arc<str>,
        /// 已完成字节数
        done: u64,
        /// 文件总字节数
        size: u64,
    },
    /// 单文件完成(接收端为最终落盘路径, 发送端为源路径)
    FileCompleted {
        /// 传输任务 ID
        transfer_id: String,
        /// 文件序号
        file_id: u32,
        /// 文件路径
        path: PathBuf,
    },
    /// 整个传输任务完成
    Completed {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 传输被主动取消(临时文件已删除)
    Cancelled {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 意外中断(保留 .part 以待续传, 方案决策 #3)
    Interrupted {
        /// 传输任务 ID
        transfer_id: String,
        /// 中断原因(本机语言的展示串, 日志与 CLI 用)
        reason: String,
        /// 稳定错误码(UI 按码查本地化文案)
        code: &'static str,
        /// 错误细节参数(不译, 附加展示)
        detail: Option<String>,
    },
    /// 对端暂停了传输(本端操作由 UI 乐观更新, 不经此事件)
    Paused {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 对端恢复了传输
    Resumed {
        /// 传输任务 ID
        transfer_id: String,
    },
    /// 收到对端文本
    TextReceived {
        /// 发送方信息
        from: PeerInfo,
        /// 文本内容(逐字节一致)
        text: String,
    },
}

/// 事件发送辅助: 进度类事件用 try_send(可丢), 关键事件用 send(必达)
#[derive(Clone)]
pub(crate) struct EventSink {
    /// 底层通道
    tx: mpsc::Sender<TransferEvent>,
}

impl EventSink {
    /// 包装通道
    pub(crate) fn new(tx: mpsc::Sender<TransferEvent>) -> Self {
        Self { tx }
    }

    /// 发送进度事件, 通道满时静默丢弃
    fn progress(&self, event: TransferEvent) {
        let _ = self.tx.try_send(event);
    }

    /// 发送关键事件, 等待通道可用; 消费端已关闭则忽略
    async fn notify(&self, event: TransferEvent) {
        let _ = self.tx.send(event).await;
    }
}

/// 绑定 TCP 监听: 优先 IPv6 双栈(同端口同时受理 IPv4/IPv6), 无 v6 栈时回退纯 IPv4
///
/// 发现层广播的候选地址包含全局 IPv6; 接收端若只在 IPv4 监听,
/// 发送端的每个 IPv6 候选都要白等一次连接超时才轮到 IPv4。
pub async fn bind_dual_stack(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Socket, Type};
    let try_v6 = || -> std::io::Result<std::net::TcpListener> {
        let s = Socket::new(Domain::IPV6, Type::STREAM, None)?;
        // Windows 默认 v6only=true, 显式关闭才能同时受理 v4(映射地址)
        s.set_only_v6(false)?;
        s.bind(&std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())?;
        s.listen(128)?;
        Ok(s.into())
    };
    let std_listener = match try_v6() {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("IPv6 双栈监听不可用({e}), 回退纯 IPv4");
            std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port))?
        }
    };
    std_listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(std_listener)
}

/// Windows 保留设备名: 与其同名(含带任意扩展名)的文件无法正常创建
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 净化单个路径分量, 使其在目标平台可安全落盘
///
/// Windows 规则(接收端为 Windows 时启用):
/// - `< > : " | ? *` 与控制字符替换为 `_` —— `:` 尤其危险: NTFS 会把
///   `foo:bar` 解释为文件 foo 的备用数据流, 建档"成功"但数据不可见
/// - 尾部的点与空格去除(Windows 落盘时静默剥离, 会导致路径对不上)
/// - 保留设备名(CON/PRN/AUX/NUL/COM1-9/LPT1-9, 含带扩展名形式)加前缀 `_`
fn sanitize_component(name: &str, windows_rules: bool) -> String {
    if !windows_rules {
        return name.to_string();
    }
    let mut cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_end_matches(['.', ' ']);
    if trimmed.len() != cleaned.len() {
        cleaned = if trimmed.is_empty() {
            "_".to_string()
        } else {
            trimmed.to_string()
        };
    }
    let stem = cleaned.split('.').next().unwrap_or("");
    if WINDOWS_RESERVED
        .iter()
        .any(|r| stem.eq_ignore_ascii_case(r))
    {
        cleaned.insert(0, '_');
    }
    cleaned
}

/// 清洗对端提供的相对路径, 防路径穿越与非法落盘名
///
/// 规则: 统一把 `\` 视作分隔符; 仅保留普通分量, 出现盘符/根目录/`..`
/// 即拒绝; 空路径拒绝。逐段再按**本机平台**做落盘净化(见内部函数
/// `sanitize_component`): 净化是确定性的, 断点续传的 `.part` 路径
/// 与协商用的原始 rel_path 天然对齐。
pub fn sanitize_rel_path(rel: &str) -> Result<PathBuf, TransferError> {
    sanitize_rel_path_for(rel, cfg!(target_os = "windows"))
}

/// [`sanitize_rel_path`] 的平台参数化实现(测试可覆盖两套规则)
fn sanitize_rel_path_for(rel: &str, windows_rules: bool) -> Result<PathBuf, TransferError> {
    let normalized = rel.replace('\\', "/");
    let mut out = PathBuf::new();
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(c) => {
                out.push(sanitize_component(&c.to_string_lossy(), windows_rules));
            }
            Component::CurDir => {}
            _ => {
                return Err(TransferError::InvalidPath {
                    path: rel.to_string(),
                });
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(TransferError::InvalidPath {
            path: rel.to_string(),
        });
    }
    Ok(out)
}

/// 目标路径已存在时自动追加序号: `file.txt` → `file (1).txt`
pub fn dedup_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()));
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for n in 1u32.. {
        let candidate = parent.join(format!("{stem} ({n}){}", ext.as_deref().unwrap_or("")));
        if !candidate.exists() {
            return candidate;
        }
    }
    // 理论不可达(序号空间远大于目录容量), 兜底加 UUID
    parent.join(format!(
        "{stem}-{}{}",
        uuid::Uuid::new_v4(),
        ext.as_deref().unwrap_or("")
    ))
}

/// 收集待发送文件清单: 展开目录, 生成 `(绝对路径, 相对路径, 大小)` 列表
///
/// 相对路径统一用 `/` 分隔; 单文件取文件名, 目录取 `目录名/内部路径`。
pub fn collect_files(paths: &[PathBuf]) -> Result<Vec<(PathBuf, String, u64)>, TransferError> {
    let mut out = Vec::new();
    for path in paths {
        let meta =
            std::fs::metadata(path).map_err(|_| TransferError::SourceNotFound(path.clone()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| TransferError::SourceNotFound(path.clone()))?;
        if meta.is_dir() {
            walk_dir(path, &name, &mut out)?;
        } else {
            out.push((path.clone(), name, meta.len()));
        }
    }
    Ok(out)
}

/// 递归遍历目录, 收集其中的普通文件(跳过子目录以外的特殊类型)
fn walk_dir(
    dir: &Path,
    rel_base: &str,
    out: &mut Vec<(PathBuf, String, u64)>,
) -> Result<(), TransferError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // 文件名跨平台传输需要 UTF-8, 非法字节做 lossy 转换
        let rel = format!("{rel_base}/{}", entry.file_name().to_string_lossy());
        let meta = std::fs::metadata(&path)?;
        if meta.is_dir() {
            walk_dir(&path, &rel, out)?;
        } else if meta.is_file() {
            out.push((path, rel, meta.len()));
        }
    }
    Ok(())
}

/// 等待控制状态回到 Running; 取消则返回 Err, 每个 chunk 发送前调用
///
/// 两个来源(本端指令 + 对端指令)合并生效, 取较大者。
/// 来源方掉线(watch sender drop)不自动放行, 以 50ms 轮询兜底防忙等。
pub(crate) async fn wait_if_paused(
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<(), TransferError> {
    loop {
        let state = (*local.borrow()).max(*remote.borrow());
        match state {
            ControlState::Running => return Ok(()),
            ControlState::Cancelled => return Err(TransferError::Cancelled),
            ControlState::Paused => {
                tokio::select! {
                    r = local.changed() => {
                        if r.is_err() {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                    r = remote.changed() => {
                        if r.is_err() {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
            }
        }
    }
}

/// 等待任一控制通道出现新状态(哪端变化不重要, 调用方回头重新合并评估)
///
/// sender 已 drop(changed 返回 Err)的通道转为永久挂起, 把进展让给另一
/// 通道或调用方 select 里的其他分支 —— 立即返回会造成忙循环。
async fn changed_either(
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) {
    async fn wait_one(r: &mut watch::Receiver<ControlState>) {
        if r.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
    tokio::select! {
        _ = wait_one(local) => {}
        _ = wait_one(remote) => {}
    }
}

/// 优雅关闭连接: 发送 close_notify 后排空对端数据直至 EOF
///
/// TLS 双向 close_notify 场景下, 若一端 close 时接收缓冲还有未读数据
/// (典型是对端回的 close_notify), 内核会发 RST 冲掉在途帧,
/// 导致对端丢失最后的 FileFooter/DataDone。排空到 EOF 可保证干净的 FIN。
pub(crate) async fn graceful_close<S>(stream: &mut S)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _ = stream.shutdown().await;
    let mut drain = [0u8; 256];
    // 对端不配合时最多等 3 秒, 不挂死收尾流程
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match stream.read(&mut drain).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
}

/// 顺序读取 `len` 字节喂进哈希器(断点续传时重放已传段, FileFooter 是整文件哈希)
///
/// 读取结束后 `file` 的读位置正好在 `len`, 调用方可直接继续顺序读。
pub(crate) async fn hash_prefix(
    file: &mut tokio::fs::File,
    hasher: &mut blake3::Hasher,
    len: u64,
    buf: &mut [u8],
) -> Result<(), TransferError> {
    let mut remaining = len;
    while remaining > 0 {
        let want = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(buf.len()));
        let n = file.read(&mut buf[..want]).await?;
        if n == 0 {
            return Err(TransferError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "文件长度不足以重放已传段",
            )));
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(())
}

/// 数据泵: 从 `r` 精确搬运 `len` 字节到 `w`, 边搬边喂哈希器、回报进度
///
/// 收发双方共用的 chunk 循环骨架(暂停等待 → 读 → 哈希 → 写 → 进度);
/// 进度从 `start` 起算, 提前 EOF 以 `eof_msg` 报错。
///
/// 读写都与控制信号、空闲超时竞速: 对端停发/停读时本端的取消不再被
/// 阻塞 I/O 拖住, 长时间无进展按空闲中断(保留断点可续传)。
#[expect(clippy::too_many_arguments, reason = "内部装配函数, 参数即搬运上下文")]
async fn pump_chunks<R, W>(
    r: &mut R,
    w: &mut W,
    len: u64,
    start: u64,
    hasher: &mut blake3::Hasher,
    eof_msg: &'static str,
    buf: &mut [u8],
    mut on_progress: impl FnMut(u64),
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<(), TransferError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut remaining = len;
    let mut done = start;
    while remaining > 0 {
        wait_if_paused(local, remote).await?;
        let want = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(buf.len()));
        // 读一块: read 取消安全(被打断即未消费), 信号触发回循环头重新评估
        let n = tokio::select! {
            got = r.read(&mut buf[..want]) => got?,
            _ = changed_either(local, remote) => continue,
            _ = tokio::time::sleep(DATA_IDLE_TIMEOUT) => {
                return Err(TransferError::Timeout("数据通道读空闲"));
            }
        };
        if n == 0 {
            return Err(TransferError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                eof_msg,
            )));
        }
        hasher.update(&buf[..n]);
        write_all_controlled(w, &buf[..n], local, remote).await?;
        remaining -= n as u64;
        done += n as u64;
        on_progress(done);
    }
    // flush 同样可能卡在停读的对端上, 一并限时
    tokio::time::timeout(DATA_IDLE_TIMEOUT, w.flush())
        .await
        .map_err(|_| TransferError::Timeout("数据通道写空闲"))??;
    Ok(())
}

/// 可中断的 write_all: 逐段写入, 每段与控制信号、空闲超时竞速
///
/// 块内只响应取消 —— 数据已进哈希器, 半途弃写会让流错位, 暂停留到
/// 下一块的循环头生效; 单次 write 取消安全(未被接受的字节不会半写)。
async fn write_all_controlled<W>(
    w: &mut W,
    mut data: &[u8],
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<(), TransferError>
where
    W: AsyncWrite + Unpin,
{
    while !data.is_empty() {
        if (*local.borrow()).max(*remote.borrow()) == ControlState::Cancelled {
            return Err(TransferError::Cancelled);
        }
        let n = tokio::select! {
            wrote = w.write(data) => wrote?,
            _ = changed_either(local, remote) => continue,
            _ = tokio::time::sleep(DATA_IDLE_TIMEOUT) => {
                return Err(TransferError::Timeout("数据通道写空闲"));
            }
        };
        if n == 0 {
            return Err(TransferError::Io(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "数据通道写入零字节",
            )));
        }
        data = &data[n..];
    }
    Ok(())
}

/// 发送文件内容: 从 `offset` 读到 `size` 截止, 边发边算 BLAKE3 并回报进度
///
/// 返回整文件哈希(hex)。offset > 0(断点续传)时先重放前段数据进哈希器,
/// FileFooter 始终携带整文件哈希。
#[expect(clippy::too_many_arguments, reason = "内部装配函数, 参数即发送上下文")]
pub(crate) async fn send_file_body<W>(
    w: &mut W,
    path: &Path,
    size: u64,
    offset: u64,
    buf: &mut [u8],
    on_progress: impl FnMut(u64),
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<String, TransferError>
where
    W: AsyncWrite + Unpin,
{
    let mut file = tokio::fs::File::open(path).await?;
    // 大文件单次顺序读不驻留页缓存(macOS), 防挤掉其他应用热页
    io_tuning::advise_no_cache(&file, size);
    let mut hasher = blake3::Hasher::new();
    if offset > 0 {
        // 重放后读位置自然停在 offset, 无需 seek
        hash_prefix(&mut file, &mut hasher, offset, buf).await?;
    }
    pump_chunks(
        &mut file,
        w,
        size.saturating_sub(offset),
        offset,
        &mut hasher,
        "源文件在发送过程中被截断",
        buf,
        on_progress,
        local,
        remote,
    )
    .await?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// 接收文件内容: 精确读取 `len` 字节写入 `file`, 边收边算 BLAKE3 并回报进度
///
/// `hasher` 由调用方构造(断点续传时已喂入 .part 的既有前段);
/// 返回整文件哈希(hex), 由调用方与 FileFooter 比对。
#[expect(clippy::too_many_arguments, reason = "内部装配函数, 参数即接收上下文")]
pub(crate) async fn receive_file_body<R>(
    r: &mut R,
    file: &mut tokio::fs::File,
    len: u64,
    mut hasher: blake3::Hasher,
    offset: u64,
    buf: &mut [u8],
    on_progress: impl FnMut(u64),
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
) -> Result<String, TransferError>
where
    R: AsyncRead + Unpin,
{
    pump_chunks(
        r,
        file,
        len,
        offset,
        &mut hasher,
        "数据流提前结束",
        buf,
        on_progress,
        local,
        remote,
    )
    .await?;
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests;
