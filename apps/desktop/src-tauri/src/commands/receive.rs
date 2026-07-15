//! 接收族命令: 传输请求应答、接收预检与传输控制(暂停/恢复/取消)

use std::path::{Path, PathBuf};

use deskmate_core::transfer::{ConflictPolicy, ControlState, OfferDecision, sanitize_rel_path};
use serde::Serialize;
use tauri::State;

use crate::state::{AppState, lock};

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
