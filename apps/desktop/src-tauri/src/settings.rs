//! 应用设置: settings.json 持久化于数据目录
//!
//! 生效时机: 下载目录即时生效; 昵称与端口重启后生效(发现层广播信息启动时固定)。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 设置文件名
const SETTINGS_FILE: &str = "settings.json";
/// 本机自定义头像图片文件名(数据目录下, 前端压缩后的 JPEG)
pub const AVATAR_FILE: &str = "avatar.jpg";
/// settings.avatar 的特殊值: 使用自定义图片而非 emoji
pub const AVATAR_CUSTOM: &str = "custom";

/// 同名冲突策略(应用层概念; "每次询问"在接收确认弹窗中折算成 core 的两值枚举)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicySetting {
    /// 自动重命名: `file.txt` → `file (1).txt`(默认)
    #[default]
    Rename,
    /// 覆盖同名旧文件
    Overwrite,
    /// 每次在接收确认弹窗中询问
    Ask,
}

/// 受信设备(白名单内的节点发来的传输免确认自动接收)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    /// 证书指纹(设备身份)
    pub fingerprint: String,
    /// 加入白名单时的显示名(仅展示用, 对方改名不影响信任判定)
    pub name: String,
}

/// 用户设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// 自定义昵称; None 表示跟随 hostname
    pub display_name: Option<String>,
    /// 默认下载目录
    pub download_dir: PathBuf,
    /// TCP 监听端口(0 表示随机)
    pub tcp_port: u16,
    /// 同名冲突策略
    pub conflict_policy: ConflictPolicySetting,
    /// 头像: emoji 字符 / [`AVATAR_CUSTOM`](自定义图片) / None(首字母样式)
    pub avatar: Option<String>,
    /// 隐身模式: 只看别人不被看见(不注册 mDNS、不发 announce; 重启后生效)
    pub passive: bool,
    /// 开机自启(保存设置时即时写入系统)
    pub autostart: bool,
    /// 受信设备白名单(免确认自动接收)
    pub trusted: Vec<TrustedDevice>,
    /// 配对 PIN: 启用后对方发文件/文本必须携带正确 PIN(None 关闭, 即时生效)
    pub pin: Option<String>,
    /// 收到文本时自动复制到系统剪贴板(即时生效)
    pub auto_copy_text: bool,
    /// 发送剪贴板的全局快捷键(None 关闭; Tauri 语法, 如 "CmdOrCtrl+Shift+D")
    pub send_clipboard_hotkey: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_name: None,
            download_dir: default_download_dir(),
            tcp_port: deskmate_core::DEFAULT_TCP_PORT,
            conflict_policy: ConflictPolicySetting::default(),
            avatar: None,
            passive: false,
            autostart: false,
            trusted: Vec::new(),
            pin: None,
            auto_copy_text: false,
            send_clipboard_hotkey: Some("CmdOrCtrl+Shift+D".to_string()),
        }
    }
}

impl Settings {
    /// 从数据目录加载; 文件缺失或损坏时回退默认值
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// 持久化到数据目录
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let json = serde_json::to_vec_pretty(self).unwrap_or_default();
        std::fs::write(data_dir.join(SETTINGS_FILE), json)
    }
}

/// 默认下载目录: ~/Downloads/deskmate
fn default_download_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("deskmate")
}
