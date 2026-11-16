//! CaseControl desktop shell (Tauri 2).
//!
//! One process (ADR 0001 #3): a window with the Devices, Profiles, Conflicts and Settings pages, a tray icon
//! with Show / Apply profile / Release devices / Quit, close-to-tray, a single-instance guard, autostart
//! registration, file logging, and the IPC commands and events the pages use.
//!
//! On start it loads settings and profiles (`cc-config`), starts the device engine (`cc-engine`) with every
//! compiled-in driver (`cc-drivers`) and re-applies the last applied profile. Every exit path (tray Quit, the
//! `quit` command, `RunEvent::ExitRequested`/`Exit`, which also covers Windows session end) releases the devices
//! through [`AppState::shutdown_engine`] exactly once.
//!
//! It deliberately has no network access and no telemetry (ADR 0001 #13), never changes other software's autostart
//! (ADR 0001 #12), and enables its own autostart only when the user switches it on in Settings.

pub mod commands;
pub mod conflicts;
pub mod events;
pub mod logging;
pub mod prefs;
pub mod profiles;
pub mod state;
pub mod tray;

use std::ffi::OsStr;

use tauri::{AppHandle, Manager, RunEvent, Runtime, WindowEvent};

pub use state::AppState;

/// Label of the main window as declared in `tauri.conf.json`.
pub const MAIN_WINDOW: &str = "main";

/// Command-line flag that starts the app with the window hidden (used by autostart).
pub const HIDDEN_FLAG: &str = "--hidden";

/// Whether the given command line (including the program name) asks for a hidden start.
///
/// Takes OS strings so a non-Unicode argument is simply not the flag instead of a startup panic.
pub fn starts_hidden<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == OsStr::new(HIDDEN_FLAG))
}

/// Shows, unminimizes and focuses the main window.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        tracing::warn!("main window not found");
        return Ok(());
    };
    window.show()?;
    window.unminimize()?;
    window.set_focus()
}

/// Hides the main window; the process keeps running in the tray.
pub fn hide_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) => {
            tracing::info!("hiding main window to tray");
            window.hide()
        }
        None => Ok(()),
    }
}

/// Orderly shutdown and process exit: releases every device to its hardware mode (once), then exits. Tray Quit and
/// the `quit` command call this; the run-loop exit events call [`AppState::shutdown_engine`] directly.
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    tracing::info!("shutting down");
    if let Some(state) = app.try_state::<AppState>() {
        state.shutdown_engine();
    }
    app.exit(0);
}

/// Builds and runs the Tauri application. Returns only on startup failure; a normal exit ends the process.
pub fn run() {
    logging::init();
    let hidden = starts_hidden(std::env::args_os());
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        hidden,
        "CaseControl starting"
    );

    let app = tauri::Builder::default()
        // Must be the first plugin so a second instance exits before anything else initializes.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            tracing::info!(?args, "second instance started; focusing the main window");
            if let Err(err) = show_main_window(app) {
                tracing::error!(%err, "cannot show main window");
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::show_window,
            commands::hide_to_tray,
            commands::quit,
            commands::list_devices,
            commands::set_direct,
            commands::set_zone,
            commands::set_mode,
            commands::resize_zone,
            commands::save_to_hardware,
            commands::release_device,
            commands::release_all,
            commands::rescan,
            commands::list_profiles,
            commands::save_profile,
            commands::apply_profile,
            commands::rename_profile,
            commands::delete_profile,
            commands::scan_conflicts,
            commands::terminate_process,
            commands::stop_service,
            commands::get_settings,
            commands::set_settings,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN_WINDOW
            {
                api.prevent_close();
                tracing::info!("close requested; hiding to tray");
                if let Err(err) = window.hide() {
                    tracing::error!(%err, "cannot hide main window");
                }
            }
        })
        .setup(move |app| {
            let state = AppState::init();
            let start_minimized = state.settings().start_minimized;
            if let Ok(engine) = state.engine() {
                events::spawn_forwarder(app.handle().clone(), engine.subscribe());
            }
            state.apply_last_profile();
            let profiles = state.profile_list().unwrap_or_else(|err| {
                tracing::warn!(%err, "cannot list profiles for the tray");
                profiles::ProfileList {
                    names: Vec::new(),
                    active: None,
                }
            });
            app.manage(state);
            tray::create(app.handle(), &profiles)?;
            // The window is declared with `visible: false` so a hidden start never flashes it.
            if !hidden && !start_minimized {
                show_main_window(app.handle())?;
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(err) => {
            tracing::error!(%err, "CaseControl failed to start");
            panic!("error while running CaseControl: {err}");
        }
    };
    app.run(|app, event| {
        // `Exit` also arrives on Windows session end (WM_ENDSESSION), where the process ends right after.
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit)
            && let Some(state) = app.try_state::<AppState>()
        {
            state.shutdown_engine();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_flag_is_detected_after_program_name() {
        assert!(starts_hidden(["casecontrol.exe", "--hidden"]));
        assert!(starts_hidden(["casecontrol.exe", "--other", "--hidden"]));
        assert!(!starts_hidden(["casecontrol.exe"]));
        assert!(!starts_hidden(["--hidden"]));
        assert!(!starts_hidden(["casecontrol.exe", "--hidden=false"]));
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_argument_does_not_panic() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let lone_surrogate = OsString::from_wide(&[0xD800]);
        assert!(!starts_hidden([
            OsString::from("casecontrol.exe"),
            lone_surrogate.clone()
        ]));
        assert!(starts_hidden([
            OsString::from("casecontrol.exe"),
            lone_surrogate,
            OsString::from("--hidden"),
        ]));
    }
}
