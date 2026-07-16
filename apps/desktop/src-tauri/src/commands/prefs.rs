//! 设置族命令: 设置读写(含各项即时生效的热应用)、头像图片与全局快捷键

use tauri::State;

use crate::settings::Settings;
use crate::state::{AppState, lock};

/// 读取当前设置
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    lock(&state.settings).clone()
}

/// 保存设置; 除监听端口(socket 启动时固定, 重启后生效)外均即时生效
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
    // 隐身模式热切换(开: goodbye + 注销 mDNS; 关: 重新广播并立即 announce;
    // 值未变时为空操作)
    state.discovery.set_passive(settings.passive);
    // 语言变化即时生效: 通知文案发送时实时取, 托盘菜单需重建
    let language_changed = lock(&state.settings).language != settings.language;
    *lock(&state.settings) = settings;
    if language_changed {
        crate::refresh_tray_menu(&app);
    }
    Ok(())
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
