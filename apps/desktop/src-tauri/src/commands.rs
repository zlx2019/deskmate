//! Tauri commands: 前端调用的全部接口

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deskmate_core::discovery::Peer;
use deskmate_core::identity::platform;
use deskmate_core::transfer::{
    ConflictPolicy, ControlState, OfferDecision, TransferError, resume_send, sanitize_rel_path,
    send_files, send_text,
};
use serde::Serialize;
use tauri::State;
use tokio::sync::watch;

use crate::bridge::{PeerDto, TransferEventDto, emit_transfer_event};
use crate::settings::Settings;
use crate::state::{AppState, InterruptedMap, InterruptedSend, lock};

/// 本机信息(前端头部展示)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfInfoDto {
    /// 展示名
    pub name: String,
    /// 设备 ID
    pub device_id: String,
    /// 证书指纹
    pub fingerprint: String,
    /// 平台
    pub platform: String,
    /// 实际监听端口
    pub port: u16,
    /// 当前下载目录
    pub download_dir: String,
    /// 内置头像(emoji)
    pub avatar: Option<String>,
}

/// 查询本机信息
#[tauri::command]
pub fn get_self_info(state: State<'_, AppState>) -> SelfInfoDto {
    let identity = crate::state::current_identity(&state);
    SelfInfoDto {
        name: identity.display_name.clone(),
        device_id: identity.device_id.clone(),
        fingerprint: identity.fingerprint.clone(),
        platform: platform(),
        port: state.tcp_port,
        download_dir: state.receiver.download_dir().display().to_string(),
        avatar: identity.avatar.clone(),
    }
}

/// 当前在线节点快照(前端启动时拉取, 之后靠 peer-up/down 事件增量维护)
#[tauri::command]
pub fn list_peers(state: State<'_, AppState>) -> Vec<PeerDto> {
    state.discovery.peers().iter().map(PeerDto::from).collect()
}

/// 向指定节点发送文件/目录; 立即返回任务 ID, 进度经 transfer-event 事件推送
///
/// `pin`: 对端启用配对 PIN 时携带(前端按会话缓存自动附带);
/// 被拒且 pin_required 时前端弹 PIN 输入, 经 retry_send_transfer 重试。
#[tauri::command]
pub async fn send_files_to(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fingerprint: String,
    paths: Vec<String>,
    pin: Option<String>,
) -> Result<String, String> {
    if paths.is_empty() {
        return Err("未选择任何文件".to_string());
    }
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let path_bufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    spawn_transfer_task(
        &app,
        &state,
        transfer_id.clone(),
        fingerprint,
        path_bufs,
        SendMode::Fresh { pin },
    )?;
    Ok(transfer_id)
}

/// 发送剪贴板截图: 前端编码好的 PNG 字节落成临时文件后走文件传输链
///
/// 对端收到的就是普通 PNG 文件(确认/白名单/进度/历史/PIN 重试全复用,
/// 协议零改动)。临时文件不主动清理, 交由系统温存目录策略回收 ——
/// 失败重试(interrupted_sends 登记的就是该路径)还需要它。
#[tauri::command]
pub async fn send_clipboard_image(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fingerprint: String,
    file_name: String,
    data: Vec<u8>,
    pin: Option<String>,
) -> Result<String, String> {
    // 文件名由前端生成(screenshot-时间戳.png), 白名单校验防路径注入
    let legal = !file_name.is_empty()
        && !file_name.contains("..")
        && file_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !legal {
        return Err("非法的截图文件名".to_string());
    }
    if data.is_empty() {
        return Err("截图数据为空".to_string());
    }
    let path = std::env::temp_dir().join(&file_name);
    std::fs::write(&path, &data).map_err(|e| format!("写入临时截图失败: {e}"))?;

    let transfer_id = uuid::Uuid::new_v4().to_string();
    spawn_transfer_task(
        &app,
        &state,
        transfer_id.clone(),
        fingerprint,
        vec![path],
        SendMode::Fresh { pin },
    )?;
    Ok(transfer_id)
}

/// 用给定 PIN 重试被拒(pin_required)的发送任务, 复用原 transfer_id 与进度条目
#[tauri::command]
pub async fn retry_send_transfer(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    transfer_id: String,
    pin: Option<String>,
) -> Result<(), String> {
    let (fingerprint, paths) = interrupted_params(&state, &transfer_id, "重试")?;
    spawn_transfer_task(
        &app,
        &state,
        transfer_id,
        fingerprint,
        paths,
        SendMode::Fresh { pin },
    )
}

/// 续传意外中断的发送任务(补发缺失段, 复用原 transfer_id 与进度条目)
#[tauri::command]
pub async fn resume_send_transfer(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<(), String> {
    let (fingerprint, paths) = interrupted_params(&state, &transfer_id, "续传")?;
    spawn_transfer_task(
        &app,
        &state,
        transfer_id,
        fingerprint,
        paths,
        SendMode::Resume,
    )
}

/// 发送任务模式: 全新发送(含 PIN 重试, 从 0 起发)或断点续传
enum SendMode {
    /// 新发送 / 带 PIN 重试
    Fresh {
        /// 对端要求配对时携带的 PIN
        pin: Option<String>,
    },
    /// 续传(向对端协商断点后仅补发缺失段)
    Resume,
}

/// 从中断登记表取回任务的原始参数; 无登记时给出场景化错误
fn interrupted_params(
    state: &State<'_, AppState>,
    transfer_id: &str,
    action: &str,
) -> Result<(String, Vec<PathBuf>), String> {
    let guard = lock(&state.interrupted_sends);
    let item = guard
        .get(transfer_id)
        .ok_or_else(|| format!("该任务不可{action}(缺少原始参数)"))?;
    Ok((item.fingerprint.clone(), item.paths.clone()))
}

/// 发送任务公共骨架: 预注册控制通道 → 后台执行引擎调用 → 统一收尾
fn spawn_transfer_task(
    app: &tauri::AppHandle,
    state: &State<'_, AppState>,
    transfer_id: String,
    fingerprint: String,
    paths: Vec<PathBuf>,
    mode: SendMode,
) -> Result<(), String> {
    let peer = find_peer(state, &fingerprint)?;

    // 预注册控制通道, 前端可立即对该任务暂停/取消
    let (control_tx, control_rx) = watch::channel(ControlState::Running);
    lock(&state.send_controls).insert(transfer_id.clone(), control_tx);

    let app = app.clone();
    let identity = crate::state::current_identity(state);
    let events_tx = state.events_tx.clone();
    let controls = Arc::clone(&state.send_controls);
    let interrupted = Arc::clone(&state.interrupted_sends);
    let tid = transfer_id;
    tauri::async_runtime::spawn(async move {
        let is_resume = matches!(mode, SendMode::Resume);
        let result = match mode {
            SendMode::Fresh { pin } => send_files(
                &identity,
                &peer.addrs,
                peer.port,
                Some(peer.info.fingerprint.clone()),
                Some(tid.clone()),
                pin,
                &paths,
                control_rx,
                events_tx,
            )
            .await
            .map(|_| ()),
            SendMode::Resume => resume_send(
                &identity,
                &peer.addrs,
                peer.port,
                Some(peer.info.fingerprint.clone()),
                &tid,
                &paths,
                control_rx,
                events_tx,
            )
            .await
            .map(|_| ()),
        };
        settle_send_result(&app, &interrupted, &tid, &peer, paths, is_resume, &result);
        lock(&controls).remove(&tid);
    });
    Ok(())
}

/// 发送结束统一收尾: 补发引擎不产生的事件, 维护中断登记表
///
/// 拒绝与 PIN 门禁在引擎视角是正常应答(无事件), 续传协商失败同样无事件,
/// 都由桥接层补发; 登记表规则 —— 成功/主动取消清除, 新发送失败登记
/// (供续传或带 PIN 重试), 续传失败保留原登记(允许再次续传)。
fn settle_send_result(
    app: &tauri::AppHandle,
    interrupted: &InterruptedMap,
    transfer_id: &str,
    peer: &Peer,
    paths: Vec<PathBuf>,
    is_resume: bool,
    result: &Result<(), TransferError>,
) {
    match result {
        Ok(()) | Err(TransferError::Cancelled) => {
            lock(interrupted).remove(transfer_id);
        }
        Err(e) => {
            match e {
                TransferError::Rejected { reason } => {
                    crate::bridge::notify_if_unfocused(app, "deskmate", "对方拒绝了本次传输");
                    emit_transfer_event(
                        app,
                        TransferEventDto::Rejected {
                            transfer_id: transfer_id.to_string(),
                            reason: reason.clone(),
                            pin_required: false,
                        },
                    );
                }
                TransferError::PinRequired => {
                    emit_transfer_event(
                        app,
                        TransferEventDto::Rejected {
                            transfer_id: transfer_id.to_string(),
                            reason: Some("对方要求配对 PIN".to_string()),
                            pin_required: true,
                        },
                    );
                }
                _ if is_resume => {
                    emit_transfer_event(
                        app,
                        TransferEventDto::Interrupted {
                            transfer_id: transfer_id.to_string(),
                            reason: e.to_string(),
                        },
                    );
                }
                _ => {}
            }
            if !is_resume {
                lock(interrupted).insert(
                    transfer_id.to_string(),
                    InterruptedSend {
                        fingerprint: peer.info.fingerprint.clone(),
                        paths,
                    },
                );
            }
            tracing::warn!(transfer_id, is_resume, "发送结束(失败): {e}");
        }
    }
}

/// 文本发送结果: `pin_required` 表示对端要求配对 PIN(前端弹输入后重试)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTextOutcome {
    /// 是否因缺少/错误 PIN 被拒
    pub pin_required: bool,
}

/// 向指定节点发送文本(逐字节一致)
///
/// PIN 被拒不是异常路径, 经返回值结构化表达; 其余失败仍走错误串。
#[tauri::command]
pub async fn send_text_to(
    state: State<'_, AppState>,
    fingerprint: String,
    text: String,
    pin: Option<String>,
) -> Result<SendTextOutcome, String> {
    let peer = find_peer(&state, &fingerprint)?;
    let identity = crate::state::current_identity(&state);
    match send_text(
        &identity,
        &peer.addrs,
        peer.port,
        Some(peer.info.fingerprint.clone()),
        pin,
        &text,
    )
    .await
    {
        Ok(_) => Ok(SendTextOutcome {
            pin_required: false,
        }),
        Err(TransferError::PinRequired) => Ok(SendTextOutcome { pin_required: true }),
        Err(other) => Err(format!("发送失败: {other}")),
    }
}

/// 应答接收请求: accept=true 整单接受(可另选保存目录), false 拒绝
///
/// `overwrite` 为本次传输的同名冲突决策(true 覆盖 / false 自动重命名),
/// 由前端按设置或弹窗内用户选择折算。
#[tauri::command]
pub fn respond_offer(
    state: State<'_, AppState>,
    offer_id: String,
    accept: bool,
    save_dir: Option<String>,
    overwrite: bool,
) -> Result<(), String> {
    let pending = lock(&state.offers)
        .remove(&offer_id)
        .ok_or_else(|| "该请求已过期或已处理".to_string())?;
    let decision = if accept {
        OfferDecision::Accept {
            accepted_files: pending.file_ids,
            save_dir: save_dir.map(PathBuf::from),
            conflict: if overwrite {
                ConflictPolicy::Overwrite
            } else {
                ConflictPolicy::Rename
            },
        }
    } else {
        OfferDecision::Reject {
            reason: Some("接收方拒绝".to_string()),
        }
    };
    pending
        .reply
        .send(decision)
        .map_err(|_| "会话已断开".to_string())
}

/// 接收预检结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrecheckDto {
    /// 目标磁盘可用字节数; 查询失败为 None(前端跳过空间校验, 不阻塞接收)
    pub free_bytes: Option<u64>,
    /// 与目标目录已有文件同名的相对路径列表
    pub conflicts: Vec<String>,
}

/// 接收前预检: 查询保存目录所在磁盘的可用空间与同名冲突文件
///
/// `dir` 为 None 时用当前默认下载目录; 前端在弹窗打开与更换目录时调用。
#[tauri::command]
pub fn precheck_receive(
    state: State<'_, AppState>,
    dir: Option<String>,
    rel_paths: Vec<String>,
) -> PrecheckDto {
    let base = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| state.receiver.download_dir());
    let conflicts = rel_paths
        .into_iter()
        .filter(|rel| {
            sanitize_rel_path(rel)
                .map(|safe| base.join(safe).exists())
                .unwrap_or(false)
        })
        .collect();
    PrecheckDto {
        free_bytes: available_space_of(&base),
        conflicts,
    }
}

/// 查询目录所在磁盘的可用空间; 目录未创建时沿目录树向上探测第一个存在的祖先
fn available_space_of(dir: &Path) -> Option<u64> {
    let mut probe = dir;
    loop {
        if probe.exists() {
            return fs4::available_space(probe).ok();
        }
        probe = probe.parent()?;
    }
}

/// 暂停传输(接收侧或发送侧)
#[tauri::command]
pub fn pause_transfer(state: State<'_, AppState>, transfer_id: String) -> bool {
    set_transfer_state(&state, &transfer_id, ControlState::Paused)
}

/// 恢复传输
#[tauri::command]
pub fn resume_transfer(state: State<'_, AppState>, transfer_id: String) -> bool {
    set_transfer_state(&state, &transfer_id, ControlState::Running)
}

/// 取消传输(接收中的 .part 会被删除)
#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, transfer_id: String) -> bool {
    // 接收侧任务由 ReceiverHandle 管理
    if state.receiver.cancel(&transfer_id) {
        return true;
    }
    // 发送侧: 置 Cancelled 后移除注册(任务自会收尾)
    match lock(&state.send_controls).remove(&transfer_id) {
        Some(tx) => tx.send(ControlState::Cancelled).is_ok(),
        None => false,
    }
}

/// 上传本机自定义头像图片(前端已压缩为 ~128×128 JPEG); 重启后随广播生效
///
/// 只落盘图片文件, `settings.avatar = "custom"` 由前端经 save_settings 统一持久化。
#[tauri::command]
pub fn set_avatar_image(state: State<'_, AppState>, data: Vec<u8>) -> Result<(), String> {
    if data.is_empty() {
        return Err("图片数据为空".to_string());
    }
    if data.len() as u64 > deskmate_core::protocol::MAX_AVATAR_SIZE {
        return Err("图片超过 256KB 上限".to_string());
    }
    std::fs::write(state.data_dir.join(crate::settings::AVATAR_FILE), &data)
        .map_err(|e| format!("保存头像失败: {e}"))
}

/// 读取头像图片字节: hash 为 None 取本机自定义头像, Some 查对端缓存
///
/// 缓存未命中返回 None(拉取完成后有 avatar-ready 事件, 前端届时重取)。
#[tauri::command]
pub fn get_avatar_image(state: State<'_, AppState>, hash: Option<String>) -> Option<Vec<u8>> {
    let path = match hash {
        None => state.data_dir.join(crate::settings::AVATAR_FILE),
        Some(h) => {
            // 哈希由前端从对端广播中提取, 不可信, 仅允许 hex(防路径注入)
            if !crate::bridge::is_safe_hash(&h) {
                return None;
            }
            state
                .data_dir
                .join(crate::bridge::AVATAR_CACHE_DIR)
                .join(format!("{h}.jpg"))
        }
    };
    std::fs::read(path).ok()
}

/// 读取传输历史(最新在前, 内存快照)
#[tauri::command]
pub fn get_history(state: State<'_, AppState>) -> Vec<crate::history::HistoryEntry> {
    state.history.snapshot()
}

/// 追加一条传输历史(前端在传输到达终态时上报; 落盘异步完成)
#[tauri::command]
pub fn append_history(state: State<'_, AppState>, entry: crate::history::HistoryEntry) {
    state.history.append(entry);
}

/// 系统通知(未聚焦才发; 供前端在窗口可能隐藏的场景反馈, 如快捷键发送结果)
#[tauri::command]
pub fn notify(app: tauri::AppHandle, title: String, body: String) {
    crate::bridge::notify_if_unfocused(&app, &title, &body);
}

/// 应用"发送剪贴板"全局快捷键: 注销旧值后注册新值(None/空 = 仅注销)
///
/// 返回错误字符串供设置保存时回显(格式非法或与其他应用冲突)。
pub(crate) fn apply_clipboard_hotkey(
    app: &tauri::AppHandle,
    old: Option<&str>,
    new: Option<&str>,
) -> Result<(), String> {
    use tauri::Emitter;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let gs = app.global_shortcut();
    // 旧值解析失败说明从未注册成功, 忽略即可
    if let Some(old) = old.filter(|s| !s.is_empty())
        && let Ok(sc) = old.parse::<Shortcut>()
    {
        let _ = gs.unregister(sc);
    }
    let Some(new) = new.filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let sc: Shortcut = new.parse().map_err(|e| format!("快捷键格式无效: {e}"))?;
    gs.on_shortcut(sc, |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            // 目标设备与 PIN 会话缓存都在前端, 通知前端读剪贴板并发送
            if let Err(e) = app.emit(crate::bridge::events::HOTKEY_SEND_CLIPBOARD, ()) {
                tracing::debug!("快捷键事件发射失败: {e}");
            }
        }
    })
    .map_err(|e| format!("快捷键注册失败(可能与其他应用冲突): {e}"))
}

/// 系统窗口材质(vibrancy/mica)是否生效; 前端据此启用半透明背景
#[tauri::command]
pub fn window_effects_active() -> bool {
    crate::WINDOW_EFFECTS_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// 读取当前设置
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    lock(&state.settings).clone()
}

/// 保存设置; 除端口与隐身模式(监听/广播启动时固定, 重启后生效)外均即时生效
#[tauri::command]
pub fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), String> {
    std::fs::create_dir_all(&settings.download_dir).map_err(|e| format!("下载目录不可用: {e}"))?;
    // 全局快捷键先于落盘应用: 注册失败(格式/冲突)时整个保存失败, 避免"文件已存新值但未生效"
    {
        let old = lock(&state.settings).send_clipboard_hotkey.clone();
        if old != settings.send_clipboard_hotkey {
            apply_clipboard_hotkey(
                &app,
                old.as_deref(),
                settings.send_clipboard_hotkey.as_deref(),
            )?;
        }
    }
    settings
        .save(&state.data_dir)
        .map_err(|e| format!("保存设置失败: {e}"))?;
    state
        .receiver
        .set_download_dir(settings.download_dir.clone());
    // 昵称/头像即时生效: 重建身份快照并同步引擎各持有方
    // (进行中的传输沿用旧快照收尾; 指纹与证书不变, 对端信任关系不受影响)
    {
        let old = crate::state::current_identity(&state);
        let avatar_image = crate::bridge::load_avatar_image(&settings, &state.data_dir);
        let identity =
            crate::bridge::build_identity(&state.data_dir, &settings, avatar_image.as_deref())
                .map_err(|e| format!("更新身份失败: {e}"))?;
        // 展示字段有变化才向局域网重新广播, 避免无谓打扰
        if identity.display_name != old.display_name || identity.avatar != old.avatar {
            state
                .receiver
                .set_self_info(identity.peer_info(), avatar_image);
            state.discovery.update_info(&identity.peer_info());
            *lock(&state.identity) = identity;
        }
    }
    // 开机自启即时写入系统(enable/disable 幂等, 直接按新值同步)
    {
        use tauri_plugin_autostart::ManagerExt;
        let launcher = app.autolaunch();
        let result = if settings.autostart {
            launcher.enable()
        } else {
            launcher.disable()
        };
        if let Err(e) = result {
            tracing::warn!("同步开机自启状态失败: {e}");
        }
    }
    // 配对 PIN 即时生效(空串视为关闭)
    state
        .receiver
        .set_pin(settings.pin.clone().filter(|p| !p.is_empty()));
    *lock(&state.settings) = settings;
    Ok(())
}

/// 按指纹查找在线节点
fn find_peer(state: &State<'_, AppState>, fingerprint: &str) -> Result<Peer, String> {
    state
        .discovery
        .peer_by_fingerprint(fingerprint)
        .ok_or_else(|| "节点已离线".to_string())
}

/// 同时尝试接收侧与发送侧的状态设置
fn set_transfer_state(state: &State<'_, AppState>, transfer_id: &str, s: ControlState) -> bool {
    let receiver_side = match s {
        ControlState::Paused => state.receiver.pause(transfer_id),
        ControlState::Running => state.receiver.resume(transfer_id),
        ControlState::Cancelled => state.receiver.cancel(transfer_id),
    };
    if receiver_side {
        return true;
    }
    lock(&state.send_controls)
        .get(transfer_id)
        .map(|tx| tx.send(s).is_ok())
        .unwrap_or(false)
}
