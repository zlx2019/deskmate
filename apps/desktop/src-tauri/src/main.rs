// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Desktop entry point. The library owns the logic so mobile can reuse it.
fn main() {
    deskmate_desktop_lib::run()
}
