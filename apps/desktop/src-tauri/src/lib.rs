//! CaseControl desktop shell (Tauri 2).
//!
//! One process (ADR 0001 #3): a window with the Lighting, Conflicts and Settings pages, a tray icon
//! with Show / Apply profile / Release devices / Quit, close-to-tray, a single-instance guard, autostart
//! registration, file logging, and the IPC commands and events the pages use. The window has no Windows title bar
//! (ADR 0001 #29); the frontend draws its own window controls. Devices and zones the user hid are kept out of
//! control by the commands (see [`hidden`], ADR 0001 #26).
//!
//! On start it loads settings and profiles (`cc-config`), starts the device engine (`cc-engine`) with every
//! compiled-in driver (`cc-drivers`) and applies the active profile; there is always exactly one, created as "Profile
//! 1" when none exists, and every change to what a device shows is saved into it by a coalescing autosave thread
//! ([`autosave`], ADR 0001 #40). Every exit path (tray Quit, the
//! `quit` command, a second instance started with `--quit`, `RunEvent::ExitRequested`/`Exit` (no update install there), which also covers
//! Windows session end) writes a pending profile change and releases the devices through
//! [`AppState::shutdown_engine`] exactly once. A `--quit`
//! process itself never starts the engine (see [`run`]).
//!
//! After the start (also a hidden one) a background thread scans for conflicting software; if RGB suites affect a
//! visible device, the window is shown and `startup-conflicts` carries the start dialog model (ADR 0001 #31, see
//! [`startup`]). Closing them or turning their autostart off happens only on the user's click
//! ([`conflict_actions`]); what needs administrator rights runs in the [`elevated`] helper mode of this same
//! executable, which `main` enters before anything here starts.
//!
//! Release builds update themselves from GitHub releases ([`updater`], ADR 0001 #39, #41): a check after the start
//! and every 6 hours (or "Check for updates" in Settings) downloads a signed update in the background; it is installed
//! on the next quit, after the devices were released, on the next start, before any device is opened, or at once
//! with "Update now" ([`install_update_now`]), which releases the devices first and restarts the app.
//!
//! The updater is its only network access (ADR 0001 #13, #39); it has no telemetry, changes other software's
//! autostart only reversibly and only on a click (ADR 0001 #31), and enables its own autostart only when the user
//! switches it on in Settings.

pub mod autosave;
pub mod autostart;
pub mod commands;
pub mod conflict_actions;
pub mod conflicts;
pub mod elevated;
pub mod events;
pub mod hidden;
pub mod logging;
pub mod prefs;
pub mod profiles;
pub mod startup;
pub mod state;
pub mod tray;
pub mod updater;

use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, RunEvent, Runtime, WindowEvent};

pub use state::AppState;

/// Label of the main window as declared in `tauri.conf.json`.
pub const MAIN_WINDOW: &str = "main";

/// Command-line flag that starts the app with the window hidden (used by autostart).
pub const HIDDEN_FLAG: &str = "--hidden";

/// Command-line flag that asks a running instance to shut down (releasing every device) and starts nothing itself.
pub const QUIT_FLAG: &str = "--quit";

/// What a command line asks the desktop app to do. `--elevated-actions` is not parsed here: `main` handles it
/// before anything else (see [`elevated::helper_main`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    /// `--hidden`: start with the window hidden.
    pub hidden: bool,
    /// `--quit`: make the running instance shut down; never start one.
    pub quit: bool,
}

impl LaunchArgs {
    /// Parses a command line including the program name, which is skipped. Flags match exactly; unknown
    /// arguments are ignored. Takes OS strings so a non-Unicode argument is simply not a flag instead of a panic.
    pub fn parse<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut parsed = Self::default();
        for arg in args.into_iter().skip(1) {
            let arg = arg.as_ref();
            if arg == OsStr::new(HIDDEN_FLAG) {
                parsed.hidden = true;
            } else if arg == OsStr::new(QUIT_FLAG) {
                parsed.quit = true;
            }
        }
        parsed
    }
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

/// Set by the first exit path that runs (tray Quit, the `quit` command, `--quit`, Update now), so exit paths never
/// overlap: a second one is ignored instead of exiting while the first is releasing devices or starting the
/// installer.
static EXITING: AtomicBool = AtomicBool::new(false);

/// Claims the exit for the caller; `false` when another exit path already runs.
fn begin_exit() -> bool {
    !EXITING.swap(true, Ordering::AcqRel)
}

/// Gives the exit back after an Update now that changed nothing (no release happened), so the app keeps running
/// normally.
fn cancel_exit() {
    EXITING.store(false, Ordering::Release);
}

/// Orderly shutdown and process exit: releases every device to its hardware mode (once), installs a pending update
/// (see [`install_update_on_exit`]), then exits. Tray Quit, the `quit` command and `--quit` call this on the main
/// thread; the run-loop exit events call [`AppState::shutdown_engine`] directly. Ignored while another exit path
/// (e.g. Update now) runs.
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    if !begin_exit() {
        tracing::info!("shutdown requested while CaseControl is already exiting or updating; ignored");
        return;
    }
    tracing::info!("shutting down");
    if let Some(state) = app.try_state::<AppState>() {
        state.shutdown_engine();
    }
    install_update_on_exit(app);
    app.exit(0);
}

/// Starts the installer of a pending update without restarting the app; call only after the devices were released.
/// On success the process ends here (after Tauri's exit cleanup, so the tray icon is removed); otherwise it returns
/// and the caller exits normally. A no-op in debug builds and when nothing is pending.
pub fn install_update_on_exit<R: Runtime>(app: &AppHandle<R>) {
    if let Some(updater) = app.try_state::<updater::AppUpdater<R>>() {
        updater.install_pending(&updater::Restart::No, || app.cleanup_before_exit());
    }
}

/// Shown when an update action is asked for in a build without updates (also the `disabled` status's reason).
pub const UPDATES_DISABLED: &str = updater::DISABLED_REASON;

/// The arguments CaseControl restarts with after Update now: the current command line without the program name,
/// `--hidden` and `--quit`. The user clicked Update now in the window, so the new version must show it even when
/// this instance was started hidden by autostart.
pub fn update_now_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .skip(1)
        .filter(|a| {
            let a = a.as_ref();
            a != OsStr::new(HIDDEN_FLAG) && a != OsStr::new(QUIT_FLAG)
        })
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect()
}

/// "Update now" (ADR 0001 #41). Runs on the main thread, like the other exit paths ([`shutdown`]): once the pending
/// update verified, writes a pending profile change and releases every device ([`AppState::shutdown_engine`]), runs
/// Tauri's exit cleanup and starts the quiet installer, which starts the new version afterwards with
/// [`update_now_args`]; on success the process ends here. Refused while another exit path runs.
///
/// Errors when nothing was installed and the app keeps running (no update pending, a check running, the update stale
/// or invalid). If the installer cannot be started after the devices were released, CaseControl starts itself again
/// with [`update_now_args`] and exits, so it comes back with its devices (without the update, which is downloaded
/// again later).
pub fn install_update_now<R: Runtime>(
    app: &AppHandle<R>,
    updater: &updater::AppUpdater<R>,
) -> Result<updater::UpdateView, String> {
    use updater::InstallOutcome;
    tracing::info!("update now requested");
    if !updater.enabled() {
        return Err(UPDATES_DISABLED.to_owned());
    }
    if !begin_exit() {
        tracing::info!("update now: CaseControl is already exiting; ignored");
        return Err("CaseControl is closing.".to_owned());
    }
    let args = update_now_args(std::env::args_os());
    let outcome = updater.install_now(updater::Restart::WithArgs(args.clone()), || {
        tracing::info!("update now: releasing devices before the installer starts");
        if let Some(state) = app.try_state::<AppState>() {
            state.shutdown_engine();
        }
        app.cleanup_before_exit();
    });
    let result = match outcome {
        InstallOutcome::Installing(version) => {
            // Only seen with test backends: on Windows the plugin ended the process.
            tracing::info!(%version, "update now: installer started");
            return Ok(updater.view());
        }
        InstallOutcome::Failed(err) => {
            tracing::error!(%err, ?args, "update now: the installer did not start; starting CaseControl again");
            restart_with(app, &args);
            return Err(format!("The update could not be installed: {err}"));
        }
        InstallOutcome::Disabled => Err(UPDATES_DISABLED.to_owned()),
        InstallOutcome::Busy => Err("An update check is running. Try again when it has finished.".to_owned()),
        InstallOutcome::NothingPending => Err("No update is ready to install.".to_owned()),
        InstallOutcome::Stale => {
            Err("The downloaded update is not newer than this version; it was removed.".to_owned())
        }
        InstallOutcome::Discarded(err) => Err(format!(
            "The downloaded update could not be verified and was removed: {err}"
        )),
    };
    // Nothing was released: the app keeps running.
    cancel_exit();
    result
}

/// Starts a new CaseControl process with `args` and exits this one. Called only after Tauri's exit cleanup, which
/// released the single-instance lock, so the new process does not hand its arguments to this one.
fn restart_with<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(err) = std::process::Command::new(exe).args(args).spawn() {
                tracing::error!(%err, "cannot start CaseControl again");
            }
        }
        Err(err) => tracing::error!(%err, "cannot find the CaseControl executable to start it again"),
    }
    app.exit(0);
}

/// Starts the thread that writes the active profile after changes (ADR 0001 #40); it ends at shutdown.
fn spawn_autosave<R: Runtime>(app: AppHandle<R>) {
    let spawned = std::thread::Builder::new()
        .name("cc-autosave".into())
        .spawn(move || {
            if let Some(state) = app.try_state::<AppState>() {
                state.run_autosave();
            }
        });
    if let Err(err) = spawned {
        tracing::error!(%err, "cannot start the autosave thread; profile changes are saved only on exit and switch");
    }
}

/// Scans for conflicts on a background thread after the start (device control is already running). If the start
/// dialog has programs to ask about, shows the window (also after a hidden start) and emits `startup-conflicts`.
fn spawn_startup_check<R: Runtime>(app: AppHandle<R>) {
    let spawned = std::thread::Builder::new()
        .name("cc-startup-conflicts".into())
        .spawn(move || {
            elevated::sweep_stale_files();
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let view = match conflict_actions::scan_and_publish(&app, &state) {
                Ok(view) => view,
                Err(err) => {
                    tracing::warn!(%err, "start-up conflict check failed");
                    return;
                }
            };
            if view.startup.is_empty() {
                tracing::info!("start-up conflict check: nothing to ask");
                return;
            }
            tracing::info!(
                programs = ?view.startup.iter().map(|p| p.entry_id.as_str()).collect::<Vec<_>>(),
                "start-up conflict check: asking the user"
            );
            if let Err(err) = show_main_window(&app) {
                tracing::error!(%err, "cannot show main window for the start-up conflicts");
            }
            events::emit(&app, events::STARTUP_CONFLICTS, view.startup);
        });
    if let Err(err) = spawned {
        tracing::error!(%err, "cannot start the start-up conflict check");
    }
}

/// Builds and runs the Tauri application. Returns only when started with `--quit` and no instance was running; a
/// normal exit ends the process.
///
/// `--quit`: the single-instance plugin decides inside `Builder::build`, before the window, the tray, the engine or
/// device detection exist (those start in the setup hook, which runs only in `App::run`). If an instance is running,
/// this process forwards its arguments there and exits inside `build`; the running instance's callback calls
/// [`shutdown`] before the forward returns. If none is running, `build` returns here and the process leaves without
/// running the app.
pub fn run() {
    logging::init();
    let launch = LaunchArgs::parse(std::env::args_os());
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        hidden = launch.hidden,
        quit = launch.quit,
        "CaseControl starting"
    );

    let app = tauri::Builder::default()
        // Must be the first plugin so a second instance exits before anything else initializes.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if LaunchArgs::parse(&args).quit {
                tracing::info!(?args, "second instance asked to quit");
                shutdown(app);
                return;
            }
            tracing::info!(?args, "second instance started; focusing the main window");
            if let Err(err) = show_main_window(app) {
                tracing::error!(%err, "cannot show main window");
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            commands::set_device_brightness,
            commands::set_master_brightness,
            commands::connect_all,
            commands::rescan,
            commands::set_device_hidden,
            commands::set_zone_hidden,
            commands::list_profiles,
            commands::apply_profile,
            commands::create_profile,
            commands::duplicate_profile,
            commands::rename_profile,
            commands::delete_profile,
            commands::scan_conflicts,
            commands::startup_conflicts,
            commands::resolve_conflicts,
            commands::set_autostart,
            commands::set_dont_ask,
            commands::close_program,
            commands::get_settings,
            commands::set_settings,
            commands::set_default_color,
            commands::update_status,
            commands::check_for_updates,
            commands::install_update,
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
            // A pending update is installed before the engine opens any device; the installer restarts the app with
            // the same arguments and this process ends here. Otherwise (nothing pending, debug build) start normally.
            let status_app = app.handle().clone();
            let updater = updater::create(app.handle(), move |view| {
                events::emit(&status_app, events::UPDATE_STATUS, view.clone());
            });
            updater.install_pending(&updater::Restart::SameArgs, || {});
            app.manage(updater.clone());
            let state = AppState::init();
            let start_minimized = state.settings().start_minimized;
            autostart::sync_run_entry(app.handle(), state.settings().autostart);
            if let Ok(engine) = state.engine() {
                events::spawn_forwarder(app.handle().clone(), engine.subscribe());
            }
            state.start_profiles();
            let profiles = state.profile_list().unwrap_or_else(|err| {
                tracing::warn!(%err, "cannot list profiles for the tray");
                profiles::ProfileList {
                    names: Vec::new(),
                    active: None,
                }
            });
            app.manage(state);
            spawn_autosave(app.handle().clone());
            tray::create(app.handle(), &profiles)?;
            // The window is declared with `visible: false` so a hidden start never flashes it.
            if !launch.hidden && !start_minimized {
                show_main_window(app.handle())?;
            }
            spawn_startup_check(app.handle().clone());
            // After the engine is up: check now and every 6 hours on a background thread.
            updater::spawn_checks(updater);
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
    if launch.quit {
        // A running instance would have made this process exit inside `build`, so none is running. The setup hook
        // (engine, tray, window) never runs; release the single-instance lock and leave.
        tracing::info!("--quit: no running instance; exiting");
        app.handle().cleanup_before_exit();
        return;
    }
    app.run(|app, event| {
        // `Exit` also arrives on Windows session end (WM_ENDSESSION), where the process ends right after.
        // No update install here: `Exit` also arrives on session end, where Windows may kill a running installer.
        // A pending update installs on an explicit quit (`shutdown`) or on the next start.
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

    fn parse(args: &[&str]) -> LaunchArgs {
        LaunchArgs::parse(args)
    }

    #[test]
    fn hidden_flag_is_detected_after_program_name() {
        assert!(parse(&["casecontrol.exe", "--hidden"]).hidden);
        assert!(parse(&["casecontrol.exe", "--other", "--hidden"]).hidden);
        assert!(!parse(&["casecontrol.exe"]).hidden);
        assert!(!parse(&["--hidden"]).hidden);
        assert!(!parse(&["casecontrol.exe", "--hidden=false"]).hidden);
    }

    #[test]
    fn quit_flag_is_detected_after_program_name() {
        assert_eq!(
            parse(&["casecontrol.exe", "--quit"]),
            LaunchArgs {
                hidden: false,
                quit: true
            }
        );
        assert_eq!(
            parse(&["casecontrol.exe", "--hidden", "--quit"]),
            LaunchArgs {
                hidden: true,
                quit: true
            }
        );
        assert!(!parse(&["--quit"]).quit);
        assert!(!parse(&["casecontrol.exe"]).quit);
        assert!(!parse(&["casecontrol.exe", "--quit=true"]).quit);
        assert!(!parse(&["casecontrol.exe", "--QUIT"]).quit);
        assert!(!parse(&["casecontrol.exe", "quit"]).quit);
        assert_eq!(parse(&[]), LaunchArgs::default());
    }

    #[test]
    fn update_now_restarts_without_hidden_or_quit() {
        assert_eq!(
            update_now_args([r"C:\Apps\casecontrol.exe", "--hidden"]),
            Vec::<String>::new()
        );
        assert_eq!(
            update_now_args(["casecontrol.exe", "--hidden", "--other", "--quit", "two words"]),
            vec!["--other".to_owned(), "two words".to_owned()]
        );
        // Exact matches only, like `LaunchArgs::parse`; the program name is never passed on.
        assert_eq!(
            update_now_args(["casecontrol.exe", "--hidden=false", "--HIDDEN"]),
            vec!["--hidden=false".to_owned(), "--HIDDEN".to_owned()]
        );
        assert!(update_now_args(Vec::<String>::new()).is_empty());
    }

    #[test]
    fn exit_paths_exclude_each_other() {
        assert!(begin_exit(), "the first exit path claims the exit");
        assert!(!begin_exit(), "a second one is refused while the first runs");
        cancel_exit();
        assert!(
            begin_exit(),
            "free again after an Update now that changed nothing"
        );
        cancel_exit();
    }

    #[test]
    fn forwarded_second_instance_args_parse() {
        // The single-instance callback receives the second instance's argv as `Vec<String>`.
        let args = vec![r"C:\Apps\casecontrol.exe".to_owned(), "--quit".to_owned()];
        assert!(LaunchArgs::parse(&args).quit);
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_argument_does_not_panic() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let lone_surrogate = OsString::from_wide(&[0xD800]);
        assert_eq!(
            LaunchArgs::parse([OsString::from("casecontrol.exe"), lone_surrogate.clone()]),
            LaunchArgs::default()
        );
        let parsed = LaunchArgs::parse([
            OsString::from("casecontrol.exe"),
            lone_surrogate,
            OsString::from("--hidden"),
            OsString::from("--quit"),
        ]);
        assert!(parsed.hidden && parsed.quit);
    }
}
