//! The ternimal app: the window, and the commands the UI calls in Rust.
//!
//! The real work lives in `ternimal-core`. This file only connects it to the UI.

// No console window next to the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod sessions;

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri::{Manager, State};
use ternimal_core::settings::Settings;
use ternimal_core::workspace::Workspace;

/// What the status bar shows about the app and the computer it runs on.
#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    os: &'static str,
    arch: &'static str,
}

/// A value kept in memory and in a file in the config folder.
struct Saved<T> {
    path: PathBuf,
    current: Mutex<T>,
}

impl<T: Clone> Saved<T> {
    fn get(&self) -> T {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, value: T) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = value;
    }

    fn save_problem(&self, error: std::io::Error) -> String {
        format!("could not save {}: {error}", self.path.display())
    }
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
fn get_settings(file: State<Saved<Settings>>) -> Settings {
    file.get()
}

/// Saves new settings at once, and returns them as kept, with every value in range.
#[tauri::command]
fn set_settings(settings: Settings, file: State<Saved<Settings>>) -> Result<Settings, String> {
    let settings = settings.checked();
    settings
        .save(&file.path)
        .map_err(|error| file.save_problem(error))?;
    file.set(settings.clone());
    Ok(settings)
}

#[tauri::command]
fn get_workspace(file: State<Saved<Workspace>>) -> Workspace {
    file.get()
}

/// Saves the open tabs. Not async, so saves happen in the order they were asked for.
#[tauri::command]
fn set_workspace(workspace: Workspace, file: State<Saved<Workspace>>) -> Result<(), String> {
    let workspace = workspace.checked();
    workspace
        .save(&file.path)
        .map_err(|error| file.save_problem(error))?;
    file.set(workspace);
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let settings = dir.join("settings.toml");
            let workspace = dir.join("workspace.toml");
            app.manage(Saved {
                current: Mutex::new(Settings::load(&settings)),
                path: settings,
            });
            app.manage(Saved {
                current: Mutex::new(Workspace::load(&workspace)),
                path: workspace,
            });
            app.manage(sessions::Sessions::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            welcome_banner,
            get_settings,
            set_settings,
            get_workspace,
            set_workspace,
            sessions::list_ports,
            sessions::list_probes,
            sessions::search_chips,
            sessions::open_session,
            sessions::open_rtt_session,
            sessions::open_swo_session,
            sessions::close_session,
            sessions::close_all_sessions,
            sessions::write_session,
            sessions::set_session_lines,
            sessions::break_session,
            sessions::reconfigure_session,
            sessions::reconfigure_rtt_session,
            sessions::reconfigure_swo_session,
            sessions::reset_target,
        ])
        .run(tauri::generate_context!())
        .expect("ternimal could not start");
}
