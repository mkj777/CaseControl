//! Conflict actions the user starts with one click (ADR 0001 #31): close a program (terminate its processes, stop
//! its services) and turn its autostart off or back on.
//!
//! Actions are values of the fixed [`Action`] enum, built only from a [`Finding`] of a scan
//! ([`close_actions`], [`autostart_actions`]) and executed through the [`SystemOps`] trait, so tests use fakes and
//! the real Windows implementation (`WindowsOps`) is the only code that changes anything. Everything that needs
//! administrator rights is collected into one [`ElevatedRequest`] that CaseControl's own executable runs in helper
//! mode behind a single UAC prompt; the helper re-validates every action against a fresh scan with
//! [`run_elevated_request`] before doing anything.

use serde::{Deserialize, Serialize};

use crate::actions::{ActionError, is_valid_service_name};
use crate::autostart::{AutostartItem, AutostartLocation};
use crate::scan::{Evidence, Finding, ScanReport};
use crate::sources::{ServiceKind, ServiceState, StartType};

/// Version of the [`ElevatedRequest`] / [`ElevatedResponse`] format.
pub const ELEVATED_PROTOCOL_VERSION: u32 = 1;

/// Upper bound on actions in one elevated request; a longer request is refused as a whole.
pub const MAX_ELEVATED_ACTIONS: usize = 64;

/// One change to the system. The only things CaseControl ever does to other software.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Terminate process `pid` if its executable is still `name`.
    TerminateProcess {
        /// Process id from the scan.
        pid: u32,
        /// Executable file name from the scan; re-checked against the live process.
        name: String,
    },
    /// Stop a Win32 service.
    StopService {
        /// Service name.
        name: String,
    },
    /// Change a Win32 service's start type (`manual` to turn autostart off, the remembered type to turn it on).
    SetServiceStartType {
        /// Service name.
        name: String,
        /// New start type; only `auto` and `manual` are accepted.
        start_type: StartType,
    },
    /// Switch a Run / Startup folder entry through its `StartupApproved` value.
    SetStartupApproved {
        /// Where the entry lives (must have an approval key).
        location: AutostartLocation,
        /// Value name or Startup folder file name.
        name: String,
        /// On (`0x02`) or off (`0x03` + timestamp).
        enabled: bool,
    },
}

impl Action {
    /// Whether the action needs administrator rights. Process termination is tried without them first.
    pub fn needs_admin(&self) -> bool {
        match self {
            Action::TerminateProcess { .. } => false,
            Action::StopService { .. } | Action::SetServiceStartType { .. } => true,
            Action::SetStartupApproved { location, .. } => location.needs_admin(),
        }
    }

    /// Execution order inside a batch: autostart changes first (so nothing restarts what is closed), then services,
    /// then processes.
    fn phase(&self) -> u8 {
        match self {
            Action::SetServiceStartType { .. } => 0,
            Action::SetStartupApproved { .. } => 1,
            Action::StopService { .. } => 2,
            Action::TerminateProcess { .. } => 3,
        }
    }
}

/// An action and the catalogue entry it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedAction {
    /// Catalogue id of the finding.
    pub entry_id: String,
    /// What to do.
    pub action: Action,
}

/// Result of one action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionOutcome {
    /// Catalogue id of the finding.
    pub entry_id: String,
    /// What was done (or refused).
    pub action: Action,
    /// Whether it succeeded.
    pub ok: bool,
    /// Why it failed or was refused.
    pub error: Option<String>,
    /// A same-user termination failed for lack of rights; the caller may retry it in the elevated batch. Not part
    /// of the wire format.
    #[serde(skip)]
    pub retry_elevated: bool,
}

impl ActionOutcome {
    /// A success.
    pub fn success(planned: &PlannedAction) -> Self {
        Self {
            entry_id: planned.entry_id.clone(),
            action: planned.action.clone(),
            ok: true,
            error: None,
            retry_elevated: false,
        }
    }

    /// A failure with `error`.
    pub fn failure(planned: &PlannedAction, error: impl Into<String>) -> Self {
        Self {
            entry_id: planned.entry_id.clone(),
            action: planned.action.clone(),
            ok: false,
            error: Some(error.into()),
            retry_elevated: false,
        }
    }
}

/// The system calls behind the actions. `WindowsOps` is the real implementation; tests use fakes.
pub trait SystemOps {
    /// Terminates `pid` after re-checking its executable name (see [`crate::terminate_process`]).
    fn terminate_process(&self, pid: u32, name: &str, reason: &str) -> Result<(), ActionError>;
    /// Stops service `name` and waits briefly for it to stop.
    fn stop_service(&self, name: &str) -> Result<(), String>;
    /// Sets the start type of service `name`.
    fn set_service_start_type(&self, name: &str, start_type: StartType) -> Result<(), String>;
    /// Writes the `StartupApproved` value of entry `name` at `location`.
    fn set_startup_approved(
        &self,
        location: AutostartLocation,
        name: &str,
        enabled: bool,
    ) -> Result<(), String>;
}

/// What "Close now" does for `finding`: terminate each matched process, stop each running Win32 service. Drivers
/// are never touched.
pub fn close_actions(finding: &Finding) -> Vec<Action> {
    finding
        .evidence
        .iter()
        .filter_map(|e| match e {
            Evidence::Process { pid, name, .. } => Some(Action::TerminateProcess {
                pid: *pid,
                name: name.clone(),
            }),
            Evidence::Service {
                name,
                state: ServiceState::Running,
                kind: ServiceKind::Win32,
            } => Some(Action::StopService { name: name.clone() }),
            _ => None,
        })
        .collect()
}

/// What "Turn off autostart" (`enabled = false`) or "Re-enable" (`true`) does for `finding`.
///
/// Off: every enabled, toggleable item: app entries get `StartupApproved` off, services start type Manual.
/// On: every disabled app entry gets `StartupApproved` on; a Manual service is set back to the start type
/// `remembered` returns for it (the one it had before CaseControl changed it) and is left alone when there is none,
/// so services the user set to Manual themselves are not touched.
pub fn autostart_actions(
    finding: &Finding,
    enabled: bool,
    remembered: impl Fn(&str) -> Option<StartType>,
) -> Vec<Action> {
    finding
        .autostart
        .iter()
        .filter(|item| item.toggleable && item.enabled != enabled)
        .filter_map(|item| match item.location {
            AutostartLocation::Service if enabled => remembered(&item.name)
                .filter(|st| *st == StartType::Auto)
                .map(|start_type| Action::SetServiceStartType {
                    name: item.name.clone(),
                    start_type,
                }),
            AutostartLocation::Service => Some(Action::SetServiceStartType {
                name: item.name.clone(),
                start_type: StartType::Manual,
            }),
            location => Some(Action::SetStartupApproved {
                location,
                name: item.name.clone(),
                enabled,
            }),
        })
        .collect()
}

/// Checks that `planned` targets something `report` (a fresh scan) attributes to its catalogue entry. This is the
/// whole authorisation model: an action on anything a scan does not tie to the catalogue is refused.
pub fn check_action(planned: &PlannedAction, report: &ScanReport) -> Result<(), String> {
    let finding = report
        .findings
        .iter()
        .find(|f| f.entry_id == planned.entry_id)
        .ok_or_else(|| format!("{:?} was not found by a fresh scan", planned.entry_id))?;
    let autostart_item = |location: AutostartLocation, name: &str| -> Option<&AutostartItem> {
        finding
            .autostart
            .iter()
            .find(|i| i.location == location && i.toggleable && i.name.eq_ignore_ascii_case(name))
    };
    match &planned.action {
        Action::TerminateProcess { pid, name } => {
            let found = finding.evidence.iter().any(
                |e| matches!(e, Evidence::Process { pid: p, name: n, .. } if p == pid && n.eq_ignore_ascii_case(name)),
            );
            if !found {
                return Err(format!(
                    "process {name} ({pid}) is not part of {:?}",
                    planned.entry_id
                ));
            }
        }
        Action::StopService { name } => {
            if !is_valid_service_name(name) {
                return Err(format!("invalid service name {name:?}"));
            }
            let found = finding.evidence.iter().any(|e| {
                matches!(e, Evidence::Service { name: n, kind: ServiceKind::Win32, .. } if n.eq_ignore_ascii_case(name))
            });
            if !found {
                return Err(format!("service {name:?} is not part of {:?}", planned.entry_id));
            }
        }
        Action::SetServiceStartType { name, start_type } => {
            if !matches!(start_type, StartType::Auto | StartType::Manual) {
                return Err(format!("start type {start_type:?} is not allowed"));
            }
            if !is_valid_service_name(name) {
                return Err(format!("invalid service name {name:?}"));
            }
            if autostart_item(AutostartLocation::Service, name).is_none() {
                return Err(format!("service {name:?} is not part of {:?}", planned.entry_id));
            }
        }
        Action::SetStartupApproved { location, name, .. } => {
            if location.approval_key().is_none() {
                return Err(format!("{location:?} entries cannot be switched"));
            }
            if name.is_empty() || name.chars().any(char::is_control) {
                return Err(format!("invalid autostart entry name {name:?}"));
            }
            if autostart_item(*location, name).is_none() {
                return Err(format!(
                    "autostart entry {name:?} is not part of {:?}",
                    planned.entry_id
                ));
            }
        }
    }
    Ok(())
}

/// Runs `actions` through `ops`, autostart changes first, then service stops, then terminations. Returns one
/// outcome per action in the order given. A process that no longer exists counts as closed.
pub fn execute(actions: &[PlannedAction], ops: &dyn SystemOps) -> Vec<ActionOutcome> {
    let mut order: Vec<usize> = (0..actions.len()).collect();
    order.sort_by_key(|&i| actions[i].action.phase());
    let mut outcomes: Vec<Option<ActionOutcome>> = vec![None; actions.len()];
    for i in order {
        let planned = &actions[i];
        let reason = planned.entry_id.as_str();
        tracing::warn!(entry = reason, action = ?planned.action, "running conflict action on user request");
        let outcome = match &planned.action {
            Action::TerminateProcess { pid, name } => match ops.terminate_process(*pid, name, reason) {
                Ok(()) | Err(ActionError::NotFound { .. }) => ActionOutcome::success(planned),
                Err(err @ ActionError::Failed(_)) => ActionOutcome {
                    retry_elevated: true,
                    ..ActionOutcome::failure(planned, err.to_string())
                },
                Err(err) => ActionOutcome::failure(planned, err.to_string()),
            },
            Action::StopService { name } => to_outcome(planned, ops.stop_service(name)),
            Action::SetServiceStartType { name, start_type } => {
                to_outcome(planned, ops.set_service_start_type(name, *start_type))
            }
            Action::SetStartupApproved {
                location,
                name,
                enabled,
            } => to_outcome(planned, ops.set_startup_approved(*location, name, *enabled)),
        };
        if let Some(err) = &outcome.error {
            tracing::warn!(entry = reason, action = ?planned.action, error = %err, "conflict action failed");
        }
        outcomes[i] = Some(outcome);
    }
    outcomes.into_iter().flatten().collect()
}

fn to_outcome(planned: &PlannedAction, result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::success(planned),
        Err(err) => ActionOutcome::failure(planned, err),
    }
}

/// What the app hands the elevated helper (serialized as JSON into a request file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElevatedRequest {
    /// Must be [`ELEVATED_PROTOCOL_VERSION`].
    pub version: u32,
    /// The actions, at most [`MAX_ELEVATED_ACTIONS`].
    pub actions: Vec<PlannedAction>,
}

impl ElevatedRequest {
    /// A request of the current version.
    pub fn new(actions: Vec<PlannedAction>) -> Self {
        Self {
            version: ELEVATED_PROTOCOL_VERSION,
            actions,
        }
    }
}

/// What the elevated helper writes back: one outcome per requested action, in request order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElevatedResponse {
    /// [`ELEVATED_PROTOCOL_VERSION`].
    pub version: u32,
    /// One per requested action.
    pub outcomes: Vec<ActionOutcome>,
}

/// Validates an elevated request against a fresh scan and runs what passes. Used by the helper process.
///
/// Rules: the version must match and the request may hold at most [`MAX_ELEVATED_ACTIONS`] actions (otherwise
/// every action is refused); each action must be one that needs administrator rights or a process termination
/// (HKCU `StartupApproved` writes are refused: in the elevated process HKCU may be another user's hive); and each
/// action must pass [`check_action`] against `report`. Refused actions are reported, never run.
pub fn run_elevated_request(
    request: &ElevatedRequest,
    report: &ScanReport,
    ops: &dyn SystemOps,
) -> ElevatedResponse {
    let refuse_all = if request.version != ELEVATED_PROTOCOL_VERSION {
        Some(format!(
            "request version {} is not supported (expected {ELEVATED_PROTOCOL_VERSION})",
            request.version
        ))
    } else if request.actions.len() > MAX_ELEVATED_ACTIONS {
        Some(format!(
            "request holds {} actions, more than {MAX_ELEVATED_ACTIONS}",
            request.actions.len()
        ))
    } else {
        None
    };
    if let Some(reason) = refuse_all {
        tracing::warn!(%reason, "elevated request refused");
        return ElevatedResponse {
            version: ELEVATED_PROTOCOL_VERSION,
            outcomes: request
                .actions
                .iter()
                .take(MAX_ELEVATED_ACTIONS)
                .map(|a| ActionOutcome::failure(a, reason.clone()))
                .collect(),
        };
    }

    let mut accepted = Vec::new();
    let mut slots: Vec<Result<usize, ActionOutcome>> = Vec::with_capacity(request.actions.len());
    for planned in &request.actions {
        let elevated_kind =
            planned.action.needs_admin() || matches!(planned.action, Action::TerminateProcess { .. });
        let check = if elevated_kind {
            check_action(planned, report)
        } else {
            Err("this action runs without administrator rights and is refused here".to_owned())
        };
        match check {
            Ok(()) => {
                slots.push(Ok(accepted.len()));
                accepted.push(planned.clone());
            }
            Err(err) => {
                tracing::warn!(entry = %planned.entry_id, action = ?planned.action, error = %err, "elevated action refused");
                slots.push(Err(ActionOutcome::failure(planned, err)));
            }
        }
    }
    let mut ran = execute(&accepted, ops).into_iter().map(Some).collect::<Vec<_>>();
    let outcomes = slots
        .into_iter()
        .map(|slot| match slot {
            Ok(i) => ran[i]
                .take()
                .unwrap_or_else(|| ActionOutcome::failure(&accepted[i], "not run")),
            Err(outcome) => outcome,
        })
        .collect();
    ElevatedResponse {
        version: ELEVATED_PROTOCOL_VERSION,
        outcomes,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::catalog::{Access, Category, Class, Severity};

    /// Records every call; answers from a script.
    #[derive(Default)]
    pub(crate) struct FakeOps {
        pub calls: RefCell<Vec<String>>,
        pub deny_terminate: bool,
        pub missing_pid: Option<u32>,
        pub fail_service: Option<String>,
    }

    impl SystemOps for FakeOps {
        fn terminate_process(&self, pid: u32, name: &str, _reason: &str) -> Result<(), ActionError> {
            self.calls.borrow_mut().push(format!("terminate {pid} {name}"));
            if self.missing_pid == Some(pid) {
                return Err(ActionError::NotFound { pid });
            }
            if self.deny_terminate {
                return Err(ActionError::Failed(
                    "OpenProcess failed: Access is denied.".into(),
                ));
            }
            Ok(())
        }
        fn stop_service(&self, name: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("stop {name}"));
            match &self.fail_service {
                Some(f) if f == name => Err("access denied".into()),
                _ => Ok(()),
            }
        }
        fn set_service_start_type(&self, name: &str, start_type: StartType) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("start_type {name} {start_type:?}"));
            Ok(())
        }
        fn set_startup_approved(
            &self,
            location: AutostartLocation,
            name: &str,
            enabled: bool,
        ) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("approved {location:?} {name} {enabled}"));
            Ok(())
        }
    }

    /// A SignalRGB finding as on the development machine, plus an HKLM entry and a running service.
    pub(crate) fn signalrgb() -> Finding {
        Finding {
            entry_id: "signalrgb".into(),
            name: "SignalRGB".into(),
            vendor: "WhirlwindFX".into(),
            category: Category::RgbSuite,
            class: Class::Suite,
            severity: Severity::High,
            verified: true,
            access: vec![Access::Hid],
            affects: vec!["wooting".into()],
            note: String::new(),
            running: true,
            installed: true,
            evidence: vec![
                Evidence::Process {
                    pid: 100,
                    name: "SignalRgb.exe".into(),
                    path: None,
                },
                Evidence::Service {
                    name: "SignalRgb.Service".into(),
                    state: ServiceState::Running,
                    kind: ServiceKind::Win32,
                },
                Evidence::Service {
                    name: "SignalRgb.Updater".into(),
                    state: ServiceState::Stopped,
                    kind: ServiceKind::Win32,
                },
                Evidence::Driver {
                    name: "SignalRgbDriver".into(),
                    state: ServiceState::Running,
                    path: None,
                },
            ],
            autostart: vec![
                AutostartItem {
                    location: AutostartLocation::HkcuRun,
                    name: "SignalRgb".into(),
                    command: Some("\"x\\SignalRgbLauncher.exe\" --silent".into()),
                    enabled: true,
                    start_type: None,
                    toggleable: true,
                    needs_admin: false,
                },
                AutostartItem {
                    location: AutostartLocation::HklmRun,
                    name: "SignalRgbAll".into(),
                    command: Some("x".into()),
                    enabled: false,
                    start_type: None,
                    toggleable: true,
                    needs_admin: true,
                },
                AutostartItem {
                    location: AutostartLocation::HklmRunOnce,
                    name: "SignalRgbSetup".into(),
                    command: Some("x".into()),
                    enabled: true,
                    start_type: None,
                    toggleable: false,
                    needs_admin: true,
                },
                AutostartItem::service("SignalRgb.Service", Some(StartType::Auto)),
                AutostartItem::service("SignalRgb.Updater", Some(StartType::Manual)),
            ],
        }
    }

    fn report() -> ScanReport {
        ScanReport {
            findings: vec![signalrgb()],
            errors: vec![],
        }
    }

    fn planned(action: Action) -> PlannedAction {
        PlannedAction {
            entry_id: "signalrgb".into(),
            action,
        }
    }

    #[test]
    fn close_targets_processes_and_running_services_only() {
        assert_eq!(
            close_actions(&signalrgb()),
            [
                Action::TerminateProcess {
                    pid: 100,
                    name: "SignalRgb.exe".into()
                },
                Action::StopService {
                    name: "SignalRgb.Service".into()
                },
            ]
        );
    }

    #[test]
    fn turning_autostart_off_and_on() {
        let off = autostart_actions(&signalrgb(), false, |_| None);
        assert_eq!(
            off,
            [
                Action::SetStartupApproved {
                    location: AutostartLocation::HkcuRun,
                    name: "SignalRgb".into(),
                    enabled: false
                },
                Action::SetServiceStartType {
                    name: "SignalRgb.Service".into(),
                    start_type: StartType::Manual
                },
            ]
        );
        // On: the disabled HKLM entry; the Manual updater only when a start type was remembered for it.
        let on = autostart_actions(&signalrgb(), true, |_| None);
        assert_eq!(
            on,
            [Action::SetStartupApproved {
                location: AutostartLocation::HklmRun,
                name: "SignalRgbAll".into(),
                enabled: true
            }]
        );
        let on = autostart_actions(&signalrgb(), true, |name| {
            (name == "SignalRgb.Updater").then_some(StartType::Auto)
        });
        assert!(on.contains(&Action::SetServiceStartType {
            name: "SignalRgb.Updater".into(),
            start_type: StartType::Auto
        }));
        // A remembered type other than Auto is never restored.
        let on = autostart_actions(&signalrgb(), true, |_| Some(StartType::Disabled));
        assert_eq!(on.len(), 1);
    }

    #[test]
    fn admin_classification() {
        assert!(
            !Action::TerminateProcess {
                pid: 1,
                name: "a".into()
            }
            .needs_admin()
        );
        assert!(Action::StopService { name: "a".into() }.needs_admin());
        assert!(
            Action::SetServiceStartType {
                name: "a".into(),
                start_type: StartType::Manual
            }
            .needs_admin()
        );
        let approved = |location| Action::SetStartupApproved {
            location,
            name: "a".into(),
            enabled: false,
        };
        assert!(!approved(AutostartLocation::HkcuRun).needs_admin());
        assert!(!approved(AutostartLocation::UserStartupFolder).needs_admin());
        assert!(approved(AutostartLocation::HklmRun32).needs_admin());
        assert!(approved(AutostartLocation::CommonStartupFolder).needs_admin());
    }

    #[test]
    fn execute_orders_phases_and_keeps_request_order_in_outcomes() {
        let ops = FakeOps {
            missing_pid: Some(100),
            ..FakeOps::default()
        };
        let actions = vec![
            planned(Action::TerminateProcess {
                pid: 100,
                name: "SignalRgb.exe".into(),
            }),
            planned(Action::StopService {
                name: "SignalRgb.Service".into(),
            }),
            planned(Action::SetStartupApproved {
                location: AutostartLocation::HkcuRun,
                name: "SignalRgb".into(),
                enabled: false,
            }),
            planned(Action::SetServiceStartType {
                name: "SignalRgb.Service".into(),
                start_type: StartType::Manual,
            }),
        ];
        let outcomes = execute(&actions, &ops);
        assert_eq!(
            *ops.calls.borrow(),
            [
                "start_type SignalRgb.Service Manual",
                "approved HkcuRun SignalRgb false",
                "stop SignalRgb.Service",
                "terminate 100 SignalRgb.exe",
            ]
        );
        assert_eq!(outcomes.len(), 4);
        for (o, a) in outcomes.iter().zip(&actions) {
            assert_eq!(o.action, a.action);
            assert!(o.ok, "{o:?}");
        }
    }

    #[test]
    fn failed_termination_is_marked_for_elevated_retry() {
        let ops = FakeOps {
            deny_terminate: true,
            fail_service: Some("SignalRgb.Service".into()),
            ..FakeOps::default()
        };
        let outcomes = execute(
            &[
                planned(Action::TerminateProcess {
                    pid: 100,
                    name: "SignalRgb.exe".into(),
                }),
                planned(Action::StopService {
                    name: "SignalRgb.Service".into(),
                }),
            ],
            &ops,
        );
        assert!(!outcomes[0].ok && outcomes[0].retry_elevated);
        assert!(!outcomes[1].ok && !outcomes[1].retry_elevated);
        assert_eq!(outcomes[1].error.as_deref(), Some("access denied"));
    }

    #[test]
    fn check_accepts_only_what_the_scan_attributes() {
        let r = report();
        let ok = [
            Action::TerminateProcess {
                pid: 100,
                name: "signalrgb.EXE".into(),
            },
            Action::StopService {
                name: "signalrgb.service".into(),
            },
            Action::SetServiceStartType {
                name: "SignalRgb.Service".into(),
                start_type: StartType::Manual,
            },
            Action::SetStartupApproved {
                location: AutostartLocation::HklmRun,
                name: "SignalRgbAll".into(),
                enabled: true,
            },
        ];
        for a in ok {
            assert_eq!(check_action(&planned(a.clone()), &r), Ok(()), "{a:?}");
        }
        let refused = [
            // Pid reuse / another process.
            Action::TerminateProcess {
                pid: 101,
                name: "SignalRgb.exe".into(),
            },
            Action::TerminateProcess {
                pid: 100,
                name: "explorer.exe".into(),
            },
            // Not in the finding, a driver, or not a valid name.
            Action::StopService {
                name: "Spooler".into(),
            },
            Action::StopService {
                name: "SignalRgbDriver".into(),
            },
            Action::StopService {
                name: "SignalRgb.Service\" & x".into(),
            },
            // Start types other than auto/manual, unknown services.
            Action::SetServiceStartType {
                name: "SignalRgb.Service".into(),
                start_type: StartType::Disabled,
            },
            Action::SetServiceStartType {
                name: "Spooler".into(),
                start_type: StartType::Manual,
            },
            // Wrong location, RunOnce, unknown entry.
            Action::SetStartupApproved {
                location: AutostartLocation::HkcuRun,
                name: "SignalRgbAll".into(),
                enabled: false,
            },
            Action::SetStartupApproved {
                location: AutostartLocation::HklmRunOnce,
                name: "SignalRgbSetup".into(),
                enabled: false,
            },
            Action::SetStartupApproved {
                location: AutostartLocation::HkcuRun,
                name: "Discord".into(),
                enabled: false,
            },
        ];
        for a in refused {
            assert!(check_action(&planned(a.clone()), &r).is_err(), "{a:?}");
        }
        let other_entry = PlannedAction {
            entry_id: "openrgb".into(),
            action: Action::StopService {
                name: "SignalRgb.Service".into(),
            },
        };
        assert!(check_action(&other_entry, &r).is_err());
    }

    #[test]
    fn elevated_request_runs_only_validated_admin_actions() {
        let ops = FakeOps::default();
        let request = ElevatedRequest::new(vec![
            planned(Action::StopService {
                name: "SignalRgb.Service".into(),
            }),
            // HKCU write: refused in the elevated helper.
            planned(Action::SetStartupApproved {
                location: AutostartLocation::HkcuRun,
                name: "SignalRgb".into(),
                enabled: false,
            }),
            planned(Action::StopService {
                name: "Spooler".into(),
            }),
            planned(Action::TerminateProcess {
                pid: 100,
                name: "SignalRgb.exe".into(),
            }),
            planned(Action::SetStartupApproved {
                location: AutostartLocation::HklmRun,
                name: "SignalRgbAll".into(),
                enabled: true,
            }),
        ]);
        let response = run_elevated_request(&request, &report(), &ops);
        assert_eq!(response.version, ELEVATED_PROTOCOL_VERSION);
        let ok: Vec<bool> = response.outcomes.iter().map(|o| o.ok).collect();
        assert_eq!(ok, [true, false, false, true, true]);
        for (o, a) in response.outcomes.iter().zip(&request.actions) {
            assert_eq!(o.action, a.action);
        }
        assert_eq!(
            *ops.calls.borrow(),
            [
                "approved HklmRun SignalRgbAll true",
                "stop SignalRgb.Service",
                "terminate 100 SignalRgb.exe",
            ]
        );
    }

    #[test]
    fn elevated_request_with_wrong_version_or_too_many_actions_runs_nothing() {
        let ops = FakeOps::default();
        let action = planned(Action::StopService {
            name: "SignalRgb.Service".into(),
        });
        let mut request = ElevatedRequest::new(vec![action.clone()]);
        request.version = 99;
        let response = run_elevated_request(&request, &report(), &ops);
        assert!(response.outcomes.iter().all(|o| !o.ok));
        let request = ElevatedRequest::new(vec![action; MAX_ELEVATED_ACTIONS + 1]);
        let response = run_elevated_request(&request, &report(), &ops);
        assert!(response.outcomes.iter().all(|o| !o.ok));
        assert!(ops.calls.borrow().is_empty());
    }

    #[test]
    fn elevated_request_against_an_empty_scan_runs_nothing() {
        let ops = FakeOps::default();
        let request = ElevatedRequest::new(vec![planned(Action::StopService {
            name: "SignalRgb.Service".into(),
        })]);
        let response = run_elevated_request(&request, &ScanReport::default(), &ops);
        assert!(!response.outcomes[0].ok);
        assert!(ops.calls.borrow().is_empty());
    }

    #[test]
    fn actions_serialize_with_a_kind_tag() {
        let value = toml::Value::try_from(planned(Action::SetStartupApproved {
            location: AutostartLocation::HklmRun32,
            name: "x".into(),
            enabled: false,
        }))
        .expect("serializes");
        let action = &value["action"];
        assert_eq!(action["kind"].as_str(), Some("set_startup_approved"));
        assert_eq!(action["location"].as_str(), Some("hklm_run32"));
    }
}
