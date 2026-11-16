//! Running conflict scans and conflict actions for the commands and the start-up check (ADR 0001 #31).
//!
//! Every action is planned from a fresh scan, re-checked against it with [`cc_conflicts::check_action`], and then
//! split: same-user work (process termination, HKCU `StartupApproved`) runs in this process; everything that needs
//! administrator rights, plus terminations that failed for lack of rights, runs in one elevated helper invocation
//! (one UAC prompt). Nothing here runs without a user click.

use std::collections::BTreeSet;

use cc_conflicts::{
    ActionOutcome, PlannedAction, ScanOptions, ScanReport, StartType, SystemOps, check_action, execute,
};
use serde::Serialize;
use tauri::{AppHandle, Runtime};

use crate::conflicts::{ConflictScan, KnownDevice, ViewContext, scan_view};
use crate::events::{self, CONFLICTS_UPDATED};
use crate::hidden::is_device_hidden;
use crate::startup::{ActionResult, forget_restored, remember_start_types};
use crate::state::AppState;

/// A program the request named that could not be turned into actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusedItem {
    /// Catalogue id.
    pub entry_id: String,
    /// Why.
    pub reason: String,
}

/// What `resolve_conflicts` and `set_autostart` return.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionReport {
    /// One result per action, in plan order.
    pub results: Vec<ActionResult>,
    /// Requested programs that were refused before any action.
    pub refused: Vec<RefusedItem>,
    /// The scan after the actions (also emitted as `conflicts-updated`).
    pub scan: ConflictScan,
}

/// All controlled devices and the ones the user did not hide.
pub fn known_devices(state: &AppState) -> (Vec<KnownDevice>, Vec<KnownDevice>) {
    let all: Vec<KnownDevice> = state
        .engine()
        .map(|e| e.devices().iter().map(KnownDevice::from).collect())
        .unwrap_or_default();
    let settings = state.settings();
    let visible = all
        .iter()
        .filter(|d| !is_device_hidden(&settings, &d.id))
        .cloned()
        .collect();
    (all, visible)
}

/// The frontend view of `report` with the current devices and settings.
pub fn conflict_view(state: &AppState, report: &ScanReport) -> ConflictScan {
    let (devices, visible) = known_devices(state);
    let settings = state.settings().clone();
    scan_view(
        report,
        &ViewContext {
            devices: &devices,
            visible: &visible,
            dont_ask: &settings.conflicts.dont_ask,
            remembered: &settings.conflicts.previous_start_types,
        },
    )
}

/// Runs a conflict scan (processes, services, registry, autostart). Slow-ish; call off the main thread.
pub fn run_scan(state: &AppState) -> Result<ScanReport, String> {
    let opts = ScanOptions {
        check_openrgb_port: state.settings().conflicts.check_openrgb_port,
    };
    platform_scan(&opts)
}

#[cfg(windows)]
fn platform_scan(opts: &ScanOptions) -> Result<ScanReport, String> {
    Ok(cc_conflicts::Scanner::windows().scan(opts))
}

#[cfg(not(windows))]
fn platform_scan(_opts: &ScanOptions) -> Result<ScanReport, String> {
    Err("conflict scanning is only available on Windows".to_owned())
}

/// Stores `report` as the last scan, emits `conflicts-updated` with its view and returns the view.
pub fn publish<R: Runtime>(app: &AppHandle<R>, state: &AppState, report: ScanReport) -> ConflictScan {
    let view = conflict_view(state, &report);
    state.set_last_scan(report);
    events::emit(app, CONFLICTS_UPDATED, view.clone());
    view
}

/// Scans, stores, emits `conflicts-updated` and returns the view.
pub fn scan_and_publish<R: Runtime>(app: &AppHandle<R>, state: &AppState) -> Result<ConflictScan, String> {
    let report = run_scan(state)?;
    Ok(publish(app, state, report))
}

#[cfg(windows)]
fn local_ops() -> &'static dyn SystemOps {
    &cc_conflicts::WindowsOps
}

#[cfg(not(windows))]
fn local_ops() -> &'static dyn SystemOps {
    struct Unsupported;
    impl SystemOps for Unsupported {
        fn terminate_process(&self, _: u32, _: &str, _: &str) -> Result<(), cc_conflicts::ActionError> {
            Err(cc_conflicts::ActionError::Unsupported)
        }
        fn stop_service(&self, _: &str) -> Result<(), String> {
            Err("not supported on this platform".into())
        }
        fn set_service_start_type(&self, _: &str, _: StartType) -> Result<(), String> {
            Err("not supported on this platform".into())
        }
        fn set_startup_approved(
            &self,
            _: cc_conflicts::AutostartLocation,
            _: &str,
            _: bool,
        ) -> Result<(), String> {
            Err("not supported on this platform".into())
        }
    }
    &Unsupported
}

/// How a batch was split.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split {
    /// Indices into the plan that run in this process.
    pub local: Vec<usize>,
    /// Indices that run in the elevated helper.
    pub admin: Vec<usize>,
}

/// Splits `actions` (indices `runnable`) into in-process and elevated work.
pub fn split(actions: &[PlannedAction], runnable: &[usize]) -> Split {
    let mut out = Split::default();
    for &i in runnable {
        if actions[i].action.needs_admin() {
            out.admin.push(i);
        } else {
            out.local.push(i);
        }
    }
    out
}

/// Runs a plan built from `report` (a fresh scan): checks each action against the scan, remembers the start types
/// of services about to be set to Manual (an action whose memory cannot be saved is not run), runs same-user work
/// in-process, everything else plus rights-denied terminations in one elevated helper run, and forgets the memory
/// of services that were restored. Returns one outcome per action, in plan order.
pub fn run_actions(state: &AppState, report: &ScanReport, actions: &[PlannedAction]) -> Vec<ActionOutcome> {
    let mut outcomes: Vec<Option<ActionOutcome>> = vec![None; actions.len()];
    let mut runnable = Vec::new();
    for (i, planned) in actions.iter().enumerate() {
        match check_action(planned, report) {
            Ok(()) => runnable.push(i),
            Err(err) => outcomes[i] = Some(ActionOutcome::failure(planned, err)),
        }
    }

    // Remember previous start types before changing any (so Re-enable can restore exactly them).
    let turning_off: Vec<usize> = runnable
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                actions[i].action,
                cc_conflicts::Action::SetServiceStartType {
                    start_type: StartType::Manual,
                    ..
                }
            )
        })
        .collect();
    if !turning_off.is_empty() {
        let planned: Vec<PlannedAction> = turning_off.iter().map(|&i| actions[i].clone()).collect();
        let needs_save = {
            let mut copy = state.settings().conflicts.previous_start_types.clone();
            remember_start_types(&mut copy, &planned, report)
        };
        if needs_save {
            let saved = state.update_settings(|s| {
                remember_start_types(&mut s.conflicts.previous_start_types, &planned, report);
                Ok(())
            });
            if let Err(err) = saved {
                for &i in &turning_off {
                    outcomes[i] = Some(ActionOutcome::failure(
                        &actions[i],
                        format!(
                            "not changed, because the previous start type could not be remembered: {err}"
                        ),
                    ));
                }
                runnable.retain(|i| !turning_off.contains(i));
            }
        }
    }

    let Split { local, mut admin } = split(actions, &runnable);
    let local_actions: Vec<PlannedAction> = local.iter().map(|&i| actions[i].clone()).collect();
    for (&i, outcome) in local.iter().zip(execute(&local_actions, local_ops())) {
        if outcome.retry_elevated {
            admin.push(i);
        } else {
            outcomes[i] = Some(outcome);
        }
    }
    admin.sort_unstable();
    let admin_actions: Vec<PlannedAction> = admin.iter().map(|&i| actions[i].clone()).collect();
    for (&i, outcome) in admin.iter().zip(crate::elevated::run_admin_batch(&admin_actions)) {
        outcomes[i] = Some(outcome);
    }

    let outcomes: Vec<ActionOutcome> = outcomes
        .into_iter()
        .zip(actions)
        .map(|(o, a)| o.unwrap_or_else(|| ActionOutcome::failure(a, "not run")))
        .collect();

    let restored = {
        let mut copy = state.settings().conflicts.previous_start_types.clone();
        forget_restored(&mut copy, &outcomes)
    };
    if restored
        && let Err(err) = state.update_settings(|s| {
            forget_restored(&mut s.conflicts.previous_start_types, &outcomes);
            Ok(())
        })
    {
        tracing::warn!(%err, "cannot forget restored service start types");
    }
    outcomes
}

/// Runs `actions`, then rescans and publishes, and builds the report for the frontend.
pub fn run_and_report<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    report: &ScanReport,
    actions: &[PlannedAction],
    refused: Vec<(String, String)>,
) -> Result<ActionReport, String> {
    let outcomes = run_actions(state, report, actions);
    let scan = scan_and_publish(app, state)?;
    Ok(ActionReport {
        results: outcomes.into_iter().map(ActionResult::from).collect(),
        refused: refused
            .into_iter()
            .map(|(entry_id, reason)| RefusedItem { entry_id, reason })
            .collect(),
        scan,
    })
}

/// Validates a catalogue id for `set_dont_ask`.
pub fn check_catalogue_id(id: &str) -> Result<(), String> {
    if cc_conflicts::Catalog::embedded().get(id).is_some() {
        Ok(())
    } else {
        Err(format!("unknown program {id:?}"))
    }
}

/// Adds `id` to or removes it from `dont_ask`; returns whether it changed.
pub fn set_dont_ask_in(dont_ask: &mut BTreeSet<String>, id: &str, on: bool) -> bool {
    if on {
        dont_ask.insert(id.to_owned())
    } else {
        dont_ask.remove(id)
    }
}

#[cfg(test)]
mod tests {
    use cc_conflicts::{Action, AutostartLocation};

    use super::*;

    fn p(action: Action) -> PlannedAction {
        PlannedAction {
            entry_id: "signalrgb".into(),
            action,
        }
    }

    #[test]
    fn split_keeps_hkcu_and_terminations_local() {
        let actions = vec![
            p(Action::TerminateProcess {
                pid: 1,
                name: "a.exe".into(),
            }),
            p(Action::SetStartupApproved {
                location: AutostartLocation::HkcuRun,
                name: "a".into(),
                enabled: false,
            }),
            p(Action::SetStartupApproved {
                location: AutostartLocation::HklmRun,
                name: "a".into(),
                enabled: false,
            }),
            p(Action::StopService { name: "a".into() }),
            p(Action::SetServiceStartType {
                name: "a".into(),
                start_type: StartType::Manual,
            }),
        ];
        assert_eq!(
            split(&actions, &[0, 1, 2, 3, 4]),
            Split {
                local: vec![0, 1],
                admin: vec![2, 3, 4]
            }
        );
        assert_eq!(
            split(&actions, &[1, 3]),
            Split {
                local: vec![1],
                admin: vec![3]
            }
        );
    }

    #[test]
    fn dont_ask_set_changes() {
        let mut set = BTreeSet::new();
        assert!(set_dont_ask_in(&mut set, "openrgb", true));
        assert!(!set_dont_ask_in(&mut set, "openrgb", true));
        assert!(set_dont_ask_in(&mut set, "openrgb", false));
        assert!(!set_dont_ask_in(&mut set, "openrgb", false));
        assert!(check_catalogue_id("openrgb").is_ok());
        assert!(check_catalogue_id("../x").is_err());
    }
}
