//! IPC commands callable from the frontend via `invoke`. Errors are returned as strings, which is what the
//! frontend receives as the rejected promise value.

use serde::Serialize;
use tauri::{AppHandle, Runtime};

/// Which Rust profile this binary was compiled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildProfile {
    /// `debug_assertions` on (`cargo tauri dev`, `cargo build`).
    Debug,
    /// `debug_assertions` off (`cargo tauri build`).
    Release,
}

impl BuildProfile {
    /// The profile of the running binary.
    pub const fn current() -> Self {
        if cfg!(debug_assertions) {
            Self::Debug
        } else {
            Self::Release
        }
    }
}

/// Basic facts about the running app, shown on the Settings page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    /// Product name from `tauri.conf.json`.
    pub name: String,
    /// App version from `tauri.conf.json`.
    pub version: String,
    /// Build profile of this binary.
    pub profile: BuildProfile,
}

/// Returns name, version and build profile of the running app.
#[tauri::command]
pub fn app_info<R: Runtime>(app: AppHandle<R>) -> AppInfo {
    let package = app.package_info();
    AppInfo {
        name: package.name.clone(),
        version: package.version.to_string(),
        profile: BuildProfile::current(),
    }
}

/// Shows, unminimizes and focuses the main window.
#[tauri::command]
pub fn show_window<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    crate::show_main_window(&app).map_err(|err| err.to_string())
}

/// Hides the main window; the app keeps running in the tray.
#[tauri::command]
pub fn hide_to_tray<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    crate::hide_main_window(&app).map_err(|err| err.to_string())
}

/// Shuts the app down (same path as Quit in the tray menu).
#[tauri::command]
pub fn quit<R: Runtime>(app: AppHandle<R>) {
    crate::shutdown(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_matches_compilation() {
        let expected = if cfg!(debug_assertions) {
            BuildProfile::Debug
        } else {
            BuildProfile::Release
        };
        assert_eq!(BuildProfile::current(), expected);
    }

    #[test]
    fn app_info_serializes_to_frontend_shape() {
        let info = AppInfo {
            name: "CaseControl".into(),
            version: "0.1.0".into(),
            profile: BuildProfile::Release,
        };
        let json = serde_json::to_value(&info).expect("AppInfo serializes");
        assert_eq!(
            json,
            serde_json::json!({ "name": "CaseControl", "version": "0.1.0", "profile": "release" })
        );
        let debug = serde_json::to_value(BuildProfile::Debug).expect("BuildProfile serializes");
        assert_eq!(debug, serde_json::json!("debug"));
    }
}
