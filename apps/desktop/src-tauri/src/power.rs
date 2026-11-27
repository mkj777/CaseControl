//! Colours across shutdown, sign-out and start (ADR 0001 #42): what the exit path does with the devices, the
//! cross-session device lock of the signed-in instance, and the short line Settings shows after "Save colours to the
//! devices" was switched.
//!
//! The exit decision ([`exit_plan`]) is a pure table over why the app closes (`cc_system::SessionEnd`, recorded by
//! the session-end watch installed in the setup hook) and the two Hardware switches:
//!
//! | Why                                   | Devices                                   | Save to devices first |
//! |---------------------------------------|-------------------------------------------|-----------------------|
//! | `NotEnding` (Quit, `--quit`, Update now) | released                               | when the switch is on |
//! | `CloseApp` (Restart Manager: installer)  | released                               | no                    |
//! | `ShutdownOrRestart`, `Unknown`        | kept                                      | when the switch is on |
//! | `SignOut`                             | kept with "Keep colours when signing out", else released | when the switch is on |
//!
//! At a session end (and for the Restart Manager) the time budgets are short, so the profile write, the save and
//! the stop together stay well under Windows' limit of about 5 s.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use cc_engine::{PersistReport, PersistResult, ShutdownMode, SkipReason};
use cc_system::{DeviceLock, LockStatus, SessionEnd};

/// Time the engine gets to release or keep every device on an explicit exit (Quit, `--quit`, Update now).
pub const STOP_TIMEOUT: Duration = Duration::from_secs(3);
/// Time the engine gets to release or keep every device at a session end or a Restart Manager close.
pub const SESSION_END_STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// Time "Save colours to the devices" gets on an explicit exit.
pub const PERSIST_TIMEOUT: Duration = Duration::from_secs(5);
/// Time "Save colours to the devices" gets at a session end (together with the stop well under 5 s).
pub const SESSION_END_PERSIST_TIMEOUT: Duration = Duration::from_millis(1500);
/// Time the switch itself (turning "Save colours to the devices" on or off in Settings) gets.
pub const SWITCH_PERSIST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the signed-in instance waits for a running start-up instance (`--apply-at-boot`) to let go of the
/// devices before it starts its engine anyway. It asks that instance to finish first (the yield signal), so the wait
/// is normally short.
pub const SESSION_LOCK_WAIT: Duration = Duration::from_secs(10);
/// How long the exit path waits for a running switch of "Save colours to the devices" at a session end before it
/// stops the engine without saving (a switch-off restore has already marked its devices pending, so they are restored
/// when they next connect).
pub const SESSION_END_SWITCH_WAIT: Duration = Duration::from_secs(1);
/// How long an explicit exit (Quit, `--quit`, Update now) waits for a running switch: its own budget plus a margin.
pub const QUIT_SWITCH_WAIT: Duration = Duration::from_secs(12);

/// What the exit path does with the devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitPlan {
    /// Release the devices or keep CaseControl's colours on them.
    pub mode: ShutdownMode,
    /// Save the colours to the devices that can store them before stopping.
    pub persist: bool,
    /// Budget of the save.
    pub persist_timeout: Duration,
    /// Budget of the stop.
    pub stop_timeout: Duration,
    /// How long to wait for a running switch of "Save colours to the devices" before the save is skipped.
    pub switch_wait: Duration,
}

impl ExitPlan {
    /// The plan when the uninstaller's clean-up asks the app to quit: release every device, never save to them.
    pub fn uninstall() -> Self {
        Self {
            mode: ShutdownMode::Release,
            persist: false,
            persist_timeout: Duration::ZERO,
            stop_timeout: STOP_TIMEOUT,
            switch_wait: Duration::ZERO,
        }
    }
}

/// The exit decision table (see the module docs).
pub fn exit_plan(why: SessionEnd, keep_on_sign_out: bool, save_to_devices: bool) -> ExitPlan {
    let (mode, persist) = match why {
        SessionEnd::NotEnding => (ShutdownMode::Release, save_to_devices),
        SessionEnd::CloseApp => (ShutdownMode::Release, false),
        SessionEnd::ShutdownOrRestart | SessionEnd::Unknown => (ShutdownMode::Keep, save_to_devices),
        SessionEnd::SignOut if keep_on_sign_out => (ShutdownMode::Keep, save_to_devices),
        SessionEnd::SignOut => (ShutdownMode::Release, save_to_devices),
    };
    let explicit = why == SessionEnd::NotEnding;
    ExitPlan {
        mode,
        persist,
        persist_timeout: if explicit {
            PERSIST_TIMEOUT
        } else {
            SESSION_END_PERSIST_TIMEOUT
        },
        stop_timeout: if explicit {
            STOP_TIMEOUT
        } else {
            SESSION_END_STOP_TIMEOUT
        },
        switch_wait: if explicit {
            QUIT_SWITCH_WAIT
        } else {
            SESSION_END_SWITCH_WAIT
        },
    }
}

thread_local! {
    /// The cross-session device lock this instance holds while its engine runs. A Windows mutex belongs to the
    /// thread that took it, so it lives on the main thread, which starts and stops the engine.
    static HELD: RefCell<Option<DeviceLock>> = const { RefCell::new(None) };
}

/// What [`take_device_lock`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLock {
    /// This instance holds the lock now (kept on this thread until [`drop_device_lock`]).
    Held,
    /// A start-up instance did not let go within the wait; this instance runs without the lock.
    Busy,
    /// Another account owns a mutex of that name; this instance runs without the lock.
    Foreign,
    /// The lock could not be checked (no SID, not Windows, a system error).
    Unavailable,
}

/// Takes the cross-session device lock for the current user (ADR 0001 #42), waiting up to `wait` for a running
/// start-up instance, and keeps it on the calling thread. First it sets the yield signal, which asks a start-up
/// instance to finish now; the signal is kept open until the wait is over. Never fails: a lock that cannot be taken
/// is logged and the app runs anyway (not driving any device would be worse than a few seconds of overlap with a
/// start-up instance that is about to exit).
pub fn take_device_lock(wait: Duration) -> SessionLock {
    let sid = match cc_system::current_user_sid() {
        Ok(sid) => sid,
        Err(err) => {
            tracing::warn!(%err, "cannot determine the user's SID; running without the device lock");
            return SessionLock::Unavailable;
        }
    };
    // Kept alive until the lock is taken (the request lasts only while a handle is open).
    let _yield = match cc_system::request_yield(&sid) {
        Ok(cc_system::SignalStatus::Opened(signal)) => Some(signal),
        Ok(cc_system::SignalStatus::HeldBySomeoneElse) => {
            tracing::warn!("the yield signal belongs to another account; not asking the start-up instance");
            None
        }
        Err(err) => {
            tracing::warn!(%err, "cannot ask the start-up instance to finish");
            None
        }
    };
    match DeviceLock::acquire_with_timeout(&sid, wait) {
        Ok(LockStatus::Acquired(lock)) => {
            HELD.with(|held| *held.borrow_mut() = Some(lock));
            tracing::info!("device lock taken");
            SessionLock::Held
        }
        Ok(LockStatus::Busy) => {
            tracing::warn!(
                wait_ms = wait.as_millis() as u64,
                "the start-up instance still holds the device lock; starting without it"
            );
            SessionLock::Busy
        }
        Ok(LockStatus::HeldBySomeoneElse) => {
            tracing::warn!("the device lock belongs to another account; starting without it");
            SessionLock::Foreign
        }
        Err(err) => {
            tracing::warn!(%err, "cannot take the device lock; starting without it");
            SessionLock::Unavailable
        }
    }
}

/// Lets go of the device lock if this thread holds it (after the engine stopped).
pub fn drop_device_lock() {
    let dropped = HELD.with(|held| held.borrow_mut().take()).is_some();
    if dropped {
        tracing::info!("device lock released");
    }
}

/// Logs every device's result of a save (`restoring == false`) or a restore.
pub fn log_report(report: &PersistReport, restoring: bool) {
    let what = if restoring {
        "restore saved state"
    } else {
        "save colours to device"
    };
    for entry in &report.devices {
        match &entry.result {
            PersistResult::Failed { error } => {
                tracing::warn!(id = %entry.id, %error, "{what}: failed")
            }
            PersistResult::Unconfirmed => {
                tracing::warn!(id = %entry.id, "{what}: not confirmed in time; it may still complete")
            }
            result => tracing::info!(id = %entry.id, ?result, "{what}"),
        }
    }
    if report.devices.is_empty() {
        tracing::info!("{what}: no device to consider");
    }
}

/// The short phrase for a result Settings mentions, `None` for one it leaves out (written, unchanged, hidden, not
/// driven, nothing to save). `restoring` picks the restore wording.
fn note_reason(result: &PersistResult, restoring: bool) -> Option<&'static str> {
    match result {
        PersistResult::Written | PersistResult::Unchanged => None,
        PersistResult::Failed { .. } => Some("failed"),
        PersistResult::Unconfirmed => Some("not confirmed in time; it may still finish"),
        PersistResult::Skipped { reason } => match reason {
            SkipReason::NotCapable => Some("can't store lighting"),
            SkipReason::OriginalUnknown => Some("original state unknown"),
            SkipReason::DailyCap if restoring => Some("daily limit reached; tried again tomorrow"),
            SkipReason::DailyCap => Some("daily write limit reached"),
            SkipReason::TimeBudget => Some("ran out of time"),
            SkipReason::Faulted if restoring => Some("device error; restored when connected again"),
            SkipReason::Faulted => Some("device error"),
            SkipReason::WritesDisabled => Some("writing to devices is off"),
            SkipReason::StateUnavailable => Some("CaseControl can't store its records"),
            SkipReason::Absent if restoring => Some("not connected; restored when connected"),
            SkipReason::NoBackup if restoring => Some("no saved state to restore"),
            SkipReason::Hidden
            | SkipReason::NotDriven
            | SkipReason::NothingToSave
            | SkipReason::Absent
            | SkipReason::NoBackup => None,
        },
    }
}

/// The one short line under "Save colours to the devices" (ADR 0001 #42): the devices that were not saved
/// (`restoring == false`) or not restored, grouped by reason, e.g. `Not saved: Wooting 60HE, RTX 5080 (can't store
/// lighting); G560 (original state unknown).` `name` gives a device's display name. `None` when every device that
/// matters went through.
pub fn persist_note(
    report: &PersistReport,
    restoring: bool,
    name: impl Fn(&cc_core::DeviceId) -> String,
) -> Option<String> {
    // Reasons in order of first appearance, each with its device names.
    let mut groups: Vec<(&'static str, Vec<String>)> = Vec::new();
    let mut index: BTreeMap<&'static str, usize> = BTreeMap::new();
    for entry in &report.devices {
        let Some(reason) = note_reason(&entry.result, restoring) else {
            continue;
        };
        let i = *index.entry(reason).or_insert_with(|| {
            groups.push((reason, Vec::new()));
            groups.len() - 1
        });
        groups[i].1.push(name(&entry.id));
    }
    if groups.is_empty() {
        return None;
    }
    let body = groups
        .iter()
        .map(|(reason, names)| format!("{} ({reason})", names.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    let lead = if restoring { "Not restored" } else { "Not saved" };
    Some(format!("{lead}: {body}."))
}

#[cfg(test)]
mod tests {
    use cc_core::DeviceId;
    use cc_engine::PersistEntry;

    use super::*;

    #[test]
    fn explicit_quit_releases_and_saves_only_when_switched_on() {
        let plan = exit_plan(SessionEnd::NotEnding, true, false);
        assert_eq!(plan.mode, ShutdownMode::Release);
        assert!(!plan.persist);
        assert_eq!(plan.stop_timeout, STOP_TIMEOUT);
        let plan = exit_plan(SessionEnd::NotEnding, false, true);
        assert_eq!(plan.mode, ShutdownMode::Release);
        assert!(plan.persist);
        assert_eq!(plan.persist_timeout, PERSIST_TIMEOUT);
    }

    #[test]
    fn installer_close_releases_and_never_saves() {
        for (keep, save) in [(true, true), (false, false), (true, false), (false, true)] {
            let plan = exit_plan(SessionEnd::CloseApp, keep, save);
            assert_eq!(plan.mode, ShutdownMode::Release);
            assert!(!plan.persist);
            assert_eq!(plan.stop_timeout, SESSION_END_STOP_TIMEOUT);
        }
    }

    #[test]
    fn shutdown_restart_and_unknown_keep() {
        for why in [SessionEnd::ShutdownOrRestart, SessionEnd::Unknown] {
            for keep in [true, false] {
                let plan = exit_plan(why, keep, false);
                assert_eq!(plan.mode, ShutdownMode::Keep, "{why:?}");
                assert!(!plan.persist);
                assert!(exit_plan(why, keep, true).persist);
            }
        }
    }

    #[test]
    fn sign_out_follows_its_switch() {
        assert_eq!(
            exit_plan(SessionEnd::SignOut, true, false).mode,
            ShutdownMode::Keep
        );
        assert_eq!(
            exit_plan(SessionEnd::SignOut, false, false).mode,
            ShutdownMode::Release
        );
        assert!(exit_plan(SessionEnd::SignOut, false, true).persist);
        assert!(exit_plan(SessionEnd::SignOut, true, true).persist);
    }

    #[test]
    fn session_end_budgets_stay_well_under_five_seconds() {
        for why in [
            SessionEnd::SignOut,
            SessionEnd::ShutdownOrRestart,
            SessionEnd::Unknown,
            SessionEnd::CloseApp,
        ] {
            let plan = exit_plan(why, true, true);
            assert!(
                plan.persist_timeout + plan.stop_timeout <= Duration::from_secs(4),
                "{why:?}"
            );
        }
    }

    fn entry(id: &DeviceId, result: PersistResult) -> PersistEntry {
        PersistEntry {
            id: id.clone(),
            result,
        }
    }

    fn skipped(reason: SkipReason) -> PersistResult {
        PersistResult::Skipped { reason }
    }

    #[test]
    fn note_groups_the_devices_that_were_not_saved() {
        let msi = DeviceId::hid(0x1462, 0x7D75, None);
        let g560 = DeviceId::hid(0x046D, 0x0A78, None);
        let wooting = DeviceId::hid(0x31E3, 0x1310, None);
        let gpu = DeviceId::nvapi(0x2702_10DE, 0x1612_10DE);
        let hidden = DeviceId::hid(0x1, 0x2, None);
        let report = PersistReport {
            devices: vec![
                entry(&msi, PersistResult::Written),
                entry(&g560, skipped(SkipReason::OriginalUnknown)),
                entry(&wooting, skipped(SkipReason::NotCapable)),
                entry(&gpu, skipped(SkipReason::NotCapable)),
                entry(&hidden, skipped(SkipReason::Hidden)),
            ],
        };
        let names = |id: &DeviceId| {
            if *id == g560 {
                "G560".to_owned()
            } else if *id == wooting {
                "Wooting 60HE".to_owned()
            } else {
                "RTX 5080".to_owned()
            }
        };
        assert_eq!(
            persist_note(&report, false, names).as_deref(),
            Some("Not saved: G560 (original state unknown); Wooting 60HE, RTX 5080 (can't store lighting).")
        );
    }

    #[test]
    fn note_is_empty_when_everything_went_through() {
        let msi = DeviceId::hid(0x1462, 0x7D75, None);
        let report = PersistReport {
            devices: vec![
                entry(&msi, PersistResult::Unchanged),
                entry(&DeviceId::hid(1, 1, None), skipped(SkipReason::NotDriven)),
            ],
        };
        assert_eq!(persist_note(&report, false, |id| id.to_string()), None);
        assert_eq!(
            persist_note(&PersistReport::default(), true, |id| id.to_string()),
            None
        );
    }

    #[test]
    fn restore_note_mentions_absent_devices_and_failures() {
        let msi = DeviceId::hid(0x1462, 0x7D75, None);
        let g560 = DeviceId::hid(0x046D, 0x0A78, None);
        let report = PersistReport {
            devices: vec![
                entry(&msi, PersistResult::Failed { error: "x".into() }),
                entry(&g560, skipped(SkipReason::Absent)),
            ],
        };
        assert_eq!(
            persist_note(&report, true, |id| id.to_string()).as_deref(),
            Some(
                "Not restored: hid:1462:7d75 (failed); hid:046d:0a78 (not connected; restored when connected)."
            )
        );
        // Absent is not news when saving (the device simply is not there).
        let report = PersistReport {
            devices: vec![entry(&g560, skipped(SkipReason::Absent))],
        };
        assert_eq!(persist_note(&report, false, |id| id.to_string()), None);
    }

    #[test]
    fn note_includes_faulted_and_unconfirmed_devices() {
        let msi = DeviceId::hid(0x1462, 0x7D75, None);
        let g560 = DeviceId::hid(0x046D, 0x0A78, None);
        let report = PersistReport {
            devices: vec![
                entry(&msi, skipped(SkipReason::Faulted)),
                entry(&g560, PersistResult::Unconfirmed),
            ],
        };
        assert_eq!(
            persist_note(&report, false, |id| id.to_string()).as_deref(),
            Some(
                "Not saved: hid:1462:7d75 (device error); hid:046d:0a78 (not confirmed in time; it may still finish)."
            )
        );
        let report = PersistReport {
            devices: vec![
                entry(&msi, skipped(SkipReason::Faulted)),
                entry(&g560, skipped(SkipReason::DailyCap)),
            ],
        };
        assert_eq!(
            persist_note(&report, true, |id| id.to_string()).as_deref(),
            Some(
                "Not restored: hid:1462:7d75 (device error; restored when connected again); hid:046d:0a78 (daily \
                 limit reached; tried again tomorrow)."
            )
        );
        for reason in [SkipReason::WritesDisabled, SkipReason::StateUnavailable] {
            let report = PersistReport {
                devices: vec![entry(&msi, skipped(reason))],
            };
            assert!(
                persist_note(&report, false, |id| id.to_string()).is_some(),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn exit_waits_for_a_running_switch_only_briefly_at_a_session_end() {
        for why in [
            SessionEnd::SignOut,
            SessionEnd::ShutdownOrRestart,
            SessionEnd::Unknown,
            SessionEnd::CloseApp,
        ] {
            let plan = exit_plan(why, true, true);
            assert!(plan.switch_wait <= Duration::from_secs(1), "{why:?}");
            assert!(
                plan.switch_wait + plan.persist_timeout + plan.stop_timeout <= Duration::from_millis(4500),
                "{why:?}"
            );
        }
        let quit = exit_plan(SessionEnd::NotEnding, true, true);
        assert!(
            quit.switch_wait >= SWITCH_PERSIST_TIMEOUT,
            "a quit waits for the switch's own budget"
        );
        let uninstall = ExitPlan::uninstall();
        assert_eq!(uninstall.mode, ShutdownMode::Release);
        assert!(!uninstall.persist);
    }
}
