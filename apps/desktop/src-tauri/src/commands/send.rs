//! 发送族命令: 文件/文本/剪贴板截图, 以及发送任务的公共骨架与收尾

use std::path::PathBuf;
use std::sync::Arc;

use deskmate_core::discovery::Peer;
use deskmate_core::transfer::{ControlState, TransferError, resume_send, send_files, send_text};
use serde::Serialize;
use tauri::State;
use tokio::sync::watch;

use crate::bridge::{TransferEventDto, emit_transfer_event};
use crate::state::{AppState, InterruptedMap, InterruptedSend, lock};

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

/// 暂存剪贴板截图字节(raw 二进制通道)
///
/// 大截图若走 JSON 数组参数会有数倍序列化膨胀与秒级解析开销,
/// 这里以 invoke 的原始载荷接收, 落入暂存目录后返回暂存 ID,
/// 前端随后调 send_clipboard_image 关联目标设备发出。
#[tauri::command]
pub fn stage_clipboard_image(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err("暂存接口只接受二进制载荷".to_string());
    };
    if data.is_empty() {
        return Err("截图数据为空".to_string());
    }
    let staged_id = uuid::Uuid::new_v4().to_string();
    let dir = std::env::temp_dir().join("deskmate-staging");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败: {e}"))?;
    std::fs::write(dir.join(&staged_id), data).map_err(|e| format!("写入暂存截图失败: {e}"))?;
    Ok(staged_id)
}

/// 发送已暂存的剪贴板截图: 挪入任务专属目录后走文件传输链
///
/// 对端收到的就是普通 PNG 文件(确认/白名单/进度/历史/PIN 重试全复用,
/// 协议零改动)。任务目录以 transfer_id 隔离 —— 截图文件名只有秒级
/// 精度, 同秒连发两张若共享目录会互相覆盖, 错乱先发任务的读取。
/// 临时文件不主动清理: 失败重试(interrupted_sends 登记的就是该路径)
/// 还需要它, 交由系统临时目录策略回收。
#[tauri::command]
pub async fn send_clipboard_image(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fingerprint: String,
    file_name: String,
    staged: String,
    pin: Option<String>,
) -> Result<String, String> {
    // 文件名由前端生成(screenshot-时间戳.png), 白名单校验防路径注入
    let legal_name = !file_name.is_empty()
        && !file_name.contains("..")
        && file_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !legal_name {
        return Err("非法的截图文件名".to_string());
    }
    // 暂存 ID 应为 stage_clipboard_image 返回的 UUID, 同样校验防注入
    if staged.is_empty() || !staged.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("非法的暂存 ID".to_string());
    }
    let staged_path = std::env::temp_dir().join("deskmate-staging").join(&staged);
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let task_dir = std::env::temp_dir()
        .join("deskmate-screenshots")
        .join(&transfer_id);
    std::fs::create_dir_all(&task_dir).map_err(|e| format!("创建截图任务目录失败: {e}"))?;
    let path = task_dir.join(&file_name);
    // 同卷 rename 原子且零拷贝, 暂存文件随之消失
    std::fs::rename(&staged_path, &path).map_err(|e| format!("暂存截图不存在或不可用: {e}"))?;

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

/// 按指纹查找在线节点
fn find_peer(state: &State<'_, AppState>, fingerprint: &str) -> Result<Peer, String> {
    state
        .discovery
        .peer_by_fingerprint(fingerprint)
        .ok_or_else(|| "节点已离线".to_string())
}
