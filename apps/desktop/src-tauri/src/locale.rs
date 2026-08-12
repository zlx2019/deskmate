//! Bilingual desktop-shell text for tray menus and system notifications.
//!
//! Frontend text lives separately in apps/desktop/src/i18n. The language comes
//! from settings after first-run system detection. Before initialization it
//! falls back to environment variables, which macOS GUI processes may omit.

use tauri::Manager;

use crate::state::{AppState, lock};

/// Supported interface languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Chinese.
    Zh,
    /// English.
    En,
}

impl Lang {
    /// Parses a settings value and falls back to environment variables when unknown.
    pub fn from_settings(value: &str) -> Self {
        match value {
            "zh" => Lang::Zh,
            "en" => Lang::En,
            _ => Self::system_fallback(),
        }
    }

    /// Environment fallback used only before settings initialization.
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

/// A desktop-shell string awaiting localization; parameters ride on the variant.
///
/// Adding text means adding a variant and one `localize` arm — the exhaustive
/// match keeps both languages complete at compile time, side by side.
pub enum Text<'a> {
    /// Tray menu: show the main window.
    TrayShow,
    /// Tray menu: open settings.
    TraySettings,
    /// Tray menu: quit.
    TrayQuit,
    /// Notification: transfer completed.
    TransferCompleted,
    /// Notification: transfer cancelled.
    TransferCancelled,
    /// Notification: transfer interrupted unexpectedly.
    TransferInterrupted,
    /// Notification: transfer failed before or during handshake.
    TransferFailed,
    /// Notification: peer rejected the transfer.
    TransferRejected,
    /// Notification: send manifest was empty.
    NothingToSend,
    /// Notification: copy-and-send found no copied content.
    CopySendNothing,
    /// Notification: copy-and-send requires macOS accessibility permission.
    CopySendPermission,
    /// Notification: simulated copy is unsupported on this platform.
    CopySendUnsupported,
    /// Text-received notification title, marking automatic clipboard copies.
    IncomingMessage {
        /// Sender display name.
        name: &'a str,
        /// Whether the text was auto-copied to the clipboard.
        copied: bool,
    },
    /// Notification: a peer requests to send files.
    OfferIncoming {
        /// Sender display name.
        name: &'a str,
        /// File count.
        n: usize,
        /// Human-readable total size.
        size: &'a str,
    },
    /// Notification: auto-accepting files from a trusted peer.
    AutoReceiving {
        /// Sender display name.
        name: &'a str,
        /// File count.
        n: usize,
        /// Human-readable total size.
        size: &'a str,
    },
}

impl Text<'_> {
    /// Renders the text in the given language.
    pub fn localize(self, lang: Lang) -> String {
        use Lang::{En, Zh};
        match self {
            Text::TrayShow => match lang {
                Zh => "显示",
                En => "Show",
            }
            .into(),
            Text::TraySettings => match lang {
                Zh => "设置",
                En => "Settings",
            }
            .into(),
            Text::TrayQuit => match lang {
                Zh => "退出",
                En => "Quit",
            }
            .into(),
            Text::TransferCompleted => match lang {
                Zh => "文件传输完成",
                En => "Transfer completed",
            }
            .into(),
            Text::TransferCancelled => match lang {
                Zh => "传输已取消",
                En => "Transfer cancelled",
            }
            .into(),
            Text::TransferInterrupted => match lang {
                Zh => "传输意外中断, 未完成部分已保留",
                En => "Transfer interrupted, partial data kept for resume",
            }
            .into(),
            Text::TransferFailed => match lang {
                Zh => "传输失败, 详情见应用内传输记录",
                En => "Transfer failed — see the in-app transfer list for details",
            }
            .into(),
            Text::TransferRejected => match lang {
                Zh => "对方拒绝了本次传输",
                En => "Peer declined the transfer",
            }
            .into(),
            Text::NothingToSend => match lang {
                Zh => "没有可发送的文件(可能已被忽略规则过滤)",
                En => "Nothing to send (files may be filtered by ignore rules)",
            }
            .into(),
            Text::CopySendNothing => match lang {
                Zh => "未检测到可复制的选中内容, 已取消发送",
                En => "Nothing was copied — sending cancelled",
            }
            .into(),
            Text::CopySendPermission => match lang {
                Zh => "需要辅助功能权限: 请在 系统设置 → 隐私与安全性 → 辅助功能 中勾选 Deskmate",
                En => "Accessibility permission required: enable Deskmate in System Settings → Privacy & Security → Accessibility",
            }
            .into(),
            Text::CopySendUnsupported => match lang {
                Zh => "当前平台不支持模拟复制, 请先手动复制再用发送剪贴板快捷键",
                En => "Simulated copy is not supported on this platform; copy manually and use the send-clipboard hotkey",
            }
            .into(),
            Text::IncomingMessage { name, copied } => {
                let mut title = match lang {
                    Zh => format!("{name} 发来文本"),
                    En => format!("Text from {name}"),
                };
                if copied {
                    title.push_str(match lang {
                        Zh => " · 已复制",
                        En => " · copied",
                    });
                }
                title
            }
            Text::OfferIncoming { name, n, size } => match lang {
                Zh => format!("{name} 请求发送 {n} 个文件({size})"),
                En => format!("{name} wants to send {} ({size})", files_en(n)),
            },
            Text::AutoReceiving { name, n, size } => match lang {
                Zh => format!("正在自动接收 {name} 发来的 {n} 个文件({size})"),
                En => format!("Automatically receiving {} from {name} ({size})", files_en(n)),
            },
        }
    }
}

/// English file count with pluralization.
fn files_en(n: usize) -> String {
    if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    }
}

/// Resolves the current language from settings.
pub fn lang(app: &tauri::AppHandle) -> Lang {
    Lang::from_settings(&lock(&app.state::<AppState>().settings).language)
}

/// Localizes text in the current settings language.
pub fn text(app: &tauri::AppHandle, t: Text<'_>) -> String {
    t.localize(lang(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// English bodies pluralize the file count; Chinese uses one form.
    #[test]
    fn notification_templates_fill_placeholders() {
        assert_eq!(
            Text::OfferIncoming {
                name: "Mac",
                n: 1,
                size: "2.0 MB"
            }
            .localize(Lang::En),
            "Mac wants to send 1 file (2.0 MB)"
        );
        assert_eq!(
            Text::AutoReceiving {
                name: "Mac",
                n: 3,
                size: "9.5 MB"
            }
            .localize(Lang::En),
            "Automatically receiving 3 files from Mac (9.5 MB)"
        );
        assert_eq!(
            Text::OfferIncoming {
                name: "Mac",
                n: 3,
                size: "9.5 MB"
            }
            .localize(Lang::Zh),
            "Mac 请求发送 3 个文件(9.5 MB)"
        );
        assert_eq!(
            Text::IncomingMessage {
                name: "Mac",
                copied: true
            }
            .localize(Lang::Zh),
            "Mac 发来文本 · 已复制"
        );
    }
}
