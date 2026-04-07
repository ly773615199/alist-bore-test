#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod config;
mod service;

use std::sync::Mutex;

use tauri::{
    WindowEvent,
};

use service::AppState;

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(AppState::new()))
        .on_window_event(|event| {
            if let WindowEvent::CloseRequested { api, .. } = event.event() {
                event.window().hide().ok();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler!(
            service::cmd_start_alist,
            service::cmd_start_bore,
            service::cmd_stop_services,
            service::cmd_get_password,
            service::cmd_open_url,
            service::cmd_quit_app,
        ))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}