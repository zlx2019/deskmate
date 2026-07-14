//! deskmate 桌面端应用壳: 注册插件、启动核心引擎、系统托盘、暴露 commands 给前端

mod bridge;
mod commands;
mod history;
mod settings;
mod state;

use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// 应用入口: 初始化日志与引擎, 挂载 Tauri 运行时
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    let app = tauri::Builder::default()
        // 单实例锁必须最先注册: 二次启动的进程直接退出, 由首实例唤起窗口
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // macOS 用 LaunchAgent; 自启时带 --hidden 直接隐入托盘
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .setup(|app| {
            // 数据目录: ~/Library/Application Support/<identifier> (macOS, 见 Tauri.toml)
            let data_dir = app.path().app_data_dir()?;
            // 引擎启动很快(bind + mDNS 注册), 同步等待以保证 commands 可用时 state 已就绪
            let state = tauri::async_runtime::block_on(bridge::start_engine(
                app.handle().clone(),
                data_dir,
            ))?;
            app.manage(state);
            setup_tray(app.handle())?;
            apply_window_effects(app.handle());
            // 开机自启拉起的实例不亮窗口, 常驻托盘等待收发
            if std::env::args().any(|a| a == "--hidden")
                && let Some(window) = app.get_webview_window("main")
            {
                let _ = window.hide();
            }
            Ok(())
        })
        // 关窗口 = 隐入托盘继续收发(P0: 关窗口 ≠ 退出); 真正退出走托盘菜单
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            // 获得焦点即视为"已读", 清空 Dock/任务栏未读角标
            tauri::WindowEvent::Focused(true) => {
                bridge::clear_unread(window.app_handle());
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_self_info,
            commands::list_peers,
            commands::send_files_to,
            commands::send_text_to,
            commands::respond_offer,
            commands::precheck_receive,
            commands::pause_transfer,
            commands::resume_transfer,
            commands::cancel_transfer,
            commands::resume_send_transfer,
            commands::retry_send_transfer,
            commands::get_settings,
            commands::save_settings,
            commands::set_avatar_image,
            commands::get_avatar_image,
            commands::get_history,
            commands::append_history,
            commands::window_effects_active,
        ])
        .build(tauri::generate_context!())
        .expect("tauri 应用构建失败");

    app.run(|app_handle, event| {
        match event {
            // macOS 点击 Dock 图标: 重新亮出隐藏的主窗口
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_main_window(app_handle),
            // 退出前发 goodbye 并注销 mDNS, 让对端雷达即时移除本节点
            tauri::RunEvent::Exit => {
                let state = app_handle.state::<state::AppState>();
                tauri::async_runtime::block_on(state.discovery.shutdown());
            }
            _ => {}
        }
    });
}

/// 系统窗口材质是否已生效(前端据此切换半透明背景变量)
pub(crate) static WINDOW_EFFECTS_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 应用系统级窗口材质: macOS vibrancy / Windows mica(Win11)
///
/// 失败(旧系统 / Linux)静默回退 —— 窗口虽为 transparent,
/// 但前端默认画满不透明背景, 只有本函数成功后才切半透明变量。
fn apply_window_effects(app: &tauri::AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        #[cfg(target_os = "macos")]
        let applied = window_vibrancy::apply_vibrancy(
            &window,
            window_vibrancy::NSVisualEffectMaterial::UnderWindowBackground,
            None,
            None,
        );
        #[cfg(target_os = "windows")]
        let applied = window_vibrancy::apply_mica(&window, None);
        match applied {
            Ok(()) => {
                WINDOW_EFFECTS_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) => tracing::debug!("窗口材质不可用(回退纯色背景): {e}"),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = app;
}

/// 亮出并聚焦主窗口(托盘点击、二次启动、Dock 点击共用), 同时清未读角标
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    bridge::clear_unread(app);
}

/// 创建系统托盘: 左键单击亮窗口, 菜单提供显示/退出
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示 deskmate", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        // 各平台统一为: 左键亮窗口, 右键(macOS 含 Ctrl+左键)弹菜单
        .show_menu_on_left_click(false)
        .tooltip("deskmate")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    // macOS 菜单栏用单色模板图标(系统按明暗自动着色); 解码失败回退彩色应用图标
    #[cfg(target_os = "macos")]
    {
        match tauri::image::Image::from_bytes(include_bytes!("../icons/tray-iconTemplate.png")) {
            Ok(template) => tray = tray.icon(template).icon_as_template(true),
            Err(e) => {
                tracing::warn!("托盘模板图标解码失败({e}), 回退应用图标");
                if let Some(icon) = app.default_window_icon() {
                    tray = tray.icon(icon.clone());
                }
            }
        }
    }
    // Windows/Linux 托盘直接用彩色应用图标
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// 初始化 tracing 日志: stderr 输出, 级别由 RUST_LOG 控制(默认 info)
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
