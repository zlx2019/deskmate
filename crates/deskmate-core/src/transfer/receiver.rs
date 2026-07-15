//! 接收端引擎: 监听 TCP 端口, 按连接首帧分派控制/数据会话
//!
//! - 控制会话(首帧 Hello): 应答握手, 处理传输请求(经 offer 通道交上层决策)、
//!   文本消息、暂停/恢复/取消指令
//! - 数据会话(首帧 DataHello): 校验来源身份后接收文件流,
//!   写 `.part` 临时文件, 校验 BLAKE3 后重命名落盘
//!
//! 断连语义(方案决策 #3): 主动取消删除 `.part`; 意外断连保留以待续传。

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::Instant;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;
use crate::identity::DeviceIdentity;
use crate::protocol::{
    ControlMessage, FileMeta, PeerInfo, ProtocolError, ResumeFileState, check_version, read_frame,
    write_frame,
};
use crate::tls::{peer_fingerprint, server_config};

use super::{
    ConflictPolicy, ControlState, EventSink, OFFER_TIMEOUT, PART_SUFFIX, TransferError,
    TransferEvent, dedup_path, graceful_close, receive_file_body, sanitize_rel_path,
};

/// 接收服务配置
#[derive(Debug, Clone)]
pub struct ReceiverOptions {
    /// 默认下载目录(单次传输可在决策时另行指定)
    pub download_dir: PathBuf,
    /// 本机头像图片字节(应答对端 AvatarRequest; 与昵称一致, 重启后生效)
    pub avatar_image: Option<Vec<u8>>,
    /// 断点元数据目录(每个已接受的任务一个 json, 意外断连后据此续传)
    pub resume_dir: PathBuf,
    /// 配对 PIN(None 不启用; 可经 [`ReceiverHandle::set_pin`] 运行时修改)
    pub pin: Option<String>,
}

use crate::config::{
    HANDSHAKE_TIMEOUT, MAX_CONCURRENT_CONNECTIONS, PENDING_SWEEP_INTERVAL, PENDING_TTL,
    PIN_MAX_FAILURES, PIN_TRACK_CAP, PIN_WINDOW,
};

/// 断点元数据: 任务被接受即落盘, 完成/取消即删除, 意外断连时留存
///
/// 与方案 4.2 的差异说明: 集中存放在引擎数据目录而非下载目录的
/// `.deskmate/` 子目录 —— 用户每次可另选保存位置, 集中存放才能可靠定位。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeMeta {
    /// 发送方指纹(续传请求者必须是原发送方)
    peer_fingerprint: String,
    /// 完整文件清单
    files: Vec<FileMeta>,
    /// 被接受的文件序号
    accepted: Vec<u32>,
    /// 已完整落盘的文件序号(续传时跳过)
    completed: Vec<u32>,
    /// 保存目录
    save_dir: PathBuf,
    /// 同名冲突策略(续传沿用)
    conflict: ConflictPolicy,
}

/// 待决策的传输请求(经 offer 通道交给上层, 用 `reply` 应答)
#[derive(Debug)]
pub struct TransferOffer {
    /// 发送方设备信息
    pub peer: PeerInfo,
    /// 传输任务 ID
    pub transfer_id: String,
    /// 文件清单
    pub files: Vec<FileMeta>,
    /// 总字节数
    pub total_size: u64,
    /// 决策回执通道(超时 5 分钟视为拒绝)
    pub reply: oneshot::Sender<OfferDecision>,
}

/// 接收方对传输请求的决策
#[derive(Debug)]
pub enum OfferDecision {
    /// 接受指定文件(支持部分接受); `save_dir` 为 None 时用默认下载目录
    Accept {
        /// 接受的文件序号
        accepted_files: Vec<u32>,
        /// 本次传输的保存目录
        save_dir: Option<PathBuf>,
        /// 同名冲突处理方式
        conflict: ConflictPolicy,
    },
    /// 拒绝本次传输
    Reject {
        /// 拒绝原因(会回传给发送方)
        reason: Option<String>,
    },
}

/// 已被接受、等待或正在进行数据传输的任务
struct PendingTransfer {
    /// 发送方信息(数据连接来源校验)
    peer: PeerInfo,
    /// 文件清单(按序号索引)
    files: HashMap<u32, FileMeta>,
    /// 被接受的文件序号
    accepted: HashSet<u32>,
    /// 保存目录
    save_dir: PathBuf,
    /// 同名冲突处理方式
    conflict: ConflictPolicy,
    /// 控制状态源(Pause/Resume/Cancel 写入, 数据会话订阅)
    control: watch::Sender<ControlState>,
    /// 已有数据会话占用(同一任务拒绝并发数据连接, 防 .part 互写)
    active: bool,
    /// 登记时刻(始终未开数据连接的条目按 TTL 清扫, 防泄漏)
    registered_at: Instant,
}

/// 进行中任务表: 控制会话写入, 数据会话消费
type PendingMap = Arc<Mutex<HashMap<String, PendingTransfer>>>;

/// 本机头像数据: (BLAKE3 哈希, 图片字节) 成对存放, 保证应答一致性
type AvatarData = Option<(String, Vec<u8>)>;

/// 接收服务共享上下文
struct ReceiverCtx {
    /// 本机设备信息(握手应答用; 昵称/头像变更经 setter 即时生效)
    self_info: Arc<RwLock<PeerInfo>>,
    /// 默认下载目录(可经 [`ReceiverHandle::set_download_dir`] 运行时修改)
    download_dir: Arc<RwLock<PathBuf>>,
    /// 本机头像图片与其哈希(应答 AvatarRequest, 运行时可换)
    avatar: Arc<RwLock<AvatarData>>,
    /// 断点元数据目录
    resume_dir: PathBuf,
    /// 配对 PIN(None 不启用, 运行时可改)
    pin: Arc<RwLock<Option<String>>>,
    /// PIN 失败限速状态: 来源指纹 → (窗口起点, 窗口内失败次数)
    ///
    /// 按来源分别计数 —— 全局单计数会让任一乱试的设备锁死全网配对。
    pin_failures: Mutex<HashMap<String, (Instant, u32)>>,
    /// 传输请求上抛通道
    offers: mpsc::Sender<TransferOffer>,
    /// 事件流
    sink: EventSink,
    /// 进行中任务表
    pending: PendingMap,
    /// 落盘定名互斥: dedup 的存在性探测与 rename 之间不允许并发插入,
    /// 否则两个同名文件会 dedup 出同一路径互相覆盖(rename 本身很快, 串行无碍)
    finalize_lock: tokio::sync::Mutex<()>,
}

/// 接收服务句柄: 查询监听地址、控制进行中的传输
pub struct ReceiverHandle {
    /// 实际监听地址
    local_addr: SocketAddr,
    /// 进行中任务表(与服务共享)
    pending: PendingMap,
    /// 默认下载目录(与服务共享, 运行时可改)
    download_dir: Arc<RwLock<PathBuf>>,
    /// 配对 PIN(与服务共享, 运行时可改)
    pin: Arc<RwLock<Option<String>>>,
    /// 握手身份(与服务共享, 昵称/头像变更即时生效)
    self_info: Arc<RwLock<PeerInfo>>,
    /// 头像数据(与服务共享, 运行时可换)
    avatar: Arc<RwLock<AvatarData>>,
}

impl ReceiverHandle {
    /// 实际监听地址(bind 端口 0 时从这里拿真实端口)
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// 暂停指定传输; 任务不存在返回 false
    pub fn pause(&self, transfer_id: &str) -> bool {
        set_control(&self.pending, transfer_id, ControlState::Paused)
    }

    /// 恢复指定传输; 任务不存在返回 false
    pub fn resume(&self, transfer_id: &str) -> bool {
        set_control(&self.pending, transfer_id, ControlState::Running)
    }

    /// 取消指定传输(接收中的 .part 会被删除); 任务不存在返回 false
    pub fn cancel(&self, transfer_id: &str) -> bool {
        cancel_transfer(&self.pending, transfer_id)
    }

    /// 当前默认下载目录
    pub fn download_dir(&self) -> PathBuf {
        read_lock(&self.download_dir).clone()
    }

    /// 修改默认下载目录, 对之后的传输即时生效
    pub fn set_download_dir(&self, dir: PathBuf) {
        *self
            .download_dir
            .write()
            .unwrap_or_else(PoisonError::into_inner) = dir;
    }

    /// 修改配对 PIN(None 关闭), 对之后的请求即时生效
    pub fn set_pin(&self, pin: Option<String>) {
        *self.pin.write().unwrap_or_else(PoisonError::into_inner) = pin;
    }

    /// 热更新握手身份与头像(昵称/头像变更即时生效)
    ///
    /// 对之后的控制会话生效: HelloAck 带新身份, AvatarRequest 应答新图。
    /// 头像哈希在此统一预计算, 保证与图片字节成对一致。
    pub fn set_self_info(&self, info: PeerInfo, avatar_image: Option<Vec<u8>>) {
        *self
            .self_info
            .write()
            .unwrap_or_else(PoisonError::into_inner) = info;
        *self.avatar.write().unwrap_or_else(PoisonError::into_inner) =
            avatar_image.map(|img| (blake3::hash(&img).to_hex().to_string(), img));
    }
}

/// 读锁; 毒锁直接恢复内部数据
fn read_lock(lock: &RwLock<PathBuf>) -> RwLockReadGuard<'_, PathBuf> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// 启动接收服务: 在给定 listener 上受理连接, 返回服务句柄
///
/// 服务生命周期与进程一致(M1); listener 关闭或致命错误时 accept 循环退出。
pub fn spawn_receiver(
    identity: Arc<DeviceIdentity>,
    listener: TcpListener,
    options: ReceiverOptions,
    offers: mpsc::Sender<TransferOffer>,
    events: mpsc::Sender<TransferEvent>,
) -> Result<ReceiverHandle, TransferError> {
    let acceptor = TlsAcceptor::from(Arc::new(server_config(&identity)?));
    let local_addr = listener.local_addr()?;
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    let download_dir = Arc::new(RwLock::new(options.download_dir));
    let pin = Arc::new(RwLock::new(options.pin));
    let self_info = Arc::new(RwLock::new(identity.peer_info()));
    // 头像哈希预计算并与图片字节成对存放(应答时保证一致)
    let avatar: Arc<RwLock<AvatarData>> = Arc::new(RwLock::new(
        options
            .avatar_image
            .map(|img| (blake3::hash(&img).to_hex().to_string(), img)),
    ));
    let ctx = Arc::new(ReceiverCtx {
        self_info: Arc::clone(&self_info),
        download_dir: Arc::clone(&download_dir),
        avatar: Arc::clone(&avatar),
        resume_dir: options.resume_dir,
        pin: Arc::clone(&pin),
        pin_failures: Mutex::new(HashMap::new()),
        offers,
        sink: EventSink::new(events),
        pending: Arc::clone(&pending),
        finalize_lock: tokio::sync::Mutex::new(()),
    });
    spawn_pending_sweeper(&ctx);
    tokio::spawn(accept_loop(listener, acceptor, ctx));
    Ok(ReceiverHandle {
        local_addr,
        pending,
        download_dir,
        pin,
        self_info,
        avatar,
    })
}

/// 校验来访请求的 PIN; 未启用 PIN 一律放行
///
/// 暴力破解限速按来源(`peer_fp`, 即 TLS 证书指纹)分别计数: 单一来源
/// 60s 窗口内累计 5 次失败后, 该来源整窗一律拒绝(即便 PIN 正确, 防在线
/// 试探), 其他设备不受影响; 校验成功清除该来源的计数。
fn pin_ok(ctx: &ReceiverCtx, peer_fp: &str, provided: Option<&str>) -> bool {
    let expected = ctx
        .pin
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(expected) = expected else {
        return true;
    };
    let mut failures = ctx
        .pin_failures
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    // 窗口已过的条目顺手回收, 表大小由此有界(≈窗口内仍在失败的来源数)
    failures.retain(|_, (start, _)| now.duration_since(*start) <= PIN_WINDOW);
    if failures
        .get(peer_fp)
        .is_some_and(|(_, fails)| *fails >= PIN_MAX_FAILURES)
    {
        tracing::warn!("PIN 尝试过于频繁, 该来源整窗一律拒绝");
        return false;
    }
    if provided == Some(expected.as_str()) {
        failures.remove(peer_fp);
        return true;
    }
    // 表满则不再追踪新来源, 保守拒绝(窗口内上千个不同指纹本身就是攻击信号)
    if failures.len() >= PIN_TRACK_CAP && !failures.contains_key(peer_fp) {
        tracing::warn!("PIN 失败来源过多, 保守拒绝新来源");
        return false;
    }
    let entry = failures.entry(peer_fp.to_string()).or_insert((now, 0));
    entry.1 += 1;
    tracing::warn!("PIN 校验失败({}/{PIN_MAX_FAILURES})", entry.1);
    false
}

/// 启动过期任务清扫: 已接受但发送方一直未开数据连接的表项按 TTL 回收
///
/// 弱引用持有上下文: 服务的全部强引用(accept 循环与连接任务)退出后,
/// 清扫任务随之自行结束(测试等场景会反复建/销服务, 不能钉住资源)。
fn spawn_pending_sweeper(ctx: &Arc<ReceiverCtx>) {
    let weak = Arc::downgrade(ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PENDING_SWEEP_INTERVAL);
        tick.tick().await; // interval 首跳立即完成, 跳过
        loop {
            tick.tick().await;
            let Some(ctx) = weak.upgrade() else { return };
            sweep_pending(&ctx);
        }
    });
}

/// 清理超时未开始的任务: 表项移除 + 断点元数据删除(防两者无界累积)
///
/// 只清"未被数据会话占用(!active)且登记已超 TTL"的条目 —— 进行中的
/// 会话不受影响; 意外断连留下的断点(表项已随会话移除, 仅剩 resume
/// 文件与 .part)也不在清理范围, 续传语义不变。
fn sweep_pending(ctx: &ReceiverCtx) {
    let mut expired = Vec::new();
    lock_pending(&ctx.pending).retain(|id, task| {
        let dead = !task.active && task.registered_at.elapsed() > PENDING_TTL;
        if dead {
            expired.push(id.clone());
        }
        !dead
    });
    // 元数据删除放锁外(单个 unlink 极快, 异步任务里直接调用可容忍)
    for id in &expired {
        remove_resume_meta(&ctx.resume_dir, id);
        tracing::info!(transfer_id = %id, "清理超时未开始的接收任务");
    }
}

/// 受理循环: 每个连接独立任务处理, 单连接故障不影响服务
async fn accept_loop(listener: TcpListener, acceptor: TlsAcceptor, ctx: Arc<ReceiverCtx>) {
    // 并发连接上限: 满载时直接拒绝新连接, 防恶意半开连接无界累积耗尽 fd
    let conn_permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    loop {
        let (tcp, remote) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!("accept 失败, 接收服务退出: {e}");
                return;
            }
        };
        let Ok(permit) = Arc::clone(&conn_permits).try_acquire_owned() else {
            tracing::warn!(%remote, "并发连接已达上限, 拒绝新连接");
            continue;
        };
        let acceptor = acceptor.clone();
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            let _permit = permit; // 连接任务全程占一个名额
            if let Err(e) = handle_connection(tcp, acceptor, ctx).await {
                tracing::debug!(%remote, "连接会话结束: {e}");
            }
        });
    }
}

/// 处理单个连接: TLS 握手后按首帧分派会话类型
async fn handle_connection(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    ctx: Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    tcp.set_nodelay(true)?;
    super::io_tuning::tune_socket(&tcp);
    // 未认证阶段限时: 握手与首帧必须尽快完成, 挡住"连上后不说话"的占坑连接
    let mut tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp))
        .await
        .map_err(|_| TransferError::Timeout("TLS 握手"))??;
    // 双向认证: 客户端证书指纹即其身份
    let conn_fp =
        peer_fingerprint(tls.get_ref().1.peer_certificates()).ok_or(TransferError::PeerMismatch)?;

    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut tls))
        .await
        .map_err(|_| TransferError::Timeout("首帧"))?;
    match first? {
        ControlMessage::Hello { version, info } => {
            check_version(&version)?;
            // 声明身份必须与 TLS 证书一致, 防冒充
            if info.fingerprint != conn_fp {
                return Err(TransferError::PeerMismatch);
            }
            control_session(tls, info, &ctx).await
        }
        ControlMessage::DataHello { transfer_id } => {
            data_session(tls, transfer_id, &conn_fp, &ctx).await
        }
        other => Err(TransferError::Protocol(ProtocolError::Unexpected {
            expected: "hello | data_hello",
            got: other.kind().to_string(),
        })),
    }
}

/// 控制会话: 应答握手后循环处理对端消息, 直至 Bye 或断连
async fn control_session(
    mut tls: TlsStream<TcpStream>,
    peer: PeerInfo,
    ctx: &Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    // 先快照再写帧: 读锁 guard 不能跨 await(std 锁非 Send)
    let self_info = ctx
        .self_info
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    write_frame(
        &mut tls,
        &ControlMessage::HelloAck {
            version: PROTOCOL_VERSION.to_string(),
            info: self_info,
        },
    )
    .await?;

    loop {
        match read_frame(&mut tls).await {
            Ok(ControlMessage::TransferRequest {
                transfer_id,
                files,
                total_size,
                pin,
            }) => {
                // PIN 是第一道门: 不过门连确认弹窗都不弹
                let resp = if pin_ok(ctx, &peer.fingerprint, pin.as_deref()) {
                    handle_request(&peer, transfer_id, files, total_size, ctx).await
                } else {
                    ControlMessage::TransferResponse {
                        transfer_id,
                        accepted_files: Vec::new(),
                        reason: Some("需要正确的配对 PIN".to_string()),
                        pin_required: true,
                    }
                };
                write_frame(&mut tls, &resp).await?;
            }
            Ok(ControlMessage::Text { text, pin }) => {
                handle_text(&mut tls, &peer, text, pin.as_deref(), ctx).await?;
            }
            Ok(ControlMessage::ResumeQuery { transfer_id }) => {
                let files = resume_states(&peer, &transfer_id, ctx);
                write_frame(&mut tls, &ControlMessage::ResumeInfo { transfer_id, files }).await?;
            }
            Ok(ControlMessage::AvatarRequest) => {
                handle_avatar_request(&mut tls, ctx).await?;
            }
            Ok(ControlMessage::Pause { transfer_id }) => {
                set_control(&ctx.pending, &transfer_id, ControlState::Paused);
            }
            Ok(ControlMessage::Resume { transfer_id }) => {
                set_control(&ctx.pending, &transfer_id, ControlState::Running);
            }
            Ok(ControlMessage::Cancel { transfer_id }) => {
                cancel_transfer(&ctx.pending, &transfer_id);
            }
            // 对端优雅告别或断连: 控制会话正常终结
            Ok(ControlMessage::Bye) | Err(_) => return Ok(()),
            Ok(other) => {
                tracing::debug!(kind = other.kind(), "控制会话忽略消息");
            }
        }
    }
}

/// 处理文本消息: 过 PIN 门后上抛事件并回执; 不过门回结构化拒因
async fn handle_text(
    tls: &mut TlsStream<TcpStream>,
    peer: &PeerInfo,
    text: String,
    pin: Option<&str>,
    ctx: &Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    if pin_ok(ctx, &peer.fingerprint, pin) {
        ctx.sink
            .notify(TransferEvent::TextReceived {
                from: peer.clone(),
                text,
            })
            .await;
        write_frame(tls, &ControlMessage::TextAck).await?;
    } else {
        write_frame(tls, &ControlMessage::TextRejected { pin_required: true }).await?;
    }
    Ok(())
}

/// 处理头像请求: 帧内报哈希与长度, 帧后紧跟裸图片字节(未设置时 size=0 无数据)
async fn handle_avatar_request(
    tls: &mut TlsStream<TcpStream>,
    ctx: &Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    // 快照取出(hash 与字节成对), 避免持锁跨网络写
    let snapshot = ctx
        .avatar
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let (hash, img) = snapshot.unwrap_or_default();
    write_frame(
        tls,
        &ControlMessage::AvatarResponse {
            hash,
            size: img.len() as u64,
        },
    )
    .await?;
    if !img.is_empty() {
        use tokio::io::AsyncWriteExt;
        tls.write_all(&img).await?;
        tls.flush().await?;
    }
    Ok(())
}

/// 处理传输请求: 上抛决策, 通过后登记任务表, 构造应答帧
async fn handle_request(
    peer: &PeerInfo,
    transfer_id: String,
    files: Vec<FileMeta>,
    total_size: u64,
    ctx: &Arc<ReceiverCtx>,
) -> ControlMessage {
    let reject = |transfer_id: String, reason: &str| ControlMessage::TransferResponse {
        transfer_id,
        accepted_files: Vec::new(),
        reason: Some(reason.to_string()),
        pin_required: false,
    };

    // 任务 ID 会拼入断点元数据文件名, 非法字符直接拒绝
    if !is_safe_transfer_id(&transfer_id) {
        return reject(transfer_id, "非法的任务 ID");
    }

    let (reply_tx, reply_rx) = oneshot::channel();
    let offer = TransferOffer {
        peer: peer.clone(),
        transfer_id: transfer_id.clone(),
        files: files.clone(),
        total_size,
        reply: reply_tx,
    };
    if ctx.offers.send(offer).await.is_err() {
        return reject(transfer_id, "接收方不可用");
    }

    match tokio::time::timeout(OFFER_TIMEOUT, reply_rx).await {
        Ok(Ok(OfferDecision::Accept {
            accepted_files,
            save_dir,
            conflict,
        })) => {
            let valid: Vec<u32> = accepted_files
                .into_iter()
                .filter(|id| files.iter().any(|f| f.file_id == *id))
                .collect();
            if valid.is_empty() {
                return reject(transfer_id, "未选择任何有效文件");
            }
            let save_dir = save_dir.unwrap_or_else(|| read_lock(&ctx.download_dir).clone());
            register_pending(peer, transfer_id, files, valid, save_dir, conflict, ctx)
        }
        Ok(Ok(OfferDecision::Reject { reason })) => ControlMessage::TransferResponse {
            transfer_id,
            accepted_files: Vec::new(),
            reason,
            pin_required: false,
        },
        // 上层丢弃回执或超时未决策, 一律按拒绝处理
        Ok(Err(_)) | Err(_) => reject(transfer_id, "等待接收方决策超时"),
    }
}

/// 接受决策落地: 断点元数据先落盘(意外断连后凭它续传, 方案决策 #3),
/// 登记任务表项, 返回接受应答帧
fn register_pending(
    peer: &PeerInfo,
    transfer_id: String,
    files: Vec<FileMeta>,
    valid: Vec<u32>,
    save_dir: PathBuf,
    conflict: ConflictPolicy,
    ctx: &Arc<ReceiverCtx>,
) -> ControlMessage {
    // 同 ID 任务已在表中(在传或待传)时拒绝重复登记, 防止覆盖其控制通道
    if lock_pending(&ctx.pending).contains_key(&transfer_id) {
        return ControlMessage::TransferResponse {
            transfer_id,
            accepted_files: Vec::new(),
            reason: Some("同 ID 任务已在进行".to_string()),
            pin_required: false,
        };
    }
    save_resume_meta(
        &ctx.resume_dir,
        &transfer_id,
        &ResumeMeta {
            peer_fingerprint: peer.fingerprint.clone(),
            files: files.clone(),
            accepted: valid.clone(),
            completed: Vec::new(),
            save_dir: save_dir.clone(),
            conflict,
        },
    );
    let (control, _) = watch::channel(ControlState::Running);
    lock_pending(&ctx.pending).insert(
        transfer_id.clone(),
        PendingTransfer {
            peer: peer.clone(),
            files: files.into_iter().map(|f| (f.file_id, f)).collect(),
            accepted: valid.iter().copied().collect(),
            save_dir,
            conflict,
            control,
            active: false,
            registered_at: Instant::now(),
        },
    );
    ControlMessage::TransferResponse {
        transfer_id,
        accepted_files: valid,
        reason: None,
        pin_required: false,
    }
}

/// 断点协商: 校验元数据与请求者身份, 重建任务表项, 返回未完成文件的断点清单
///
/// 任一环节不满足(ID 非法 / 无元数据 / 身份不符 / 全部已完成)均返回空列表,
/// 发送端据此判定不可续传。
fn resume_states(
    peer: &PeerInfo,
    transfer_id: &str,
    ctx: &Arc<ReceiverCtx>,
) -> Vec<ResumeFileState> {
    if !is_safe_transfer_id(transfer_id) {
        return Vec::new();
    }
    // 任务仍在表中(在传或待传)时拒绝续传协商: 覆盖表项会顶掉原会话的控制通道
    if lock_pending(&ctx.pending).contains_key(transfer_id) {
        tracing::warn!(transfer_id, "任务尚在进行, 拒绝续传协商");
        return Vec::new();
    }
    let Some(meta) = load_resume_meta(&ctx.resume_dir, transfer_id) else {
        return Vec::new();
    };
    // 续传请求者必须是原发送方本人
    if meta.peer_fingerprint != peer.fingerprint {
        tracing::warn!(transfer_id, "续传请求者与原发送方身份不符, 已拒绝");
        return Vec::new();
    }

    // 未完成 = 已接受 − 已完成; 断点 = .part 文件当前长度(无 part 从 0 起)
    let mut states = Vec::new();
    for file in &meta.files {
        if !meta.accepted.contains(&file.file_id) || meta.completed.contains(&file.file_id) {
            continue;
        }
        let Ok(rel) = sanitize_rel_path(&file.rel_path) else {
            continue;
        };
        let received = std::fs::metadata(part_path_of(&meta.save_dir.join(&rel), transfer_id))
            .map(|m| m.len())
            .unwrap_or(0);
        states.push(ResumeFileState {
            file_id: file.file_id,
            rel_path: file.rel_path.clone(),
            size: file.size,
            // part 比声明还大(异常残留)时从 0 重传
            received: if received > file.size { 0 } else { received },
        });
    }
    if states.is_empty() {
        return Vec::new();
    }

    // 重建任务表项, 发送端随后的数据连接走既有 data_session 路径
    let (control, _) = watch::channel(ControlState::Running);
    lock_pending(&ctx.pending).insert(
        transfer_id.to_string(),
        PendingTransfer {
            peer: peer.clone(),
            files: meta.files.iter().cloned().map(|f| (f.file_id, f)).collect(),
            accepted: states.iter().map(|s| s.file_id).collect(),
            save_dir: meta.save_dir.clone(),
            conflict: meta.conflict,
            control,
            active: false,
            registered_at: Instant::now(),
        },
    );
    states
}

/// 数据会话: 校验来源与任务归属后接收文件流
async fn data_session(
    mut tls: TlsStream<TcpStream>,
    transfer_id: String,
    conn_fp: &str,
    ctx: &Arc<ReceiverCtx>,
) -> Result<(), TransferError> {
    // 快照任务信息; 表项保留至会话结束, 供控制会话下发指令
    let (files, accepted, save_dir, conflict, control_rx) = {
        let mut guard = lock_pending(&ctx.pending);
        let task = guard
            .get_mut(&transfer_id)
            .ok_or_else(|| TransferError::UnknownTransfer(transfer_id.clone()))?;
        // 数据连接必须来自请求方本人
        if task.peer.fingerprint != conn_fp {
            return Err(TransferError::PeerMismatch);
        }
        // 独占占用: 同一任务的并发数据连接会写坏同一组 .part
        // (此处的 return 不会走下方的表项清理, 不影响已占用的会话)
        if task.active {
            return Err(TransferError::DuplicateDataSession(transfer_id.clone()));
        }
        task.active = true;
        (
            task.files.clone(),
            task.accepted.clone(),
            task.save_dir.clone(),
            task.conflict,
            task.control.subscribe(),
        )
    };

    // 接收端本端与对端指令走同一条 watch(控制会话统一写入),
    // 克隆两份只为复用发送端"local + remote"双参聚合接口
    let mut local = control_rx.clone();
    let mut remote = control_rx;
    let result = receive_data_stream(
        &mut tls,
        &transfer_id,
        &files,
        &accepted,
        &save_dir,
        conflict,
        &ctx.resume_dir,
        &ctx.finalize_lock,
        &mut local,
        &mut remote,
        &ctx.sink,
    )
    .await;

    // 成功路径回 close_notify 并排空对端, 保证双向干净关闭(失败路径连接已不可用)
    if result.is_ok() {
        graceful_close(&mut tls).await;
    }

    // 会话结束(无论成败)即清理任务表
    lock_pending(&ctx.pending).remove(&transfer_id);
    match result {
        Ok(()) => {
            remove_resume_meta(&ctx.resume_dir, &transfer_id);
            ctx.sink
                .notify(TransferEvent::Completed { transfer_id })
                .await;
            Ok(())
        }
        Err(TransferError::Cancelled) => {
            remove_resume_meta(&ctx.resume_dir, &transfer_id);
            ctx.sink
                .notify(TransferEvent::Cancelled { transfer_id })
                .await;
            Ok(())
        }
        Err(e) => {
            // 意外中断: .part 与断点元数据均保留, 等待发送端 ResumeQuery(决策 #3)
            ctx.sink
                .notify(TransferEvent::Interrupted {
                    transfer_id,
                    reason: e.to_string(),
                })
                .await;
            Err(e)
        }
    }
}

/// 接收数据流: 循环处理 FileHeader + 字节流 + FileFooter, 直至 DataDone
#[expect(clippy::too_many_arguments, reason = "内部装配函数, 参数即会话上下文")]
async fn receive_data_stream(
    tls: &mut TlsStream<TcpStream>,
    transfer_id: &str,
    files: &HashMap<u32, FileMeta>,
    accepted: &HashSet<u32>,
    save_dir: &Path,
    conflict: ConflictPolicy,
    resume_dir: &Path,
    finalize_lock: &tokio::sync::Mutex<()>,
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<(), TransferError> {
    // 会话级复用: chunk 缓冲、任务 ID、断点元数据(内存持有, 完成时增量落盘)
    let mut buf = vec![0u8; super::CHUNK_SIZE];
    let tid: Arc<str> = Arc::from(transfer_id);
    let mut resume_meta = load_resume_meta(resume_dir, transfer_id);
    loop {
        // 帧间隙同样受空闲上限约束(对端本地暂停可长时间停在文件边界, 故用长值)
        let frame = tokio::time::timeout(crate::config::DATA_IDLE_TIMEOUT, read_frame(tls))
            .await
            .map_err(|_| TransferError::Timeout("等待数据帧"))?;
        match frame? {
            ControlMessage::FileHeader { file_id, offset } => {
                let meta = files
                    .get(&file_id)
                    .ok_or(TransferError::BadFileId(file_id))?;
                if !accepted.contains(&file_id) {
                    return Err(TransferError::BadFileId(file_id));
                }
                receive_one_file(
                    tls,
                    &tid,
                    meta,
                    offset,
                    save_dir,
                    conflict,
                    finalize_lock,
                    &mut buf,
                    local,
                    remote,
                    sink,
                )
                .await?;
                // 断点元数据记为已完成: 之后中断的续传协商会跳过本文件
                if let Some(state) = resume_meta.as_mut() {
                    mark_completed_persist(state, resume_dir, transfer_id, file_id).await;
                }
            }
            ControlMessage::DataDone => return Ok(()),
            other => {
                return Err(TransferError::Protocol(ProtocolError::Unexpected {
                    expected: "file_header | data_done",
                    got: other.kind().to_string(),
                }));
            }
        }
    }
}

/// 接收单个文件: 写 .part → 校验 FileFooter 哈希 → 去重重命名落盘
#[expect(clippy::too_many_arguments, reason = "内部装配函数, 参数即会话上下文")]
async fn receive_one_file(
    tls: &mut TlsStream<TcpStream>,
    transfer_id: &Arc<str>,
    meta: &FileMeta,
    offset: u64,
    save_dir: &Path,
    conflict: ConflictPolicy,
    finalize_lock: &tokio::sync::Mutex<()>,
    buf: &mut [u8],
    local: &mut watch::Receiver<ControlState>,
    remote: &mut watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<(), TransferError> {
    let rel = sanitize_rel_path(&meta.rel_path)?;
    let target = save_dir.join(&rel);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part_path = part_path_of(&target, transfer_id);

    // FileFooter 是整文件哈希: 续传(offset > 0)时先重放 .part 既有前段
    let mut hasher = blake3::Hasher::new();
    let mut file = open_part(&part_path, offset, meta.size, &mut hasher, buf).await?;

    let (tid, rel_str, fid, size) = (
        Arc::clone(transfer_id),
        Arc::<str>::from(meta.rel_path.as_str()),
        meta.file_id,
        meta.size,
    );
    let progress_sink = sink.clone();
    let received_hash = match receive_file_body(
        tls,
        &mut file,
        meta.size.saturating_sub(offset),
        hasher,
        offset,
        buf,
        move |done| {
            progress_sink.progress(TransferEvent::Progress {
                transfer_id: Arc::clone(&tid),
                file_id: fid,
                rel_path: Arc::clone(&rel_str),
                done,
                size,
            });
        },
        local,
        remote,
    )
    .await
    {
        Ok(hash) => hash,
        Err(e) => {
            drop(file);
            // 主动取消删除临时文件; 意外中断保留以待续传(方案决策 #3)
            if matches!(e, TransferError::Cancelled) {
                let _ = tokio::fs::remove_file(&part_path).await;
            }
            return Err(e);
        }
    };
    drop(file);

    // 尾帧校验失败(编号/哈希不符)即丢弃数据; 协议错误则保留 .part 以待续传
    if let Err(e) = expect_footer(tls, meta, &received_hash).await {
        if matches!(e, TransferError::HashMismatch { .. }) {
            let _ = tokio::fs::remove_file(&part_path).await;
        }
        return Err(e);
    }

    finalize_file(
        &target,
        &part_path,
        conflict,
        finalize_lock,
        transfer_id,
        meta,
        sink,
    )
    .await
}

/// `.part` 临时文件路径: 目标名后掺任务 ID 前缀, 不同任务的同名文件互不干扰
///
/// 命名必须确定(续传按同一规则重建路径), 故只依赖 target 与 transfer_id;
/// ID 已过 [`is_safe_transfer_id`](纯 ASCII), 取前 8 位截断安全且足以避撞。
fn part_path_of(target: &Path, transfer_id: &str) -> PathBuf {
    let tid = &transfer_id[..transfer_id.len().min(8)];
    let mut os = target.to_path_buf().into_os_string();
    os.push(format!(".{tid}{PART_SUFFIX}"));
    PathBuf::from(os)
}

/// 打开 .part 临时文件: 首传直接创建; 续传校验断点对齐并重放前段进哈希器
///
/// `size` 为整文件大小: 首传时预分配空间(磁盘不足立即失败而非写到一半),
/// 大文件不驻留页缓存。预分配不改变文件长度(断点续传依赖 len)。
async fn open_part(
    part_path: &Path,
    offset: u64,
    size: u64,
    hasher: &mut blake3::Hasher,
    buf: &mut [u8],
) -> Result<tokio::fs::File, TransferError> {
    if offset == 0 {
        let file = tokio::fs::File::create(part_path).await?;
        // 预分配失败(磁盘不足)时清掉刚建的空 .part: 无数据可续传, 留着只碍事
        if let Err(e) = super::io_tuning::preallocate(&file, size).await {
            drop(file);
            let _ = tokio::fs::remove_file(part_path).await;
            return Err(e.into());
        }
        super::io_tuning::advise_no_cache(&file, size);
        return Ok(file);
    }
    // 断点必须与 .part 当前长度一致, 否则数据将错位
    let part_len = tokio::fs::metadata(part_path).await.map(|m| m.len()).ok();
    if part_len != Some(offset) {
        return Err(TransferError::ResumeOffsetMismatch {
            offset,
            part_len: part_len.unwrap_or(0),
        });
    }
    let mut existing = tokio::fs::File::open(part_path).await?;
    super::hash_prefix(&mut existing, hasher, offset, buf).await?;
    drop(existing);
    let file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(part_path)
        .await?;
    super::io_tuning::advise_no_cache(&file, size);
    Ok(file)
}

/// 读取并校验 FileFooter: 文件编号与整文件哈希都必须匹配
async fn expect_footer(
    tls: &mut TlsStream<TcpStream>,
    meta: &FileMeta,
    received_hash: &str,
) -> Result<(), TransferError> {
    let footer = tokio::time::timeout(crate::config::DATA_IDLE_TIMEOUT, read_frame(tls))
        .await
        .map_err(|_| TransferError::Timeout("等待文件尾帧"))??;
    let ControlMessage::FileFooter {
        file_id,
        hash: expected,
    } = footer
    else {
        return Err(TransferError::Protocol(ProtocolError::Unexpected {
            expected: "file_footer",
            got: footer.kind().to_string(),
        }));
    };
    if file_id != meta.file_id || received_hash != expected {
        return Err(TransferError::HashMismatch {
            rel_path: meta.rel_path.clone(),
        });
    }
    Ok(())
}

/// 校验通过后落盘: 按冲突策略定名 → 原子重命名 → 上报事件
///
/// 定名与 rename 全程持 finalize 锁: dedup 的存在性探测到 rename 落地
/// 之间若有并发 finalize 插入, 两个同名文件会拿到同一路径互相覆盖。
async fn finalize_file(
    target: &Path,
    part_path: &Path,
    conflict: ConflictPolicy,
    finalize_lock: &tokio::sync::Mutex<()>,
    transfer_id: &str,
    meta: &FileMeta,
    sink: &EventSink,
) -> Result<(), TransferError> {
    let final_path = {
        let _guard = finalize_lock.lock().await;
        // 同名冲突: 按接收方选定的策略落盘(rename 在两平台均为原子覆盖语义)
        let final_path = match conflict {
            // 同名探测是逐个 stat 的同步调用, 放阻塞线程池执行
            ConflictPolicy::Rename => {
                let t = target.to_path_buf();
                tokio::task::spawn_blocking(move || dedup_path(&t))
                    .await
                    .map_err(|e| TransferError::Io(std::io::Error::other(e)))?
            }
            ConflictPolicy::Overwrite => target.to_path_buf(),
        };
        tokio::fs::rename(part_path, &final_path).await?;
        final_path
        // 锁到此为止: 事件通知可能因通道背压等待, 不应拖住其他文件的落盘
    };
    sink.notify(TransferEvent::FileCompleted {
        transfer_id: transfer_id.to_string(),
        file_id: meta.file_id,
        path: final_path,
    })
    .await;
    Ok(())
}

/// transfer_id 是否可安全拼入文件路径(仅允许 UUID 字符, 防路径注入)
fn is_safe_transfer_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 断点元数据文件路径(调用方须先通过 [`is_safe_transfer_id`] 校验)
fn resume_meta_path(dir: &Path, transfer_id: &str) -> PathBuf {
    dir.join(format!("{transfer_id}.resume.json"))
}

/// 写断点元数据(尽力而为: 失败只降级为不可续传, 不影响本次传输)
fn save_resume_meta(dir: &Path, transfer_id: &str, meta: &ResumeMeta) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(meta).unwrap_or_default();
        std::fs::write(resume_meta_path(dir, transfer_id), json)
    };
    if let Err(e) = write() {
        tracing::warn!(transfer_id, "断点元数据写入失败(不影响本次传输): {e}");
    }
}

/// 读断点元数据; 缺失或损坏返回 None
fn load_resume_meta(dir: &Path, transfer_id: &str) -> Option<ResumeMeta> {
    let bytes = std::fs::read(resume_meta_path(dir, transfer_id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 删除断点元数据(任务完成或取消后)
fn remove_resume_meta(dir: &Path, transfer_id: &str) {
    let _ = std::fs::remove_file(resume_meta_path(dir, transfer_id));
}

/// 内存中标记文件完成并把快照写盘(阻塞写放线程池, await 串行保证不乱序)
async fn mark_completed_persist(
    meta: &mut ResumeMeta,
    dir: &Path,
    transfer_id: &str,
    file_id: u32,
) {
    if meta.completed.contains(&file_id) {
        return;
    }
    meta.completed.push(file_id);
    let snapshot = meta.clone();
    let dir = dir.to_path_buf();
    let tid = transfer_id.to_string();
    let _ = tokio::task::spawn_blocking(move || save_resume_meta(&dir, &tid, &snapshot)).await;
}

/// 取任务表锁; 毒锁直接恢复内部数据
fn lock_pending(pending: &PendingMap) -> MutexGuard<'_, HashMap<String, PendingTransfer>> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 更新指定传输的控制状态; 任务不存在返回 false
fn set_control(pending: &PendingMap, transfer_id: &str, state: ControlState) -> bool {
    lock_pending(pending)
        .get(transfer_id)
        .map(|t| t.control.send(state).is_ok())
        .unwrap_or(false)
}

/// 取消并移除指定传输; sender 随表项 drop, 订阅方保持 Cancelled 终态
fn cancel_transfer(pending: &PendingMap, transfer_id: &str) -> bool {
    match lock_pending(pending).remove(transfer_id) {
        Some(task) => {
            let _ = task.control.send(ControlState::Cancelled);
            true
        }
        None => false,
    }
}
