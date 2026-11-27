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
use crate::boot_task::{BOOT_TASK_ENTRY, HelperContext};
use crate::scan::{Evidence, Finding, ScanReport};
use crate::sources::{ServiceKind, ServiceState, StartType};

/// Version of the [`ElevatedRequest`] / [`ElevatedResponse`] format.
pub const ELEVATED_PROTOCOL_VERSION: u32 = 1;

/// Upper bound on actions in one elevated request; a longer request is refused as a whole.
pub const MAX_ELEVATED_ACTIONS: usize = 64;

/// One change to the system. The only things CaseControl ever does to other software, plus CaseControl's own
/// start-up task (ADR 0001 #42). Unknown kinds and unknown fields are refused when parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
    /// Register CaseControl's start-up task for the signed-in user (elevated helper only, entry
    /// [`BOOT_TASK_ENTRY`]). Carries no values: the helper derives the user, executable and arguments itself
    /// (see [`crate::boot_task`]).
    RegisterBootTask {},
    /// Remove the signed-in user's start-up task if it is CaseControl's (elevated helper only, entry
    /// [`BOOT_TASK_ENTRY`]).
    RemoveBootTask {},
}

impl Action {
    /// Whether the action needs administrator rights. Process termination is tried without them first.
    pub fn needs_admin(&self) -> bool {
        match self {
            Action::TerminateProcess { .. } => false,
            Action::StopService { .. } | Action::SetServiceStartType { .. } => true,
            Action::SetStartupApproved { location, .. } => location.needs_admin(),
            Action::RegisterBootTask {} | Action::RemoveBootTask {} => true,
        }
    }

    /// Whether this is one of the start-up task actions, which belong to [`BOOT_TASK_ENTRY`] only.
    pub fn is_boot_task(&self) -> bool {
        matches!(self, Action::RegisterBootTask {} | Action::RemoveBootTask {})
    }

    /// Execution order inside a batch: autostart changes first (so nothing restarts what is closed), then services,
    /// then processes.
    fn phase(&self) -> u8 {
        match self {
            Action::SetServiceStartType { .. } => 0,
            Action::SetStartupApproved { .. } => 1,
            Action::StopService { .. } => 2,
            Action::TerminateProcess { .. } => 3,
            Action::RegisterBootTask {} | Action::RemoveBootTask {} => 4,
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
    /// The result is not known yet (the elevated helper was still running at the timeout). `ok` is `false`; a
    /// rescan shows the real state.
    #[serde(default)]
    pub unknown: bool,
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
            unknown: false,
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
            unknown: false,
            retry_elevated: false,
        }
    }

    /// Not known yet (see [`ActionOutcome::unknown`]).
    pub fn not_known(planned: &PlannedAction, note: impl Into<String>) -> Self {
        Self {
            unknown: true,
            ..Self::failure(planned, note)
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
    /// Registers the start-up task (elevated helper only; see [`crate::boot_task::register_boot_task`]). The
    /// default refuses.
    fn register_boot_task(&self, ctx: &HelperContext) -> Result<(), String> {
        let _ = ctx;
        Err("the start-up task is not supported here".to_owned())
    }
    /// Removes the start-up task if it is ours (elevated helper only; see
    /// [`crate::boot_task::remove_boot_task`]). The default refuses.
    fn remove_boot_task(&self, ctx: &HelperContext) -> Result<(), String> {
        let _ = ctx;
        Err("the start-up task is not supported here".to_owned())
    }
}

/// What "Close now" does for `finding`: terminate each matched process (only those in `session` when given), stop
/// each running Win32 service. Drivers are never touched.
pub fn close_actions(finding: &Finding, session: Option<u32>) -> Vec<Action> {
    finding
        .evidence
        .iter()
        .filter_map(|e| match e {
            Evidence::Process {
                pid,
                name,
                session: s,
                ..
            } if session.is_none_or(|own| *s == Some(own)) => Some(Action::TerminateProcess {
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
/// whole authorisation model: an action on anything a scan does not tie to the catalogue is refused. With
/// `session`, a process is only ended if the scan saw it in that Windows session (never another user's process).
///
/// The start-up task actions are the exception: they need no finding and are accepted only with the reserved entry
/// [`BOOT_TASK_ENTRY`], which in turn is refused for every other action.
pub fn check_action(
    planned: &PlannedAction,
    report: &ScanReport,
    session: Option<u32>,
) -> Result<(), String> {
    match (planned.action.is_boot_task(), planned.entry_id == BOOT_TASK_ENTRY) {
        (true, true) => return Ok(()),
        (true, false) => {
            return Err(format!(
                "{:?} cannot carry the start-up task action",
                planned.entry_id
            ));
        }
        (false, true) => {
            return Err(format!(
                "{BOOT_TASK_ENTRY:?} carries only the start-up task actions"
            ));
        }
        (false, false) => {}
    }
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
            let found = finding.evidence.iter().any(|e| {
                matches!(e, Evidence::Process { pid: p, name: n, session: s, .. }
                    if p == pid && n.eq_ignore_ascii_case(name) && session.is_none_or(|own| *s == Some(own)))
            });
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
        Action::RegisterBootTask {} | Action::RemoveBootTask {} => {
            return Err("the start-up task actions are checked above".to_owned());
        }
    }
    Ok(())
}

/// Runs `actions` through `ops`, autostart changes first, then service stops, then terminations. Returns one
/// outcome per action in the order given. A process that no longer exists counts as closed. The start-up task
/// actions fail here: only the elevated helper runs them ([`run_elevated_request_in`]).
pub fn execute(actions: &[PlannedAction], ops: &dyn SystemOps) -> Vec<ActionOutcome> {
    execute_in(actions, ops, None)
}

fn execute_in(
    actions: &[PlannedAction],
    ops: &dyn SystemOps,
    helper: Option<&HelperContext>,
) -> Vec<ActionOutcome> {
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
            Action::RegisterBootTask {} => match helper {
                Some(ctx) => to_outcome(planned, ops.register_boot_task(ctx)),
                None => {
                    ActionOutcome::failure(planned, "only the elevated helper registers the start-up task")
                }
            },
            Action::RemoveBootTask {} => match helper {
                Some(ctx) => to_outcome(planned, ops.remove_boot_task(ctx)),
                None => ActionOutcome::failure(planned, "only the elevated helper removes the start-up task"),
            },
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
    /// The requesting app's Windows session. Processes are only ended in this session; without it the helper ends
    /// none.
    pub session_id: Option<u32>,
}

impl ElevatedRequest {
    /// A request of the current version.
    pub fn new(actions: Vec<PlannedAction>, session_id: Option<u32>) -> Self {
        Self {
            version: ELEVATED_PROTOCOL_VERSION,
            actions,
            session_id,
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

/// [`run_elevated_request_in`] with the running process's [`HelperContext::current`] (its own session, no request
/// file, so the start-up task actions are refused). Prefer [`run_elevated_request_in`] in the helper.
pub fn run_elevated_request(
    request: &ElevatedRequest,
    report: &ScanReport,
    ops: &dyn SystemOps,
) -> ElevatedResponse {
    run_elevated_request_in(request, report, ops, &HelperContext::current())
}

/// Validates an elevated request against a fresh scan and runs what passes. Used by the helper process with
/// `helper` = [`HelperContext::for_request`] of its validated request file.
///
/// Rules: the version must match, the request may hold at most [`MAX_ELEVATED_ACTIONS`] actions, and a request
/// that names a session must come from the helper's own session (otherwise every action is refused); each action
/// must be one that needs administrator rights or a process termination (HKCU `StartupApproved` writes are refused:
/// in the elevated process HKCU may be another user's hive); a process is only ended in the request's session
/// (none without one); the start-up task actions also need the request's session; and each action must pass
/// [`check_action`] against `report`. Refused actions are reported, never run.
pub fn run_elevated_request_in(
    request: &ElevatedRequest,
    report: &ScanReport,
    ops: &dyn SystemOps,
    helper: &HelperContext,
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
    } else if request.session_id.is_some() && request.session_id != helper.session_id {
        Some(format!(
            "the request comes from Windows session {:?}, the helper runs in session {:?}",
            request.session_id, helper.session_id
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
        let is_termination = matches!(planned.action, Action::TerminateProcess { .. });
        let check = if is_termination && request.session_id.is_none() {
            Err("the request names no session, so no process is ended".to_owned())
        } else if planned.action.is_boot_task() && request.session_id.is_none() {
            Err("the request names no session, so the start-up task is not changed".to_owned())
        } else if elevated_kind {
            check_action(planned, report, request.session_id)
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
    let mut ran = execute_in(&accepted, ops, Some(helper))
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
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

    /// The helper context the existing tests run in: session 1, like the requests.
    fn helper() -> HelperContext {
        crate::boot_task::tests::ctx()
    }

    fn run(request: &ElevatedRequest, report: &ScanReport, ops: &dyn SystemOps) -> ElevatedResponse {
        run_elevated_request_in(request, report, ops, &helper())
    }

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
        fn register_boot_task(&self, ctx: &HelperContext) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("register_boot_task {:?}", ctx.session_id));
            Ok(())
        }
        fn remove_boot_task(&self, ctx: &HelperContext) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("remove_boot_task {:?}", ctx.session_id));
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
                    session: Some(1),
                },
                Evidence::Process {
                    pid: 200,
                    name: "SignalRgb.exe".into(),
                    path: None,
                    session: Some(2),
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
            close_actions(&signalrgb(), Some(1)),
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
            assert_eq!(check_action(&planned(a.clone()), &r, Some(1)), Ok(()), "{a:?}");
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
            assert!(check_action(&planned(a.clone()), &r, Some(1)).is_err(), "{a:?}");
        }
        let other_entry = PlannedAction {
            entry_id: "openrgb".into(),
            action: Action::StopService {
                name: "SignalRgb.Service".into(),
            },
        };
        assert!(check_action(&other_entry, &r, Some(1)).is_err());
        // Another user's process: refused with a session, accepted only when no session is given.
        let other_user = planned(Action::TerminateProcess {
            pid: 200,
            name: "SignalRgb.exe".into(),
        });
        assert!(check_action(&other_user, &r, Some(1)).is_err());
        assert!(check_action(&other_user, &r, None).is_ok());
    }

    #[test]
    fn elevated_request_runs_only_validated_admin_actions() {
        let ops = FakeOps::default();
        let request = ElevatedRequest::new(
            vec![
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
            ],
            Some(1),
        );
        let response = run(&request, &report(), &ops);
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
    fn elevated_terminations_need_the_requesting_session() {
        let ops = FakeOps::default();
        let kill = |pid| {
            planned(Action::TerminateProcess {
                pid,
                name: "SignalRgb.exe".into(),
            })
        };
        let response = run(
            &ElevatedRequest::new(vec![kill(100), kill(200)], Some(1)),
            &report(),
            &ops,
        );
        assert_eq!(
            response.outcomes.iter().map(|o| o.ok).collect::<Vec<_>>(),
            [true, false]
        );
        let response = run(&ElevatedRequest::new(vec![kill(100)], None), &report(), &ops);
        assert!(!response.outcomes[0].ok);
        assert_eq!(*ops.calls.borrow(), ["terminate 100 SignalRgb.exe"]);
    }

    #[test]
    fn elevated_request_with_wrong_version_or_too_many_actions_runs_nothing() {
        let ops = FakeOps::default();
        let action = planned(Action::StopService {
            name: "SignalRgb.Service".into(),
        });
        let mut request = ElevatedRequest::new(vec![action.clone()], Some(1));
        request.version = 99;
        let response = run(&request, &report(), &ops);
        assert!(response.outcomes.iter().all(|o| !o.ok));
        let request = ElevatedRequest::new(vec![action; MAX_ELEVATED_ACTIONS + 1], Some(1));
        let response = run(&request, &report(), &ops);
        assert!(response.outcomes.iter().all(|o| !o.ok));
        assert!(ops.calls.borrow().is_empty());
    }

    #[test]
    fn elevated_request_against_an_empty_scan_runs_nothing() {
        let ops = FakeOps::default();
        let request = ElevatedRequest::new(
            vec![planned(Action::StopService {
                name: "SignalRgb.Service".into(),
            })],
            Some(1),
        );
        let response = run(&request, &ScanReport::default(), &ops);
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

    fn boot(action: Action) -> PlannedAction {
        PlannedAction {
            entry_id: BOOT_TASK_ENTRY.into(),
            action,
        }
    }

    #[test]
    fn boot_task_actions_wire_format() {
        for (action, json) in [
            (Action::RegisterBootTask {}, r#"{"kind":"register_boot_task"}"#),
            (Action::RemoveBootTask {}, r#"{"kind":"remove_boot_task"}"#),
        ] {
            assert_eq!(serde_json::to_string(&action).expect("serializes"), json);
            assert_eq!(serde_json::from_str::<Action>(json).expect("parses"), action);
            assert!(action.needs_admin());
        }
        let planned = boot(Action::RegisterBootTask {});
        let text =
            serde_json::to_string(&ElevatedRequest::new(vec![planned.clone()], Some(1))).expect("request");
        let back: ElevatedRequest = serde_json::from_str(&text).expect("round trip");
        assert_eq!(back.actions, [planned]);
    }

    #[test]
    fn extra_fields_are_refused() {
        for json in [
            r#"{"kind":"register_boot_task","exe":"C:\\x.exe"}"#,
            r#"{"kind":"remove_boot_task","user":"S-1-5-18"}"#,
            r#"{"kind":"register_boot_task","arguments":"--evil"}"#,
            r#"{"kind":"stop_service","name":"a","extra":1}"#,
            r#"{"kind":"unknown_kind"}"#,
        ] {
            assert!(serde_json::from_str::<Action>(json).is_err(), "{json}");
        }
        assert!(serde_json::from_str::<Action>(r#"{"kind":"stop_service","name":"a"}"#).is_ok());
    }

    #[test]
    fn boot_task_entry_id_is_reserved() {
        let r = report();
        assert_eq!(
            check_action(&boot(Action::RegisterBootTask {}), &r, Some(1)),
            Ok(())
        );
        assert_eq!(
            check_action(&boot(Action::RemoveBootTask {}), &ScanReport::default(), None),
            Ok(())
        );
        // Start-up task actions under a catalogue entry, and catalogue actions under the reserved entry.
        assert!(check_action(&planned(Action::RegisterBootTask {}), &r, Some(1)).is_err());
        assert!(
            check_action(
                &boot(Action::StopService {
                    name: "SignalRgb.Service".into()
                }),
                &r,
                Some(1)
            )
            .is_err()
        );
        // The catalogue never uses the reserved id.
        assert!(
            crate::Catalog::embedded()
                .entries()
                .iter()
                .all(|e| e.id != BOOT_TASK_ENTRY)
        );
    }

    #[test]
    fn boot_task_actions_run_only_in_the_elevated_helper() {
        let ops = FakeOps::default();
        let outcomes = execute(&[boot(Action::RegisterBootTask {})], &ops);
        assert!(!outcomes[0].ok);
        assert!(ops.calls.borrow().is_empty());

        let request = ElevatedRequest::new(
            vec![
                boot(Action::RegisterBootTask {}),
                planned(Action::RemoveBootTask {}),
                boot(Action::RemoveBootTask {}),
                planned(Action::StopService {
                    name: "SignalRgb.Service".into(),
                }),
            ],
            Some(1),
        );
        let response = run(&request, &report(), &ops);
        assert_eq!(
            response.outcomes.iter().map(|o| o.ok).collect::<Vec<_>>(),
            [true, false, true, true]
        );
        assert_eq!(
            *ops.calls.borrow(),
            [
                "stop SignalRgb.Service",
                "register_boot_task Some(1)",
                "remove_boot_task Some(1)"
            ]
        );
    }

    #[test]
    fn boot_task_actions_need_a_session() {
        let ops = FakeOps::default();
        let response = run(
            &ElevatedRequest::new(vec![boot(Action::RegisterBootTask {})], None),
            &report(),
            &ops,
        );
        assert!(!response.outcomes[0].ok);
        assert!(ops.calls.borrow().is_empty());
    }

    #[test]
    fn a_request_from_another_session_is_refused_as_a_whole() {
        let ops = FakeOps::default();
        let request = ElevatedRequest::new(
            vec![
                boot(Action::RegisterBootTask {}),
                planned(Action::StopService {
                    name: "SignalRgb.Service".into(),
                }),
            ],
            Some(2),
        );
        let response = run(&request, &report(), &ops);
        assert!(response.outcomes.iter().all(|o| !o.ok));
        let unknown_own = HelperContext {
            session_id: None,
            ..helper()
        };
        let response = run_elevated_request_in(&request, &report(), &ops, &unknown_own);
        assert!(response.outcomes.iter().all(|o| !o.ok));
        assert!(ops.calls.borrow().is_empty());
    }
}
