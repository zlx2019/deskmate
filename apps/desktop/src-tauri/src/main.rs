// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 桌面端入口: 逻辑全部在 lib 中, 便于移动端复用同一入口
fn main() {
    deskmate_desktop_lib::run()
}
