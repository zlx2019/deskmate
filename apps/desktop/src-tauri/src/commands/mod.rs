//! Tauri commands: 前端调用的全部接口
//!
//! 按域拆分: [`send`] 发送族(文件/文本/剪贴板截图与任务骨架)、
//! [`receive`] 接收应答与传输控制、[`prefs`] 设置与头像;
//! 本模块保留本机信息、历史与杂项小命令。

pub mod prefs;
pub mod receive;
pub mod send;

use deskmate_core::identity::platform;
use deskmate_core::transfer::TransferError;
use serde::Serialize;
use tauri::State;

use crate::bridge::PeerDto;
use crate::state::AppState;

/// 结构化命令错误: 前端按 `code` 查本地化文案, `detail` 为不译的原始细节
/// (底层 IO 错误串/路径等, 附加在主句之后展示)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrDto {
    /// 稳定错误码(与前端 i18n 的 errors 分区键对应)
    pub code: &'static str,
    /// 细节参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl std::fmt::Display for ErrDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "{} ({d})", self.code),
            None => f.write_str(self.code),
        }
    }
}

impl ErrDto {
    /// 无细节的错误
    pub fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    /// 携带细节的错误
    pub fn with(code: &'static str, detail: impl ToString) -> Self {
        Self {
            code,
            detail: Some(detail.to_string()),
        }
    }
}

impl From<&TransferError> for ErrDto {
    fn from(e: &TransferError) -> Self {
        Self {
            code: e.code(),
            detail: e.detail(),
        }
    }
}

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

/// 删除一条传输历史(按 transfer_id 定位)
#[tauri::command]
pub fn delete_history(state: State<'_, AppState>, transfer_id: String) {
    state.history.remove(&transfer_id);
}

/// 清空全部传输历史
#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) {
    state.history.clear();
}

/// 系统通知(未聚焦才发; 供前端在窗口可能隐藏的场景反馈, 如快捷键发送结果)
#[tauri::command]
pub fn notify(app: tauri::AppHandle, title: String, body: String) {
    crate::bridge::notify_if_unfocused(&app, &title, &body);
}

/// 系统窗口材质(vibrancy/mica)是否生效; 前端据此启用半透明背景
#[tauri::command]
pub fn window_effects_active() -> bool {
    crate::WINDOW_EFFECTS_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}
