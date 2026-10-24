//! The ternimal app: the window, and the commands the UI calls in Rust.
//!
//! The real work lives in `ternimal-core`. This file only connects it to the UI.

// No console window next to the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri::{Manager, State};
use ternimal_core::settings::Settings;

/// What the status bar shows about the app and the computer it runs on.
#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    os: &'static str,
    arch: &'static str,
}

/// The settings in use, and the file they are saved to.
struct SettingsFile {
    path: PathBuf,
    current: Mutex<Settings>,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: ternimal_core::VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

#[tauri::command]
fn welcome_banner(background: ternimal_core::Background) -> String {
    ternimal_core::welcome_banner(background)
}

#[tauri::command]
fn get_settings(file: State<SettingsFile>) -> Settings {
    file.current
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Saves new settings at once, and returns them as kept, with every value in range.
#[tauri::command]
fn set_settings(settings: Settings, file: State<SettingsFile>) -> Result<Settings, String> {
    let settings = settings.checked();
    settings
        .save(&file.path)
        .map_err(|error| format!("could not save {}: {error}", file.path.display()))?;
    *file.current.lock().unwrap_or_else(PoisonError::into_inner) = settings.clone();
    Ok(settings)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let path = app.path().app_config_dir()?.join("settings.toml");
            let current = Mutex::new(Settings::load(&path));
            app.manage(SettingsFile { path, current });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            welcome_banner,
            get_settings,
            set_settings
        ])
        .run(tauri::generate_context!())
        .expect("ternimal could not start");
}
