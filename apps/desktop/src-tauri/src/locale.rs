//! 桌面壳用户可见文案(托盘菜单 + 系统通知)的双语文案表
//!
//! 前端界面文案在 apps/desktop/src/i18n/(zh.ts / en.ts), 与本文件分开维护。
//! 语言取自设置(settings.language, 由前端首启按系统语言检测写入);
//! 设置尚未初始化时退化读 LANG 环境变量(macOS GUI 进程常缺失 → 英文,
//! 前端初始化后写回设置并热更新托盘, 只影响首启头几秒)。

use tauri::Manager;

use crate::state::{AppState, lock};

/// 支持的界面语言
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 中文
    Zh,
    /// 英文
    En,
}

impl Lang {
    /// 从设置值解析; 未初始化(空/未知)时按环境变量兜底
    pub fn from_settings(value: &str) -> Self {
        match value {
            "zh" => Lang::Zh,
            "en" => Lang::En,
            _ => Self::system_fallback(),
        }
    }

    /// 环境变量兜底判断(仅设置未初始化时使用)
    fn system_fallback() -> Self {
        let lang_env = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default();
        if lang_env.to_lowercase().starts_with("zh") {
            Lang::Zh
        } else {
            Lang::En
        }
    }
}

/// 桌面壳文案表(带 {name} 占位的字段经辅助方法填充)
pub struct ShellTexts {
    /// 托盘菜单: 显示主窗口
    pub tray_show: &'static str,
    /// 托盘菜单: 退出
    pub tray_quit: &'static str,
    /// 通知: 传输完成
    pub transfer_completed: &'static str,
    /// 通知: 传输已取消
    pub transfer_cancelled: &'static str,
    /// 通知: 传输意外中断
    pub transfer_interrupted: &'static str,
    /// 通知: 对方拒绝了本次传输
    pub transfer_rejected: &'static str,
    /// 通知标题模板: 收到文本({name} = 发送者昵称)
    text_from: &'static str,
    /// 通知标题后缀: 已自动复制到剪贴板
    text_copied_suffix: &'static str,
}

impl ShellTexts {
    /// 组装"收到文本"通知标题; `copied` 为自动复制已生效
    pub fn text_from(&self, name: &str, copied: bool) -> String {
        let base = self.text_from.replace("{name}", name);
        if copied {
            format!("{base}{}", self.text_copied_suffix)
        } else {
            base
        }
    }
}

/// 中文文案
const ZH: ShellTexts = ShellTexts {
    tray_show: "显示 deskmate",
    tray_quit: "退出",
    transfer_completed: "文件传输完成",
    transfer_cancelled: "传输已取消",
    transfer_interrupted: "传输意外中断, 未完成部分已保留",
    transfer_rejected: "对方拒绝了本次传输",
    text_from: "{name} 发来文本",
    text_copied_suffix: " · 已复制",
};

/// 英文文案
const EN: ShellTexts = ShellTexts {
    tray_show: "Show deskmate",
    tray_quit: "Quit",
    transfer_completed: "Transfer completed",
    transfer_cancelled: "Transfer cancelled",
    transfer_interrupted: "Transfer interrupted, partial data kept for resume",
    transfer_rejected: "Peer declined the transfer",
    text_from: "Text from {name}",
    text_copied_suffix: " · copied",
};

/// 按语言取文案表
pub fn texts(lang: Lang) -> &'static ShellTexts {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
    }
}

/// 按当前设置取文案表(托盘/通知发送时实时调用, 语言切换即时生效)
pub fn current(app: &tauri::AppHandle) -> &'static ShellTexts {
    let lang = Lang::from_settings(&lock(&app.state::<AppState>().settings).language);
    texts(lang)
}
