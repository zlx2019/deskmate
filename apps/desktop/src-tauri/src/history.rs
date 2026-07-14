//! 传输历史: history.json 持久化于数据目录(环形, 最新在前)
//!
//! 记录由前端在传输到达终态时上报(前端聚合状态最完整)。
//! 启动时读盘一次, 之后内存为准; 追加即时更新内存, 落盘走阻塞线程池,
//! 避免每条终态都在命令线程里做"读盘→改→全量重写"。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::state::lock;

/// 历史文件名
const HISTORY_FILE: &str = "history.json";
/// 最多保留的条数(超出丢弃最旧)
const HISTORY_CAP: usize = 200;

/// 一条传输历史
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// 传输任务 ID
    pub transfer_id: String,
    /// 方向: "send" | "recv"
    pub direction: String,
    /// 对端名称
    pub peer_name: String,
    /// 终态: completed | cancelled | interrupted | rejected
    pub status: String,
    /// 完成的文件数
    pub files_done: u32,
    /// 传输的字节数(终态时的进度值, 近似)
    pub bytes: u64,
    /// 结束时间(unix 毫秒)
    pub at: u64,
    /// 最后落盘路径("在文件夹中显示"用, 仅接收侧)
    pub last_path: Option<String>,
}

/// 历史存储: 内存为准, 追加异步落盘
pub struct HistoryStore {
    /// 数据目录(落盘位置)
    data_dir: PathBuf,
    /// 全部条目, 最新在前
    entries: Mutex<Vec<HistoryEntry>>,
}

impl HistoryStore {
    /// 从磁盘装载历史; 文件缺失或损坏视为空
    pub fn load(data_dir: &Path) -> Self {
        let entries = std::fs::read(data_dir.join(HISTORY_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            data_dir: data_dir.to_path_buf(),
            entries: Mutex::new(entries),
        }
    }

    /// 全部历史快照(最新在前)
    pub fn snapshot(&self) -> Vec<HistoryEntry> {
        lock(&self.entries).clone()
    }

    /// 追加一条(同 transfer_id 重复上报时覆盖旧条目), 随后异步落盘
    pub fn append(self: &Arc<Self>, entry: HistoryEntry) {
        {
            let mut entries = lock(&self.entries);
            entries.retain(|e| e.transfer_id != entry.transfer_id);
            entries.insert(0, entry);
            entries.truncate(HISTORY_CAP);
        }
        let store = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || store.flush());
    }

    /// 把当前内存快照写盘(阻塞调用, 只在线程池内使用)
    ///
    /// 并发追加时后台任务可能乱序执行, 但每次都取"此刻最新"快照,
    /// 最终写入的内容与内存一致。
    fn flush(&self) {
        let json = {
            let entries = lock(&self.entries);
            serde_json::to_vec_pretty(&*entries).unwrap_or_default()
        };
        if let Err(e) = std::fs::write(self.data_dir.join(HISTORY_FILE), json) {
            tracing::warn!("传输历史落盘失败: {e}");
        }
    }
}
