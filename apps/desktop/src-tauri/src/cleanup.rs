//! The uninstaller's clean-up mode, `--uninstall-cleanup` (ADR 0001 #42).
//!
//! The NSIS uninstaller (`windows/hooks.nsh`, `NSIS_HOOK_PREUNINSTALL`; not on updates and not when the installer
//! runs it for a reinstall) runs the installed executable with this flag and waits; a silent uninstall (`/S`) adds
//! [`NO_PROMPT_FLAG`]. [`cleanup_main`] catches it in `main` before [`crate::run`] (no Tauri, no single-instance
//! guard) and, best effort and bounded in time:
//!
//! 1. asks a running instance to quit with [`crate::UNINSTALL_QUIT_FLAG`] (it releases its devices without saving to
//!    them and without installing a pending update; the updater's files are left alone) and waits for it, then takes
//!    the cross-session device lock so no instance drives the devices any more;
//! 2. deletes the logon task (no rights needed);
//! 3. writes the original saved state back to every device CaseControl saved colours to (the engine's restore of
//!    "Save colours to the devices"), when there is a backup. The device families of those devices are detected even
//!    when they are switched off in Settings, and pending restores are cleared first so the engine's restore on
//!    connect and the restore here never both run for a device (the restore marks them pending again itself);
//! 4. only if the start-up task exists ("Apply colours before sign-in") and the uninstall is not silent, removes it
//!    through the elevated helper (one UAC prompt; declining leaves the task and is logged). A silent uninstall
//!    never shows a prompt and leaves the task (logged).
//!
//! Every problem is logged and the mode always exits with code 0, so the uninstall goes on.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::time::{Duration, Instant};

use cc_config::{HardwareState, Paths};
use cc_engine::EngineConfig;

/// The flag the uninstaller passes.
pub const UNINSTALL_CLEANUP_FLAG: &str = "--uninstall-cleanup";
/// The second flag a silent uninstall passes: no UAC prompt.
pub const NO_PROMPT_FLAG: &str = "--no-prompt";

/// How long the running instance gets to quit.
const QUIT_WAIT: Duration = Duration::from_secs(15);
/// How long the device lock is waited for after that.
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// Time the restore gets.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the uninstaller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupArgs {
    /// Whether the clean-up may show a UAC prompt (not in a silent uninstall).
    pub prompt: bool,
}

/// The clean-up's command line: exactly `<exe> --uninstall-cleanup` or `<exe> --uninstall-cleanup --no-prompt`;
/// `None` for anything else.
pub fn parse_cleanup_invocation<S: AsRef<OsStr>>(args: &[S]) -> Option<CleanupArgs> {
    match args {
        [_, flag] if flag.as_ref() == OsStr::new(UNINSTALL_CLEANUP_FLAG) => {
            Some(CleanupArgs { prompt: true })
        }
        [_, flag, no_prompt]
            if flag.as_ref() == OsStr::new(UNINSTALL_CLEANUP_FLAG)
                && no_prompt.as_ref() == OsStr::new(NO_PROMPT_FLAG) =>
        {
            Some(CleanupArgs { prompt: false })
        }
        _ => None,
    }
}

/// Whether `args` is one of the clean-up's exact command lines (see [`parse_cleanup_invocation`]).
pub fn is_cleanup_invocation<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<S> = args.into_iter().collect();
    parse_cleanup_invocation(&args).is_some()
}

/// The device families to detect for the restore: the configured ones plus the family of every device with a backup
/// (so a family switched off in Settings is still restored); returns the families to keep switched off. A backed-up
/// device whose family cannot be told from its id switches every family on.
pub fn families_off_for_restore<'a>(
    disabled: &HashSet<String>,
    backed_up: impl IntoIterator<Item = &'a cc_core::DeviceId>,
) -> HashSet<String> {
    let mut off = disabled.clone();
    for id in backed_up {
        match crate::prefs::family_of(id) {
            Some(family) => {
                off.remove(family);
            }
            None => return HashSet::new(),
        }
    }
    off
}

/// Whether the start-up task has to be removed: when it can be read, whether it exists; when it cannot be read (the
/// task was created with administrator rights), the setting says whether it was created.
pub fn boot_task_present(read: Result<bool, String>, setting: bool) -> bool {
    match read {
        Ok(exists) => exists,
        Err(err) => {
            tracing::info!(%err, setting, "cannot read the start-up task; going by the setting");
            setting
        }
    }
}

/// Entry point for `main`: `None` unless the command line asks for the clean-up; otherwise runs it and returns its
/// exit code (always 0).
pub fn cleanup_main(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let args: Vec<OsString> = args.into_iter().collect();
    let parsed = parse_cleanup_invocation(&args)?;
    crate::logging::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        prompt = parsed.prompt,
        "uninstall clean-up"
    );
    run_cleanup(parsed);
    tracing::info!("uninstall clean-up done");
    Some(0)
}

fn run_cleanup(args: CleanupArgs) {
    quit_running_instance();
    let sid = match cc_system::current_user_sid() {
        Ok(sid) => sid,
        Err(err) => {
            tracing::warn!(%err, "cannot determine the user; nothing cleaned up");
            return;
        }
    };
    let lock = match cc_system::DeviceLock::acquire_with_timeout(&sid, LOCK_WAIT) {
        Ok(cc_system::LockStatus::Acquired(lock)) => Some(lock),
        Ok(status) => {
            tracing::warn!(
                ?status,
                "another CaseControl instance still holds the devices; they are not restored"
            );
            None
        }
        Err(err) => {
            tracing::warn!(%err, "cannot take the device lock; devices are not restored");
            None
        }
    };
    let paths = Paths::from_env();
    let settings = paths
        .as_ref()
        .map(|p| crate::boot::read_settings(p).0)
        .unwrap_or_default();
    delete_logon_task();
    if lock.is_some()
        && let Some(paths) = &paths
    {
        restore_devices(paths, &settings);
    }
    drop(lock);
    let setting = crate::prefs::PowerSettings::of(&settings).apply_before_sign_in;
    remove_boot_task(&sid, setting, args.prompt);
}

/// Starts `<exe> --quit-for-uninstall` and waits for it (bounded). The running instance releases its devices (no
/// save to them, no update install) before that process ends; with no instance running it ends at once.
fn quit_running_instance() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            tracing::warn!(%err, "cannot locate the executable; a running instance is not asked to quit");
            return;
        }
    };
    let mut child = match std::process::Command::new(&exe)
        .arg(crate::UNINSTALL_QUIT_FLAG)
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            tracing::warn!(%err, "cannot ask a running instance to quit");
            return;
        }
    };
    let deadline = Instant::now() + QUIT_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::info!(?status, "running instance asked to quit");
                return;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                tracing::warn!("the --quit process did not end in time; going on");
                return;
            }
            Err(err) => {
                tracing::warn!(%err, "cannot wait for the --quit process");
                return;
            }
        }
    }
}

fn delete_logon_task() {
    #[cfg(windows)]
    {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => {
                tracing::warn!(%err, "cannot locate the executable; logon task not deleted");
                return;
            }
        };
        match crate::autostart::logon_spec(&exe).and_then(|spec| {
            crate::autostart::delete_task(&cc_system::WindowsTaskStore, &spec).map_err(|e| e.to_string())
        }) {
            Ok(deleted) => tracing::info!(deleted, "logon task"),
            Err(err) => tracing::warn!(%err, "cannot delete the logon task"),
        }
    }
}

/// Writes the original saved state back to every device with a backup (nothing happens without one).
fn restore_devices(paths: &Paths, settings: &cc_config::Settings) {
    let mut hardware_state = HardwareState::load(paths);
    if hardware_state.saved_backups().is_empty() {
        tracing::info!("no device has saved colours; nothing to restore");
        return;
    }
    tracing::info!(
        devices = hardware_state.saved_backups().len(),
        pending = hardware_state.pending_restores().len(),
        "restoring the devices' original saved state"
    );
    // `restore_saved` below restores every backed-up device and marks it pending itself; a pending mark left from
    // earlier would make the engine restore the device a first time as soon as it connects.
    if let Err(err) = hardware_state.clear_pending_restores() {
        tracing::warn!(%err, "cannot clear the pending restores; devices are not restored");
        return;
    }
    let base = crate::prefs::engine_config(settings, hardware_state);
    let disabled_families = families_off_for_restore(
        &base.disabled_families,
        base.hardware_state.saved_backups().keys(),
    );
    let config = EngineConfig {
        hotplug: false,
        resume: false,
        disabled_families,
        ..base
    };
    let engine = match cc_engine::start(config, cc_drivers::all_backends()) {
        Ok(engine) => engine,
        Err(err) => {
            tracing::warn!(%err, "the device engine did not start; devices are not restored");
            return;
        }
    };
    let report = engine.restore_saved(RESTORE_TIMEOUT);
    crate::power::log_report(&report, true);
    engine.shutdown(crate::power::STOP_TIMEOUT);
}

/// Removes the start-up task through the elevated helper when it exists (one UAC prompt; none when `prompt` is
/// false, a silent uninstall, which leaves the task).
fn remove_boot_task(sid: &str, setting: bool, prompt: bool) {
    #[cfg(windows)]
    let read = {
        use cc_system::TaskStore as _;
        cc_system::WindowsTaskStore
            .read(&cc_system::boot_task_name(sid))
            .map(|t| t.is_some())
            .map_err(|e| e.to_string())
    };
    #[cfg(not(windows))]
    let read: Result<bool, String> = {
        let _ = sid;
        Ok(false)
    };
    if !boot_task_present(read, setting) {
        tracing::info!("no start-up task");
        return;
    }
    if !prompt {
        tracing::warn!(
            "silent uninstall: the start-up task is left (removing it needs a Windows permission prompt)"
        );
        return;
    }
    tracing::info!("removing the start-up task (needs administrator rights)");
    let action = cc_conflicts::PlannedAction {
        entry_id: cc_conflicts::BOOT_TASK_ENTRY.to_owned(),
        action: cc_conflicts::Action::RemoveBootTask {},
    };
    let outcomes = crate::elevated::run_admin_batch(&[action], cc_conflicts::current_session_id(), None);
    match outcomes.first() {
        Some(o) if o.ok => tracing::info!("start-up task removed"),
        Some(o) => tracing::warn!(error = ?o.error, "the start-up task was not removed (declined or failed)"),
        None => tracing::warn!("the start-up task was not removed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_flag_enters_the_cleanup() {
        assert!(is_cleanup_invocation(["cc.exe", "--uninstall-cleanup"]));
        assert!(!is_cleanup_invocation(["cc.exe"]));
        assert!(!is_cleanup_invocation(["--uninstall-cleanup"]));
        assert!(!is_cleanup_invocation(["cc.exe", "--uninstall-cleanup", "x"]));
        assert!(!is_cleanup_invocation([
            "cc.exe",
            "--hidden",
            "--uninstall-cleanup"
        ]));
        assert!(!is_cleanup_invocation(["cc.exe", "--UNINSTALL-CLEANUP"]));
        assert_eq!(cleanup_main(["cc.exe", "--hidden"].map(OsString::from)), None);
        assert_eq!(
            parse_cleanup_invocation(&["cc.exe", "--uninstall-cleanup"]),
            Some(CleanupArgs { prompt: true })
        );
        assert_eq!(
            parse_cleanup_invocation(&["cc.exe", "--uninstall-cleanup", "--no-prompt"]),
            Some(CleanupArgs { prompt: false })
        );
        assert_eq!(
            parse_cleanup_invocation(&["cc.exe", "--no-prompt", "--uninstall-cleanup"]),
            None
        );
        assert_eq!(
            parse_cleanup_invocation(&["cc.exe", "--uninstall-cleanup", "--NO-PROMPT"]),
            None
        );
    }

    #[test]
    fn switched_off_families_with_backups_are_detected_for_the_restore() {
        use cc_core::{DeviceId, family};
        let disabled = HashSet::from([family::MSI_MYSTIC_LIGHT.to_owned(), family::WOOTING.to_owned()]);
        let board = DeviceId::hid(0x1462, 0x7D75, None);
        let gpu = DeviceId::nvapi(0x2702_10DE, 0x1612_10DE);
        assert_eq!(
            families_off_for_restore(&disabled, [&board, &gpu]),
            HashSet::from([family::WOOTING.to_owned()])
        );
        assert_eq!(families_off_for_restore(&disabled, []), disabled);
        // An id whose family is unknown switches every family on.
        let unknown = DeviceId::hid(0x1234, 0x5678, None);
        assert!(families_off_for_restore(&disabled, [&unknown]).is_empty());
    }

    #[test]
    fn boot_task_is_removed_when_it_exists_or_the_setting_says_so() {
        assert!(boot_task_present(Ok(true), false));
        assert!(!boot_task_present(Ok(false), true), "a readable answer wins");
        assert!(boot_task_present(Err("access denied".into()), true));
        assert!(!boot_task_present(Err("access denied".into()), false));
    }
}
