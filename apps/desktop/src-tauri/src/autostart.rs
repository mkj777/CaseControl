//! CaseControl's own autostart entry (the HKCU `Run` value the autostart plugin writes).
//!
//! Only release builds write it: a debug build has a console window, so an entry pointing at it opens a terminal
//! at every log-on. Toggling autostart in a debug build changes only the setting (and logs that the entry is not
//! written). On start, a release build with autostart on rewrites the entry when it points at another executable,
//! e.g. a debug build or an older install location, so installing the release fixes the path.

use std::path::Path;

use cc_conflicts::{
    AutostartEntry, AutostartLocation, AutostartSource as _, WindowsAutostart, command_executable,
};
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt as _;

/// Whether this build writes the `Run` value (release builds only).
pub const WRITES_RUN_ENTRY: bool = !cfg!(debug_assertions);

/// Whether the start-up check rewrites the entry: only in a release build, only with autostart on in the settings,
/// only for an entry that exists, is not switched off in Task Manager (rewriting would switch it on again) and
/// starts another executable than `current_exe` (compared case-insensitively, `/` and `\` alike).
pub fn needs_rewrite(
    release_build: bool,
    autostart: bool,
    entry: Option<&AutostartEntry>,
    current_exe: &Path,
) -> bool {
    if !release_build || !autostart {
        return false;
    }
    let Some(entry) = entry else {
        return false;
    };
    if entry.approved == Some(false) {
        return false;
    }
    let registered = entry
        .executable
        .clone()
        .or_else(|| command_executable(&entry.command));
    let norm = |s: &str| s.trim().replace('/', "\\").to_lowercase();
    registered.is_none_or(|r| norm(&r) != norm(&current_exe.display().to_string()))
}

/// CaseControl's entry among `entries`: the HKCU `Run` value named `app_name`.
pub fn own_entry<'a>(entries: &'a [AutostartEntry], app_name: &str) -> Option<&'a AutostartEntry> {
    entries
        .iter()
        .find(|e| e.location == AutostartLocation::HkcuRun && e.name.eq_ignore_ascii_case(app_name))
}

/// The start-up check: rewrites the entry to this executable when [`needs_rewrite`] says so. Errors are logged.
pub fn sync_run_entry<R: Runtime>(app: &AppHandle<R>, autostart: bool) {
    if !WRITES_RUN_ENTRY || !autostart {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            tracing::warn!(%err, "cannot determine the executable; autostart entry not checked");
            return;
        }
    };
    let entries = match WindowsAutostart.autostart_entries() {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(%err, "cannot read autostart entries; autostart entry not checked");
            return;
        }
    };
    let entry = own_entry(&entries, &app.package_info().name);
    if !needs_rewrite(WRITES_RUN_ENTRY, autostart, entry, &exe) {
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => tracing::info!(
            old = entry.map(|e| e.command.as_str()).unwrap_or_default(),
            new = %exe.display(),
            "autostart entry pointed at another executable; rewritten"
        ),
        Err(err) => tracing::warn!(%err, "cannot rewrite the autostart entry"),
    }
}

/// Applies an autostart change from the settings to the `Run` value; in a debug build only logs that it is not
/// written.
pub fn apply<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<(), String> {
    if !WRITES_RUN_ENTRY {
        tracing::info!(
            enabled,
            "debug build: autostart setting changed, the Run entry is not written"
        );
        return Ok(());
    }
    let autolaunch = app.autolaunch();
    let result = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    result.map_err(|err| format!("cannot change autostart: {err}"))?;
    tracing::info!(enabled, "autostart changed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(command: &str, approved: Option<bool>) -> AutostartEntry {
        AutostartEntry {
            location: AutostartLocation::HkcuRun,
            name: "CaseControl".into(),
            command: command.into(),
            executable: command_executable(command),
            approved,
        }
    }

    const RELEASE: &str = r"C:\Program Files\CaseControl\casecontrol-desktop.exe";
    const DEBUG: &str = r"C:\src\casecontrol\target\debug\casecontrol-desktop.exe";

    #[test]
    fn release_build_rewrites_an_entry_that_points_elsewhere() {
        let exe = Path::new(RELEASE);
        let stale = entry(&format!("{DEBUG} --hidden"), None);
        assert!(needs_rewrite(true, true, Some(&stale), exe));
        // Already right (also in another case or with forward slashes): nothing to do.
        let ok = entry(&format!("{RELEASE} --hidden"), None);
        assert!(!needs_rewrite(true, true, Some(&ok), exe));
        let upper = entry(
            &format!("{} --hidden", RELEASE.to_uppercase().replace('\\', "/")),
            None,
        );
        assert!(!needs_rewrite(true, true, Some(&upper), exe));
        // Unparseable command: rewrite.
        let empty = AutostartEntry {
            executable: None,
            ..entry("", None)
        };
        assert!(needs_rewrite(true, true, Some(&empty), exe));
    }

    #[test]
    fn nothing_is_written_otherwise() {
        let exe = Path::new(RELEASE);
        let stale = entry(&format!("{DEBUG} --hidden"), None);
        assert!(!needs_rewrite(false, true, Some(&stale), exe), "debug build");
        assert!(!needs_rewrite(true, false, Some(&stale), exe), "autostart off");
        assert!(!needs_rewrite(true, true, None, exe), "no entry");
        let off = entry(&format!("{DEBUG} --hidden"), Some(false));
        assert!(
            !needs_rewrite(true, true, Some(&off), exe),
            "switched off in Task Manager"
        );
    }

    #[test]
    fn own_entry_is_the_hkcu_run_value_of_the_app() {
        let mut other = entry("x.exe", None);
        other.name = "Other".into();
        let mut machine = entry("y.exe", None);
        machine.location = AutostartLocation::HklmRun;
        let own = entry("z.exe", None);
        let entries = vec![other, machine, own.clone()];
        assert_eq!(own_entry(&entries, "casecontrol"), Some(&own));
        assert_eq!(own_entry(&entries[..2], "CaseControl"), None);
    }
}
