//! 应用运行时状态: 引擎句柄与跨 command 共享的数据

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use deskmate_core::discovery::DiscoveryService;
use deskmate_core::identity::DeviceIdentity;
use deskmate_core::transfer::{ControlState, OfferDecision, ReceiverHandle, TransferEvent};
use tokio::sync::{mpsc, oneshot, watch};

use crate::settings::Settings;

/// 待前端决策的接收请求
pub struct PendingOffer {
    /// 决策回执(送回接收引擎)
    pub reply: oneshot::Sender<OfferDecision>,
    /// 清单内全部文件序号(M2 阶段整单接受)
    pub file_ids: Vec<u32>,
}

/// 待决策请求表: offer_id → 回执
pub type OfferMap = Arc<Mutex<HashMap<String, PendingOffer>>>;
/// 发送侧控制表: transfer_id → 暂停/取消状态源
pub type ControlMap = Arc<Mutex<HashMap<String, watch::Sender<ControlState>>>>;

/// 意外中断的发送任务(续传所需的原始参数)
pub struct InterruptedSend {
    /// 目标节点指纹
    pub fingerprint: String,
    /// 原始发送路径
    pub paths: Vec<PathBuf>,
}

/// 中断任务表: transfer_id → 续传参数(仅发送侧; 接收侧续传是被动的)
pub type InterruptedMap = Arc<Mutex<HashMap<String, InterruptedSend>>>;

/// 应用全局状态(由 tauri manage, commands 内借用)
pub struct AppState {
    /// 设备身份(昵称/头像变更时整体替换快照, 进行中的任务沿用旧快照)
    pub identity: Mutex<Arc<DeviceIdentity>>,
    /// 实际监听端口
    pub tcp_port: u16,
    /// 数据目录(身份与设置所在)
    pub data_dir: PathBuf,
    /// 接收服务句柄(接收侧暂停/取消, 下载目录热更新)
    pub receiver: ReceiverHandle,
    /// 发现服务(节点快照, 退出时 shutdown)
    pub discovery: DiscoveryService,
    /// 事件通道发送端(发送任务与接收服务共用同一事件泵)
    pub events_tx: mpsc::Sender<TransferEvent>,
    /// 待决策的接收请求
    pub offers: OfferMap,
    /// 发送侧传输控制
    pub send_controls: ControlMap,
    /// 意外中断的发送任务(等待用户点击续传)
    pub interrupted_sends: InterruptedMap,
    /// 当前设置(Arc: 事件泵按白名单自动接收时同步读取)
    pub settings: Arc<Mutex<Settings>>,
    /// 传输历史(内存为准, 追加异步落盘)
    pub history: Arc<crate::history::HistoryStore>,
}

/// 取 std Mutex 锁; 毒锁直接恢复内部数据
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 当前身份快照(Arc 克隆, 廉价; 热更新后新调用自然拿到新快照)
pub fn current_identity(state: &AppState) -> Arc<DeviceIdentity> {
    Arc::clone(&lock(&state.identity))
}
