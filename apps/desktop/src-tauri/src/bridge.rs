//! 引擎桥接层: 启动 deskmate-core 引擎, 把引擎事件泵送为 Tauri 事件
//!
//! 前端事件约定:
//! - `peer-up` / `peer-down`  — 节点上线(PeerDto)/ 下线(指纹字符串)
//! - `transfer-offer`         — 收到传输请求, 等待用户决策(OfferDto)
//! - `transfer-event`         — 传输过程事件(TransferEventDto, kind 区分)

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use deskmate_core::DEFAULT_DISCOVERY_PORT;
use deskmate_core::discovery::{DiscoveryService, Peer, PeerEvent};
use deskmate_core::identity::DeviceIdentity;
use deskmate_core::protocol::FileMeta;
use deskmate_core::transfer::{
    ConflictPolicy, OfferDecision, ReceiverOptions, TransferEvent, TransferOffer, bind_dual_stack,
    fetch_avatar, spawn_receiver,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::settings::{AVATAR_CUSTOM, AVATAR_FILE, ConflictPolicySetting, Settings};
use crate::state::{AppState, ControlMap, InterruptedMap, OfferMap, PendingOffer, lock};

/// 头像缓存子目录名(数据目录下, 文件名 = <hash>.jpg)
pub const AVATAR_CACHE_DIR: &str = "avatars";

/// Tauri 事件名: 与前端 src/events.ts 一一对应, 改名必须两端同步
pub mod events {
    /// 节点上线(载荷 PeerDto)
    pub const PEER_UP: &str = "peer-up";
    /// 节点下线(载荷为指纹字符串)
    pub const PEER_DOWN: &str = "peer-down";
    /// 收到传输请求, 等待用户决策(载荷 OfferDto)
    pub const TRANSFER_OFFER: &str = "transfer-offer";
    /// 传输过程事件(载荷 TransferEventDto)
    pub const TRANSFER_EVENT: &str = "transfer-event";
    /// 白名单自动接收已开始(载荷 AutoStartDto)
    pub const TRANSFER_AUTOSTART: &str = "transfer-autostart";
    /// 对端头像缓存就绪(载荷 AvatarReadyDto)
    pub const AVATAR_READY: &str = "avatar-ready";
}

/// 发射传输事件到前端(发送失败只记录, 不影响引擎)
pub(crate) fn emit_transfer_event(app: &AppHandle, dto: TransferEventDto) {
    if let Err(e) = app.emit(events::TRANSFER_EVENT, dto) {
        tracing::debug!("transfer-event 发射失败: {e}");
    }
}

/// 进度事件转发的最小间隔(节流至约 10Hz, 高速传输时每秒可达上百个原始事件)
const PROGRESS_EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// 拉取中的头像哈希集合(去重, 防止同一头像并发重复拉取)
type InflightAvatars = Arc<Mutex<HashSet<String>>>;

/// 节点信息(前端展示)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerDto {
    /// 设备 ID
    pub device_id: String,
    /// 展示名
    pub name: String,
    /// 证书指纹(hex, 前端也用作节点 key)
    pub fingerprint: String,
    /// 平台(macos/windows/linux)
    pub platform: String,
    /// 候选地址(展示用)
    pub addrs: Vec<String>,
    /// TCP 端口
    pub port: u16,
    /// 内置头像(emoji); None 时前端用首字母样式
    pub avatar: Option<String>,
}

impl From<&Peer> for PeerDto {
    fn from(p: &Peer) -> Self {
        Self {
            device_id: p.info.device_id.clone(),
            name: p.info.name.clone(),
            fingerprint: p.info.fingerprint.clone(),
            platform: p.info.platform.clone(),
            addrs: p.addrs.iter().map(|a| a.to_string()).collect(),
            port: p.port,
            avatar: p.info.avatar.clone(),
        }
    }
}

/// 清单文件项(前端展示)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetaDto {
    /// 文件序号
    pub file_id: u32,
    /// 相对路径
    pub rel_path: String,
    /// 字节数
    pub size: u64,
}

impl From<&FileMeta> for FileMetaDto {
    fn from(f: &FileMeta) -> Self {
        Self {
            file_id: f.file_id,
            rel_path: f.rel_path.clone(),
            size: f.size,
        }
    }
}

/// 传输请求(待前端决策)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferDto {
    /// 决策回执 ID(respond_offer 用)
    pub offer_id: String,
    /// 传输任务 ID
    pub transfer_id: String,
    /// 发送方名称
    pub peer_name: String,
    /// 发送方指纹
    pub peer_fingerprint: String,
    /// 发送方平台
    pub peer_platform: String,
    /// 发送方头像(emoji)
    pub peer_avatar: Option<String>,
    /// 文件清单
    pub files: Vec<FileMetaDto>,
    /// 总字节数
    pub total_size: u64,
}

/// 传输过程事件(kind 字段区分类型)
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TransferEventDto {
    /// 单文件进度
    Progress {
        transfer_id: String,
        file_id: u32,
        rel_path: String,
        done: u64,
        size: u64,
    },
    /// 单文件完成
    FileCompleted {
        transfer_id: String,
        file_id: u32,
        path: String,
    },
    /// 任务完成
    Completed { transfer_id: String },
    /// 任务取消(临时文件已删)
    Cancelled { transfer_id: String },
    /// 意外中断(.part 保留)
    Interrupted { transfer_id: String, reason: String },
    /// 对端拒绝(发送侧); pin_required 表示拒因是缺少或错误的配对 PIN
    Rejected {
        transfer_id: String,
        reason: Option<String>,
        pin_required: bool,
    },
    /// 收到文本
    TextReceived {
        from_name: String,
        from_fingerprint: String,
        text: String,
    },
}

impl From<TransferEvent> for TransferEventDto {
    fn from(ev: TransferEvent) -> Self {
        match ev {
            TransferEvent::Progress {
                transfer_id,
                file_id,
                rel_path,
                done,
                size,
            } => Self::Progress {
                // 引擎侧为共享引用(Arc<str>), DTO 序列化需要拥有权
                transfer_id: transfer_id.to_string(),
                file_id,
                rel_path: rel_path.to_string(),
                done,
                size,
            },
            TransferEvent::FileCompleted {
                transfer_id,
                file_id,
                path,
            } => Self::FileCompleted {
                transfer_id,
                file_id,
                path: path.display().to_string(),
            },
            TransferEvent::Completed { transfer_id } => Self::Completed { transfer_id },
            TransferEvent::Cancelled { transfer_id } => Self::Cancelled { transfer_id },
            TransferEvent::Interrupted {
                transfer_id,
                reason,
            } => Self::Interrupted {
                transfer_id,
                reason,
            },
            TransferEvent::TextReceived { from, text } => Self::TextReceived {
                from_name: from.name,
                from_fingerprint: from.fingerprint,
                text,
            },
        }
    }
}

/// 读取自定义头像图片; 设置未选自定义头像或读取失败时返回 None
pub(crate) fn load_avatar_image(settings: &Settings, data_dir: &Path) -> Option<Vec<u8>> {
    if settings.avatar.as_deref() != Some(AVATAR_CUSTOM) {
        return None;
    }
    match std::fs::read(data_dir.join(AVATAR_FILE)) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            tracing::warn!("读取自定义头像失败({e}), 回退默认样式");
            None
        }
    }
}

/// 装配设备身份: 设置的昵称优先于 hostname, 头像按类型注入广播字段
///
/// emoji 头像直接进身份; 自定义图片以 `img:<hash>` 广播, 本体走 TCP 按需拉取。
/// 启动与设置热更新共用(每次从磁盘装载同一证书, 指纹恒定)。
pub(crate) fn build_identity(
    data_dir: &Path,
    settings: &Settings,
    avatar_image: Option<&[u8]>,
) -> Result<Arc<DeviceIdentity>> {
    let mut identity = DeviceIdentity::load_or_create(data_dir).context("加载设备身份失败")?;
    if let Some(name) = &settings.display_name
        && !name.trim().is_empty()
    {
        identity.display_name = name.clone();
    }
    identity.avatar = match avatar_image {
        Some(img) => Some(format!("img:{}", blake3::hash(img).to_hex())),
        None => settings.avatar.clone().filter(|a| a != AVATAR_CUSTOM),
    };
    Ok(Arc::new(identity))
}

/// 绑定 TCP 监听(IPv6 双栈优先): 配置端口被占用时回退随机端口
/// (发现层会广播真实端口)
async fn bind_listener(port: u16) -> Result<TcpListener> {
    match bind_dual_stack(port).await {
        Ok(l) => Ok(l),
        Err(e) => {
            tracing::warn!("端口 {port} 监听失败({e}), 回退随机端口");
            bind_dual_stack(0).await.context("监听失败")
        }
    }
}

/// 启动核心引擎: 身份 → 监听 → 接收服务 → 发现服务 → 事件泵
pub async fn start_engine(app: AppHandle, data_dir: PathBuf) -> Result<AppState> {
    // Arc 共享: commands 读写 + offers 泵按白名单自动接收;
    // 启动阶段用一次性快照, 避免逐处加锁
    let shared_settings = Arc::new(Mutex::new(Settings::load(&data_dir)));
    let settings = lock(&shared_settings).clone();

    let avatar_image = load_avatar_image(&settings, &data_dir);
    let identity = build_identity(&data_dir, &settings, avatar_image.as_deref())?;

    tokio::fs::create_dir_all(&settings.download_dir)
        .await
        .context("创建下载目录失败")?;

    let listener = bind_listener(settings.tcp_port).await?;
    let tcp_port = listener.local_addr().context("读取监听地址失败")?.port();

    let (offers_tx, offers_rx) = mpsc::channel::<TransferOffer>(16);
    let (events_tx, events_rx) = mpsc::channel::<TransferEvent>(256);
    let receiver = spawn_receiver(
        Arc::clone(&identity),
        listener,
        ReceiverOptions {
            download_dir: settings.download_dir.clone(),
            avatar_image,
            resume_dir: data_dir.join("resume"),
            pin: settings.pin.clone().filter(|p| !p.is_empty()),
        },
        offers_tx,
        events_tx.clone(),
    )
    .context("启动接收服务失败")?;

    // passive = 隐身模式: 只浏览不广播(方案 P1 项, 重启后生效)
    let (discovery, peers_rx) = DiscoveryService::start(
        identity.peer_info(),
        tcp_port,
        DEFAULT_DISCOVERY_PORT,
        settings.passive,
    )
    .await
    .context("启动发现服务失败")?;

    let offers: OfferMap = Arc::new(Mutex::new(HashMap::new()));
    let send_controls: ControlMap = Arc::new(Mutex::new(HashMap::new()));
    spawn_pumps(
        app,
        peers_rx,
        events_rx,
        offers_rx,
        Arc::clone(&offers),
        Arc::clone(&identity),
        data_dir.join(AVATAR_CACHE_DIR),
        Arc::clone(&shared_settings),
    );

    tracing::info!(
        name = %identity.display_name,
        port = tcp_port,
        "deskmate 引擎已启动"
    );
    let history = Arc::new(crate::history::HistoryStore::load(&data_dir));
    Ok(AppState {
        identity: Mutex::new(identity),
        tcp_port,
        data_dir,
        receiver,
        discovery,
        events_tx,
        offers,
        send_controls,
        interrupted_sends: Arc::new(Mutex::new(HashMap::new())) as InterruptedMap,
        settings: shared_settings,
        history,
    })
}

/// 启动三路事件泵: 节点事件 / 传输事件 / 接收请求
#[expect(
    clippy::too_many_arguments,
    reason = "内部装配函数, 参数即泵的完整上下文"
)]
fn spawn_pumps(
    app: AppHandle,
    mut peers_rx: mpsc::Receiver<PeerEvent>,
    events_rx: mpsc::Receiver<TransferEvent>,
    mut offers_rx: mpsc::Receiver<TransferOffer>,
    offers: OfferMap,
    identity: Arc<DeviceIdentity>,
    avatar_cache: PathBuf,
    settings: Arc<Mutex<Settings>>,
) {
    let peer_app = app.clone();
    let inflight: InflightAvatars = Arc::new(Mutex::new(HashSet::new()));
    tauri::async_runtime::spawn(async move {
        while let Some(event) = peers_rx.recv().await {
            let _ = match event {
                PeerEvent::Up(p) => {
                    // 广播 img: 头像且缓存未命中时后台拉取, 不阻塞事件泵
                    ensure_peer_avatar(&peer_app, &identity, &avatar_cache, &inflight, &p);
                    peer_app.emit(events::PEER_UP, PeerDto::from(&p))
                }
                PeerEvent::Down(fp) => peer_app.emit(events::PEER_DOWN, fp),
            };
        }
    });

    tauri::async_runtime::spawn(pump_transfer_events(app.clone(), events_rx));

    tauri::async_runtime::spawn(async move {
        while let Some(offer) = offers_rx.recv().await {
            // 白名单命中: 免确认自动接收, 不进待决策队列
            let Some(offer) = try_auto_accept(&app, &settings, offer) else {
                continue;
            };
            let offer_id = uuid::Uuid::new_v4().to_string();
            let dto = OfferDto {
                offer_id: offer_id.clone(),
                transfer_id: offer.transfer_id.clone(),
                peer_name: offer.peer.name.clone(),
                peer_fingerprint: offer.peer.fingerprint.clone(),
                peer_platform: offer.peer.platform.clone(),
                peer_avatar: offer.peer.avatar.clone(),
                files: offer.files.iter().map(FileMetaDto::from).collect(),
                total_size: offer.total_size,
            };
            lock(&offers).insert(
                offer_id,
                PendingOffer {
                    reply: offer.reply,
                    file_ids: offer.files.iter().map(|f| f.file_id).collect(),
                },
            );
            notify_if_unfocused(
                &app,
                "收到传输请求",
                &format!(
                    "{} 想发送 {} 个文件({})",
                    dto.peer_name,
                    dto.files.len(),
                    human_bytes(dto.total_size)
                ),
            );
            let _ = app.emit(events::TRANSFER_OFFER, dto);
        }
    });
}

/// 传输事件泵: 进度事件按任务节流转发, 其余事件全量转发并按需发系统通知
///
/// 千兆速度下引擎每秒产生上百个 Progress(每 1MiB 一个), 逐个转发会让
/// 前端每个事件重渲一轮; 节流到约 10Hz 后肉眼无差而渲染压力骤降。
/// 顺带把活跃传输的聚合进度同步到系统任务栏/Dock。
async fn pump_transfer_events(app: AppHandle, mut events_rx: mpsc::Receiver<TransferEvent>) {
    // 每任务上次转发进度的时刻, 任务终态时清理
    let mut last_progress: HashMap<String, std::time::Instant> = HashMap::new();
    // 活跃传输的当前文件进度(transfer_id → (done, size)), 供系统进度条聚合
    let mut active: HashMap<String, (u64, u64)> = HashMap::new();
    while let Some(event) = events_rx.recv().await {
        let mut progress_dirty = false;
        match &event {
            TransferEvent::Progress {
                transfer_id,
                done,
                size,
                ..
            } => {
                active.insert(transfer_id.to_string(), (*done, *size));
                let now = std::time::Instant::now();
                let due = last_progress
                    .get(transfer_id.as_ref())
                    .is_none_or(|t| now.duration_since(*t) >= PROGRESS_EMIT_INTERVAL);
                // 文件末尾进度(done == size)必须放行, 否则条目会卡在临近完成处
                if !due && done < size {
                    continue;
                }
                last_progress.insert(transfer_id.to_string(), now);
                progress_dirty = true;
            }
            TransferEvent::Completed { transfer_id }
            | TransferEvent::Cancelled { transfer_id }
            | TransferEvent::Interrupted { transfer_id, .. } => {
                last_progress.remove(transfer_id);
                active.remove(transfer_id);
                progress_dirty = true;
            }
            // 收到文本: 按设置自动写入系统剪贴板
            TransferEvent::TextReceived { text, .. } => auto_copy_text(&app, text),
            _ => {}
        }
        if progress_dirty {
            update_taskbar_progress(&app, &active);
        }
        notify_transfer_event(&app, &event);
        emit_transfer_event(&app, TransferEventDto::from(event));
    }
}

/// 把活跃传输的聚合进度同步到系统任务栏(Windows)/ Dock 图标(macOS)
///
/// 展示的是"正在传的文件"聚合进度 —— 引擎事件不含任务总量,
/// 多文件任务会逐个文件打满, 与系统下载类进度的惯例一致。
/// Linux 仅 libunity 桌面支持, 失败静默忽略。
fn update_taskbar_progress(app: &AppHandle, active: &HashMap<String, (u64, u64)>) {
    use tauri::window::{ProgressBarState, ProgressBarStatus};
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let state = if active.is_empty() {
        ProgressBarState {
            status: Some(ProgressBarStatus::None),
            progress: None,
        }
    } else {
        let (done, size) = active
            .values()
            .fold((0u64, 0u64), |(d, s), (fd, fs)| (d + fd, s + fs));
        let pct = (done * 100).checked_div(size).unwrap_or(0).min(100);
        ProgressBarState {
            status: Some(ProgressBarStatus::Normal),
            progress: Some(pct),
        }
    };
    if let Err(e) = window.set_progress_bar(state) {
        tracing::debug!("系统进度条更新失败: {e}");
    }
}

/// 窗口未聚焦期间累计的未读事件数(与系统通知同源: 发通知即 +1, 亮窗清零)
static UNREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 清空未读角标(亮窗或窗口获得焦点时调用)
pub(crate) fn clear_unread(app: &AppHandle) {
    if UNREAD.swap(0, std::sync::atomic::Ordering::Relaxed) != 0 {
        apply_badge(app, 0);
    }
}

/// 未读 +1 并刷新角标(仅在发出系统通知的路径上调用)
fn bump_unread(app: &AppHandle) {
    let n = UNREAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    apply_badge(app, n);
}

/// 按平台呈现未读数: macOS Dock 数字角标 / Windows 任务栏红点覆盖图标
#[cfg_attr(
    target_os = "linux",
    expect(unused_variables, reason = "Linux 无通用角标协议")
)]
fn apply_badge(app: &AppHandle, count: u32) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        let badge = if count == 0 { None } else { Some(count as i64) };
        if let Err(e) = window.set_badge_count(badge) {
            tracing::debug!("Dock 角标更新失败: {e}");
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Windows 不支持数字角标, 用红点覆盖图标示意"有未读"
        let icon = if count == 0 {
            None
        } else {
            tauri::image::Image::from_bytes(include_bytes!("../icons/unread-dot.png")).ok()
        };
        if let Err(e) = window.set_overlay_icon(icon) {
            tracing::debug!("任务栏覆盖图标更新失败: {e}");
        }
    }
    #[cfg(target_os = "linux")]
    let _ = window;
}

/// 确保对端 img: 头像的本地缓存: 未命中时后台拉取, 成功落盘后发 avatar-ready 事件
///
/// 前端拿到 peer 后自行调 get_avatar_image 读缓存; 此处只负责补缺,
/// 因此命中缓存时无需通知。哈希不匹配(对端刚换头像)按丢弃处理, 等广播刷新。
fn ensure_peer_avatar(
    app: &AppHandle,
    identity: &Arc<DeviceIdentity>,
    cache_dir: &Path,
    inflight: &InflightAvatars,
    peer: &Peer,
) {
    let Some(hash) = peer
        .info
        .avatar
        .as_deref()
        .and_then(|a| a.strip_prefix("img:"))
    else {
        return;
    };
    if !is_safe_hash(hash) {
        return;
    }
    let cache_path = cache_dir.join(format!("{hash}.jpg"));
    if cache_path.exists() {
        return;
    }
    // 同一哈希只允许一个拉取任务在途
    if !lock(inflight).insert(hash.to_string()) {
        return;
    }

    let app = app.clone();
    let identity = Arc::clone(identity);
    let inflight = Arc::clone(inflight);
    let hash = hash.to_string();
    let fingerprint = peer.info.fingerprint.clone();
    let (addrs, port) = (peer.addrs.clone(), peer.port);
    tauri::async_runtime::spawn(async move {
        match fetch_avatar(&identity, &addrs, port, Some(fingerprint.clone())).await {
            // fetch_avatar 已校验数据与应答哈希一致, 此处再核对广播声明的哈希
            Ok(Some((got, data))) if got == hash => {
                let ok = async {
                    if let Some(parent) = cache_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&cache_path, &data).await
                }
                .await;
                match ok {
                    Ok(()) => {
                        let _ = app.emit(
                            events::AVATAR_READY,
                            AvatarReadyDto {
                                fingerprint,
                                hash: hash.clone(),
                            },
                        );
                    }
                    Err(e) => tracing::warn!("头像缓存写入失败: {e}"),
                }
            }
            Ok(Some(_)) => tracing::debug!("头像哈希与广播不符, 丢弃并等待广播刷新"),
            Ok(None) => tracing::debug!("对端已取消头像"),
            Err(e) => tracing::debug!("拉取头像失败: {e}"),
        }
        lock(&inflight).remove(&hash);
    });
}

/// 白名单自动接收: 命中则直接应答 Accept 并通知, 返回 None; 未命中原样返还
fn try_auto_accept(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    offer: TransferOffer,
) -> Option<TransferOffer> {
    let conflict = {
        let guard = lock(settings);
        if !guard
            .trusted
            .iter()
            .any(|t| t.fingerprint == offer.peer.fingerprint)
        {
            return Some(offer);
        }
        // 无人在环, "每次询问"折算为安全默认(自动重命名)
        match guard.conflict_policy {
            ConflictPolicySetting::Overwrite => ConflictPolicy::Overwrite,
            _ => ConflictPolicy::Rename,
        }
    };
    let decision = OfferDecision::Accept {
        accepted_files: offer.files.iter().map(|f| f.file_id).collect(),
        save_dir: None,
        conflict,
    };
    if offer.reply.send(decision).is_err() {
        // 会话已断开, 静默丢弃
        return None;
    }
    notify_if_unfocused(
        app,
        "deskmate",
        &format!(
            "已自动接收来自 {} 的 {} 个文件({})",
            offer.peer.name,
            offer.files.len(),
            human_bytes(offer.total_size)
        ),
    );
    // 前端据此建立进度条目(不弹确认窗)
    let _ = app.emit(
        events::TRANSFER_AUTOSTART,
        AutoStartDto {
            transfer_id: offer.transfer_id.clone(),
            peer_name: offer.peer.name.clone(),
        },
    );
    None
}

/// 白名单自动接收开始事件(前端建立接收进度条目)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoStartDto {
    /// 传输任务 ID
    transfer_id: String,
    /// 发送方名称
    peer_name: String,
}

/// 头像缓存就绪事件(前端据此重新读取缓存)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvatarReadyDto {
    /// 节点指纹
    fingerprint: String,
    /// 头像哈希
    hash: String,
}

/// 头像哈希是否安全: 仅允许 hex 字符且长度受限(用于拼接缓存文件名, 防路径注入)
pub(crate) fn is_safe_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.len() <= 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// 收到文本时按设置自动写入系统剪贴板(开关默认关闭, 保存设置即时生效)
fn auto_copy_text(app: &AppHandle, text: &str) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let enabled = lock(&app.state::<AppState>().settings).auto_copy_text;
    if !enabled {
        return;
    }
    if let Err(e) = app.clipboard().write_text(text.to_string()) {
        tracing::debug!("自动复制文本到剪贴板失败: {e}");
    }
}

/// 传输关键节点发系统通知(进度类高频事件不通知)
fn notify_transfer_event(app: &AppHandle, event: &TransferEvent) {
    match event {
        TransferEvent::Completed { .. } => notify_if_unfocused(app, "deskmate", "文件传输完成"),
        TransferEvent::Cancelled { .. } => notify_if_unfocused(app, "deskmate", "传输已取消"),
        TransferEvent::Interrupted { .. } => {
            notify_if_unfocused(app, "deskmate", "传输意外中断, 未完成部分已保留");
        }
        TransferEvent::TextReceived { from, text } => {
            notify_if_unfocused(app, &format!("{} 发来文本", from.name), &preview_of(text));
        }
        _ => {}
    }
}

/// 窗口未聚焦或已隐入托盘时发系统通知; 聚焦时应用内已可见, 不重复打扰
///
/// 同一路径顺带累计未读角标(通知与角标同源, 亮窗时一并清零)。
pub(crate) fn notify_if_unfocused(app: &AppHandle, title: &str, body: &str) {
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    bump_unread(app);
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!("系统通知发送失败: {e}");
    }
}

/// 文本预览: 取首行前 60 个字符, 通知栏放不下完整内容
fn preview_of(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("");
    let mut preview: String = first_line.chars().take(60).collect();
    if preview.len() < text.len() {
        preview.push('…');
    }
    preview
}

/// 字节数人性化: 1536 → "1.5 KB"
pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
