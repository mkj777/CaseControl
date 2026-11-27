//! "Start with Windows" (ADR 0001 #42): a per-user Task Scheduler logon task (`cc_system::TaskSpec::logon`,
//! starting this executable with `--hidden`), replacing the HKCU `Run` entry earlier versions wrote through
//! `tauri-plugin-autostart`.
//!
//! Only release builds create or delete the task: a debug build has a console window, so a task pointing at it
//! would open a terminal at every sign-in. Switching autostart in a debug build changes only the setting (and logs
//! that no task is written).
//!
//! At start ([`sync_at_start`], release builds, on a background thread, skipped while the settings cannot be saved and
//! serialised with the "Start with Windows" switch): an old `Run` entry that was switched off in Task Manager, or a
//! logon task of ours that was disabled (in Task Manager or the Task Scheduler), turns autostart off (the user's last
//! word; the task is never silently enabled again); with autostart on, the task is made to exist and point at this
//! executable (updated in place when it differs, for example after an install elsewhere); once that succeeded, the
//! old HKCU `Run\CaseControl` and `StartupApproved\Run\CaseControl` values are removed. With autostart off, a task
//! of ours that is still there is deleted. Only the user switching autostart on enables a disabled task again.

use std::path::Path;

use cc_conflicts::{AutostartEntry, AutostartLocation};
use cc_system::{Ownership, TaskSpec, TaskState, TaskStore, not_ours_message, register_verified, task_state};

/// Whether this build creates and deletes the logon task (release builds only).
pub const MANAGES_TASK: bool = !cfg!(debug_assertions);

/// The name of the HKCU `Run` value earlier versions wrote (the product name).
pub const LEGACY_RUN_VALUE: &str = "CaseControl";

/// CaseControl's old entry among `entries`: the HKCU `Run` value named `app_name`.
pub fn own_entry<'a>(entries: &'a [AutostartEntry], app_name: &str) -> Option<&'a AutostartEntry> {
    entries
        .iter()
        .find(|e| e.location == AutostartLocation::HkcuRun && e.name.eq_ignore_ascii_case(app_name))
}

/// What the start does about autostart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartPlan {
    /// Set the autostart setting to off (the old entry was switched off in Task Manager, or the logon task was
    /// disabled).
    pub turn_off: bool,
    /// Make sure the logon task exists and points at this executable.
    pub ensure_task: bool,
    /// Delete a logon task of ours (autostart is off).
    pub delete_task: bool,
    /// Remove the old HKCU `Run` and `StartupApproved` values (after the task was made sure of, when it is).
    pub remove_legacy: bool,
}

/// The start-up migration and check (pure): `autostart` is the setting, `legacy` the old HKCU `Run` entry if there
/// is one, `task_disabled` whether the logon task is ours but disabled ([`TaskState::Disabled`]). An old entry
/// switched off in Task Manager migrates as autostart off; a disabled task counts the same.
pub fn start_plan(
    release_build: bool,
    autostart: bool,
    legacy: Option<&AutostartEntry>,
    task_disabled: bool,
) -> StartPlan {
    if !release_build {
        return StartPlan {
            turn_off: false,
            ensure_task: false,
            delete_task: false,
            remove_legacy: false,
        };
    }
    let switched_off = legacy.is_some_and(|e| e.approved == Some(false)) || task_disabled;
    let on = autostart && !switched_off;
    StartPlan {
        turn_off: autostart && switched_off,
        ensure_task: on,
        delete_task: !on,
        remove_legacy: legacy.is_some(),
    }
}

/// What [`ensure_task`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
    /// The task was there and right.
    AlreadyRight,
    /// The task was missing and is created now.
    Created,
    /// The task differed (these differences) and is updated in place now.
    Recreated(Vec<String>),
    /// The task was ours but disabled and is enabled again (only on the user's explicit switch-on).
    Reenabled,
}

/// Makes the logon task `spec` exist exactly as described in `store`: creates it when missing, updates it in place
/// when it is ours but differs (for example a stale executable path) or is disabled. A task of that name that is not
/// ours is left alone and reported as [`cc_system::Error::NotOurs`] with a text meant to be shown as it is. Callers
/// that must not enable a disabled task check [`task_state`] first (see [`sync_at_start`]).
pub fn ensure_task(store: &dyn TaskStore, spec: &TaskSpec) -> Result<Ensured, cc_system::Error> {
    let outcome = match task_state(store, spec)? {
        TaskState::Matches => return Ok(Ensured::AlreadyRight),
        TaskState::Foreign(why) => {
            return Err(cc_system::Error::NotOurs(not_ours_message(&spec.name(), &why)));
        }
        TaskState::Missing => Ensured::Created,
        TaskState::Differs(diffs) => Ensured::Recreated(diffs),
        TaskState::Disabled => Ensured::Reenabled,
    };
    register_verified(store, spec)?;
    Ok(outcome)
}

/// Deletes the logon task `spec` names if it is ours; `Ok(false)` when there is none.
pub fn delete_task(store: &dyn TaskStore, spec: &TaskSpec) -> Result<bool, cc_system::Error> {
    cc_system::delete_ours(store, &Ownership::of(spec))
}

/// The text the Settings page shows for a failed autostart change: a task under our name that is not ours is
/// explained as it is ([`cc_system::not_ours_message`]); anything else gets a short lead.
pub fn change_error_text(err: &cc_system::Error) -> String {
    match err {
        cc_system::Error::NotOurs(message) => message.clone(),
        other => format!("Start with Windows could not be changed: {other}"),
    }
}

/// The logon task for this user and `exe`.
pub fn logon_spec(exe: &Path) -> Result<TaskSpec, String> {
    let sid = cc_system::current_user_sid().map_err(|e| format!("cannot determine the user: {e}"))?;
    TaskSpec::logon(&sid, exe).map_err(|e| e.to_string())
}

/// Applies an autostart change from the settings: creates (`enabled`) or deletes the logon task and removes an old
/// `Run` entry; in a debug build only logs that nothing is written.
pub fn apply(enabled: bool) -> Result<(), String> {
    if !MANAGES_TASK {
        tracing::info!(
            enabled,
            "debug build: autostart setting changed, the logon task is not written"
        );
        return Ok(());
    }
    #[cfg(windows)]
    {
        let exe =
            std::env::current_exe().map_err(|e| format!("cannot locate CaseControl's executable: {e}"))?;
        let spec = logon_spec(&exe)?;
        let store = cc_system::WindowsTaskStore;
        if enabled {
            let outcome = ensure_task(&store, &spec).map_err(|e| change_error_text(&e))?;
            tracing::info!(task = %spec.name(), ?outcome, "autostart on: logon task ready");
        } else {
            let deleted = delete_task(&store, &spec).map_err(|e| change_error_text(&e))?;
            tracing::info!(task = %spec.name(), deleted, "autostart off");
        }
        remove_legacy_entry();
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("autostart is only available on Windows".to_owned())
    }
}

/// Whether the logon task is ours but disabled; a task that cannot be read counts as not disabled (logged).
pub fn task_is_disabled(store: &dyn TaskStore, spec: &TaskSpec) -> bool {
    match task_state(store, spec) {
        Ok(state) => state == TaskState::Disabled,
        Err(err) => {
            tracing::warn!(%err, "cannot read the logon task");
            false
        }
    }
}

/// Removes the old HKCU `Run` entry and its `StartupApproved` value; problems are logged.
#[cfg(windows)]
fn remove_legacy_entry() {
    match cc_conflicts::remove_own_run_entry(LEGACY_RUN_VALUE) {
        Ok(true) => tracing::info!("old autostart entry (HKCU Run) removed"),
        Ok(false) => {}
        Err(err) => tracing::warn!(%err, "cannot remove the old autostart entry"),
    }
}

/// The start-up migration and check (see the module docs). Runs [`start_plan`] against the real old entry and Task
/// Scheduler; `turn_off` is called when autostart has to be switched off in the settings. Errors are logged. The
/// caller skips it while the settings cannot be saved (an unreadable settings file must never delete the task) and
/// holds the autostart lock, so the "Start with Windows" switch does not run meanwhile.
pub fn sync_at_start(autostart: bool, turn_off: impl FnOnce()) {
    if !MANAGES_TASK {
        return;
    }
    #[cfg(windows)]
    {
        use cc_conflicts::AutostartSource as _;

        let entries = cc_conflicts::WindowsAutostart
            .autostart_entries()
            .unwrap_or_else(|err| {
                tracing::warn!(%err, "cannot read autostart entries; old entry not migrated");
                Vec::new()
            });
        let legacy = own_entry(&entries, LEGACY_RUN_VALUE);
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => {
                tracing::warn!(%err, "cannot determine the executable; logon task not checked");
                return;
            }
        };
        let spec = match logon_spec(&exe) {
            Ok(spec) => spec,
            Err(err) => {
                tracing::warn!(%err, "logon task not checked");
                return;
            }
        };
        let store = cc_system::WindowsTaskStore;
        let task_disabled = autostart && task_is_disabled(&store, &spec);
        let plan = start_plan(MANAGES_TASK, autostart, legacy, task_disabled);
        if plan.turn_off {
            if task_disabled {
                tracing::info!("the logon task was disabled; autostart turned off");
            } else {
                tracing::info!(
                    "the old autostart entry was switched off in Task Manager; autostart turned off"
                );
            }
            turn_off();
        }
        let task_ok = if plan.ensure_task {
            match ensure_task(&store, &spec) {
                Ok(outcome) => {
                    tracing::info!(task = %spec.name(), ?outcome, "logon task checked");
                    true
                }
                Err(err) => {
                    tracing::warn!(%err, "cannot create the logon task; the old autostart entry is kept");
                    false
                }
            }
        } else {
            if plan.delete_task {
                match delete_task(&store, &spec) {
                    Ok(true) => tracing::info!(task = %spec.name(), "autostart is off; logon task deleted"),
                    Ok(false) => {}
                    Err(err) => tracing::warn!(%err, "cannot delete the logon task"),
                }
            }
            true
        };
        if plan.remove_legacy && task_ok {
            remove_legacy_entry();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (autostart, turn_off);
    }
}

#[cfg(test)]
mod tests {
    use cc_conflicts::command_executable;
    use cc_system::MemoryTaskStore;

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

    const SID: &str = "S-1-5-21-1000000000-2000000000-3000000000-1001";
    const EXE: &str = r"C:\Users\me\AppData\Local\CaseControl\casecontrol-desktop.exe";

    fn spec(exe: &str) -> TaskSpec {
        TaskSpec::logon(SID, Path::new(exe)).expect("valid spec")
    }

    #[test]
    fn debug_builds_change_nothing() {
        let legacy = entry(&format!("{EXE} --hidden"), Some(false));
        let plan = start_plan(false, true, Some(&legacy), false);
        assert!(!plan.turn_off && !plan.ensure_task && !plan.delete_task && !plan.remove_legacy);
    }

    #[test]
    fn autostart_on_migrates_to_the_task_and_removes_the_old_entry() {
        let legacy = entry(&format!("{EXE} --hidden"), None);
        assert_eq!(
            start_plan(true, true, Some(&legacy), false),
            StartPlan {
                turn_off: false,
                ensure_task: true,
                delete_task: false,
                remove_legacy: true
            }
        );
        // Approved explicitly on is the same.
        let on = entry(&format!("{EXE} --hidden"), Some(true));
        assert!(start_plan(true, true, Some(&on), false).ensure_task);
        // Without an old entry the task is still made sure of.
        let plan = start_plan(true, true, None, false);
        assert!(plan.ensure_task && !plan.remove_legacy && !plan.turn_off);
    }

    #[test]
    fn an_entry_switched_off_in_task_manager_migrates_as_off() {
        let legacy = entry(&format!("{EXE} --hidden"), Some(false));
        assert_eq!(
            start_plan(true, true, Some(&legacy), false),
            StartPlan {
                turn_off: true,
                ensure_task: false,
                delete_task: true,
                remove_legacy: true
            }
        );
    }

    #[test]
    fn autostart_off_deletes_a_leftover_task_and_entry() {
        let legacy = entry(&format!("{EXE} --hidden"), None);
        let plan = start_plan(true, false, Some(&legacy), false);
        assert!(!plan.turn_off && !plan.ensure_task && plan.delete_task && plan.remove_legacy);
        let plan = start_plan(true, false, None, false);
        assert!(plan.delete_task && !plan.remove_legacy);
    }

    #[test]
    fn ensure_creates_a_missing_task_and_keeps_a_right_one() {
        let store = MemoryTaskStore::new();
        let spec = spec(EXE);
        assert_eq!(ensure_task(&store, &spec), Ok(Ensured::Created));
        assert!(store.get(&spec.name()).is_some());
        assert_eq!(ensure_task(&store, &spec), Ok(Ensured::AlreadyRight));
    }

    #[test]
    fn ensure_recreates_a_task_that_points_at_another_executable() {
        let store = MemoryTaskStore::new();
        let old = spec(r"D:\Old\casecontrol-desktop.exe");
        assert_eq!(ensure_task(&store, &old), Ok(Ensured::Created));
        let new = spec(EXE);
        match ensure_task(&store, &new) {
            Ok(Ensured::Recreated(diffs)) => assert!(!diffs.is_empty()),
            other => panic!("expected a re-created task, got {other:?}"),
        }
        let info = store.get(&new.name()).expect("task exists");
        assert!(new.verify(&info).is_ok());
    }

    #[test]
    fn ensure_leaves_a_foreign_task_alone() {
        let store = MemoryTaskStore::new();
        let spec = spec(EXE);
        let mut foreign = spec.expected();
        foreign.exec_actions[0].path = r"C:\evil\other.exe".into();
        store.insert(&spec.name(), foreign.clone());
        assert!(ensure_task(&store, &spec).is_err());
        assert_eq!(store.get(&spec.name()), Some(foreign));
        assert!(delete_task(&store, &spec).is_err(), "never deleted either");
    }

    #[test]
    fn delete_removes_only_our_task() {
        let store = MemoryTaskStore::new();
        let spec = spec(EXE);
        assert_eq!(delete_task(&store, &spec), Ok(false));
        ensure_task(&store, &spec).expect("created");
        assert_eq!(delete_task(&store, &spec), Ok(true));
        assert!(store.get(&spec.name()).is_none());
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

    #[test]
    fn a_disabled_task_turns_autostart_off_and_is_not_enabled_at_start() {
        let store = MemoryTaskStore::new();
        let spec = spec(EXE);
        ensure_task(&store, &spec).expect("created");
        let mut info = store.get(&spec.name()).expect("task exists");
        info.enabled = false;
        store.insert(&spec.name(), info.clone());
        assert!(task_is_disabled(&store, &spec));
        assert_eq!(
            start_plan(true, true, None, true),
            StartPlan {
                turn_off: true,
                ensure_task: false,
                delete_task: true,
                remove_legacy: false
            }
        );
        // Checking changed nothing; only an explicit switch-on enables it again.
        assert_eq!(store.get(&spec.name()), Some(info));
        assert_eq!(ensure_task(&store, &spec), Ok(Ensured::Reenabled));
        assert!(!task_is_disabled(&store, &spec));
        assert_eq!(ensure_task(&store, &spec), Ok(Ensured::AlreadyRight));
    }

    #[test]
    fn a_foreign_task_is_explained_as_it_is() {
        let store = MemoryTaskStore::new();
        let spec = spec(EXE);
        let mut foreign = spec.expected();
        foreign.exec_actions[0].path = r"C:\evil\other.exe".into();
        store.insert(&spec.name(), foreign);
        let err = ensure_task(&store, &spec).expect_err("not ours");
        let text = change_error_text(&err);
        assert!(text.contains("was not created by CaseControl"), "{text}");
        assert!(
            !text.starts_with("Start with Windows could not be changed"),
            "{text}"
        );
        let other = change_error_text(&cc_system::Error::Invalid("x".into()));
        assert_eq!(other, "Start with Windows could not be changed: x");
    }
}
