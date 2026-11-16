//! The start dialog (ADR 0001 #31): which RGB suites to offer to close, and the actions a click turns into.
//!
//! Everything here is pure: it takes a scan report, the visible devices and the remembered settings and returns the
//! dialog model or the planned actions. The commands run the actions (see `commands::run_actions`).

use std::collections::{BTreeMap, BTreeSet};

use cc_conflicts::{
    Action, ActionOutcome, AutostartItem, Class, Finding, PlannedAction, ScanReport, StartType,
    autostart_actions, close_actions,
};
use serde::{Deserialize, Serialize};

use crate::conflicts::{AffectedDevice, KnownDevice, affected_devices};

/// Something "Close now" ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloseTarget {
    /// A running process.
    Process {
        /// Process id.
        pid: u32,
        /// Executable name.
        name: String,
    },
    /// A running Win32 service (stopping it needs the permission prompt).
    Service {
        /// Service name.
        name: String,
    },
}

/// One program in the start dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConflictProgram {
    /// Catalogue id (what `resolve_conflicts`, `set_autostart` and `set_dont_ask` take).
    pub entry_id: String,
    /// Product name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Catalogue note.
    pub note: String,
    /// Our visible devices it affects (the "why").
    pub affected_devices: Vec<AffectedDevice>,
    /// Something of it is active now (process, running service or driver).
    pub running: bool,
    /// What "Close now" ends. Drivers are never closed.
    pub close_targets: Vec<CloseTarget>,
    /// The enabled autostart items "Turn off autostart" switches off.
    pub autostart: Vec<AutostartItem>,
    /// At least one autostart item is on (drives the persistent hint).
    pub autostart_on: bool,
    /// Proceeding with both options checked will show a Windows permission prompt.
    pub needs_admin: bool,
}

/// The start dialog model: programs in scan order (running first, then by severity).
pub type StartupConflicts = Vec<ConflictProgram>;

/// What "Close now" ends for `finding`: its processes in `session` (when given) and its running Win32 services.
pub fn close_targets(finding: &Finding, session: Option<u32>) -> Vec<CloseTarget> {
    close_actions(finding, session)
        .into_iter()
        .filter_map(|a| match a {
            Action::TerminateProcess { pid, name } => Some(CloseTarget::Process { pid, name }),
            Action::StopService { name } => Some(CloseTarget::Service { name }),
            _ => None,
        })
        .collect()
}

/// The dialog entry for `finding`, or `None` if it does not belong in the dialog: it must be a suite, affect at
/// least one of `visible` (the detected, non-hidden devices), not be in `dont_ask`, and have something to do
/// (a process or running service to close, or an autostart item that is on). A suite that is only "running"
/// through a loaded kernel driver has nothing to close and is left out, so the dialog does not come back on every
/// start for something it cannot change.
pub fn program(
    finding: &Finding,
    visible: &[KnownDevice],
    dont_ask: &BTreeSet<String>,
    session: Option<u32>,
) -> Option<ConflictProgram> {
    if finding.class != Class::Suite || dont_ask.contains(&finding.entry_id) {
        return None;
    }
    let affected = affected_devices(finding, visible);
    if affected.is_empty() {
        return None;
    }
    let close_targets = close_targets(finding, session);
    let autostart: Vec<AutostartItem> = finding
        .autostart
        .iter()
        .filter(|i| i.can_turn_off())
        .cloned()
        .collect();
    if close_targets.is_empty() && autostart.is_empty() {
        return None;
    }
    let needs_admin = close_targets
        .iter()
        .any(|t| matches!(t, CloseTarget::Service { .. }))
        || autostart.iter().any(|i| i.needs_admin);
    Some(ConflictProgram {
        entry_id: finding.entry_id.clone(),
        name: finding.name.clone(),
        vendor: finding.vendor.clone(),
        note: finding.note.clone(),
        affected_devices: affected,
        running: finding.running,
        autostart_on: !autostart.is_empty(),
        close_targets,
        autostart,
        needs_admin,
    })
}

/// The start dialog model for `report`.
pub fn startup_model(
    report: &ScanReport,
    visible: &[KnownDevice],
    dont_ask: &BTreeSet<String>,
    session: Option<u32>,
) -> StartupConflicts {
    report
        .findings
        .iter()
        .filter_map(|f| program(f, visible, dont_ask, session))
        .collect()
}

/// What the user chose for one program of the dialog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResolveItem {
    /// Catalogue id.
    pub entry_id: String,
    /// "Close now" checked.
    pub close: bool,
    /// "Turn off autostart" checked.
    pub disable_autostart: bool,
}

/// The dialog's Proceed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResolveRequest {
    /// One entry per program the user left in the dialog.
    pub items: Vec<ResolveItem>,
}

/// Outcome of one action, as the frontend shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionResult {
    /// Catalogue id.
    pub entry_id: String,
    /// What was done.
    pub action: Action,
    /// Success.
    pub ok: bool,
    /// Why it failed.
    pub error: Option<String>,
    /// Not confirmed yet (the elevated helper was still running); `ok` is `false`, a rescan shows the state.
    pub unknown: bool,
}

impl From<ActionOutcome> for ActionResult {
    fn from(o: ActionOutcome) -> Self {
        Self {
            entry_id: o.entry_id,
            action: o.action,
            ok: o.ok,
            error: o.error,
            unknown: o.unknown,
        }
    }
}

/// A plan: actions to run and items refused up front (with their reason).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Actions to run.
    pub actions: Vec<PlannedAction>,
    /// Requests that could not be turned into actions: `(entry_id, reason)`.
    pub refused: Vec<(String, String)>,
}

/// Turns the dialog's Proceed into actions, checked against `report` (a fresh scan): each item must still be a
/// dialog program for `visible` (the "don't ask again" set is ignored here, so ticking it together with Proceed
/// works). Close: processes and running services. Turn off autostart: every enabled toggleable item.
pub fn plan_resolve(
    request: &ResolveRequest,
    report: &ScanReport,
    visible: &[KnownDevice],
    session: Option<u32>,
) -> Plan {
    let mut plan = Plan::default();
    let none = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for item in &request.items {
        if !seen.insert(item.entry_id.clone()) {
            continue;
        }
        let finding = report.findings.iter().find(|f| f.entry_id == item.entry_id);
        if finding
            .and_then(|f| program(f, visible, &none, session))
            .is_none()
        {
            plan.refused.push((
                item.entry_id.clone(),
                "this program is no longer running or starting with Windows".to_owned(),
            ));
            continue;
        }
        let Some(finding) = finding else { continue };
        let mut actions = Vec::new();
        if item.disable_autostart {
            actions.extend(autostart_actions(finding, false, |_| None));
        }
        if item.close {
            actions.extend(close_actions(finding, session));
        }
        plan.actions
            .extend(actions.into_iter().map(|action| PlannedAction {
                entry_id: item.entry_id.clone(),
                action,
            }));
    }
    plan
}

/// The actions of the Conflicts page's End process / Stop service for `targets` of `entry_id` (any class). Every
/// target must still be a close target of the finding in `report` (a fresh scan); anything else is an error and
/// nothing runs.
pub fn plan_close(
    entry_id: &str,
    targets: &[CloseTarget],
    report: &ScanReport,
    session: Option<u32>,
) -> Result<Vec<PlannedAction>, String> {
    let finding = report
        .findings
        .iter()
        .find(|f| f.entry_id == entry_id)
        .ok_or_else(|| format!("{entry_id:?} was not found by a fresh scan; scan again"))?;
    let current = close_targets(finding, session);
    targets
        .iter()
        .map(|t| {
            let found = current.iter().any(|c| match (c, t) {
                (CloseTarget::Process { pid: a, name: n }, CloseTarget::Process { pid: b, name: m }) => {
                    a == b && n.eq_ignore_ascii_case(m)
                }
                (CloseTarget::Service { name: n }, CloseTarget::Service { name: m }) => {
                    n.eq_ignore_ascii_case(m)
                }
                _ => false,
            });
            if !found {
                return Err(match t {
                    CloseTarget::Process { pid, name } => {
                        format!("{name} (process {pid}) is no longer running; scan again")
                    }
                    CloseTarget::Service { name } => {
                        format!("service {name} is no longer running; scan again")
                    }
                });
            }
            Ok(PlannedAction {
                entry_id: entry_id.to_owned(),
                action: match t {
                    CloseTarget::Process { pid, name } => Action::TerminateProcess {
                        pid: *pid,
                        name: name.clone(),
                    },
                    CloseTarget::Service { name } => Action::StopService { name: name.clone() },
                },
            })
        })
        .collect()
}

/// The actions of the Conflicts page's Turn off (`enabled = false`) / Re-enable (`true`) for `entry_id`, any class.
pub fn plan_autostart(
    entry_id: &str,
    enabled: bool,
    report: &ScanReport,
    remembered: &BTreeMap<String, String>,
) -> Result<Vec<PlannedAction>, String> {
    let finding = report
        .findings
        .iter()
        .find(|f| f.entry_id == entry_id)
        .ok_or_else(|| format!("{entry_id:?} was not found by a fresh scan"))?;
    Ok(
        autostart_actions(finding, enabled, |name| remembered_start_type(remembered, name))
            .into_iter()
            .map(|action| PlannedAction {
                entry_id: entry_id.to_owned(),
                action,
            })
            .collect(),
    )
}

/// The lower-case name of a start type as stored in `conflicts.previous_start_types`.
pub fn start_type_name(st: StartType) -> &'static str {
    match st {
        StartType::Boot => "boot",
        StartType::System => "system",
        StartType::Auto => "auto",
        StartType::Manual => "manual",
        StartType::Disabled => "disabled",
    }
}

/// Parses a stored start type name.
pub fn parse_start_type(name: &str) -> Option<StartType> {
    Some(match name {
        "boot" => StartType::Boot,
        "system" => StartType::System,
        "auto" => StartType::Auto,
        "manual" => StartType::Manual,
        "disabled" => StartType::Disabled,
        _ => return None,
    })
}

/// The start type remembered for service `name` (service names compare case-insensitively).
pub fn remembered_start_type(remembered: &BTreeMap<String, String>, name: &str) -> Option<StartType> {
    remembered
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| parse_start_type(v))
}

/// Updates the remembered start types for `actions` before they run: a service about to be set to Manual gets its
/// current start type (from `report`) remembered unless one is already stored. Returns the names it added.
pub fn remember_start_types(
    remembered: &mut BTreeMap<String, String>,
    actions: &[PlannedAction],
    report: &ScanReport,
) -> Vec<String> {
    let mut added = Vec::new();
    for planned in actions {
        let Action::SetServiceStartType {
            name,
            start_type: StartType::Manual,
        } = &planned.action
        else {
            continue;
        };
        if remembered_start_type(remembered, name).is_some() {
            continue;
        }
        let current = report
            .findings
            .iter()
            .filter(|f| f.entry_id == planned.entry_id)
            .flat_map(|f| &f.autostart)
            .find(|i| {
                i.location == cc_conflicts::AutostartLocation::Service && i.name.eq_ignore_ascii_case(name)
            })
            .and_then(|i| i.start_type);
        if let Some(st) = current {
            remembered.insert(name.clone(), start_type_name(st).to_owned());
            added.push(name.clone());
        }
    }
    added
}

/// Forgets the memories `newly` (service names remembered for this batch) whose change to Manual failed or did not
/// run, so a failed "Turn off" leaves no memory behind. Returns whether anything changed.
pub fn forget_failed_turn_off(
    remembered: &mut BTreeMap<String, String>,
    newly: &[String],
    outcomes: &[ActionOutcome],
) -> bool {
    let before = remembered.len();
    for name in newly {
        let succeeded = outcomes.iter().any(|o| {
            o.ok && matches!(&o.action, Action::SetServiceStartType { name: n, start_type: StartType::Manual }
                if n.eq_ignore_ascii_case(name))
        });
        if !succeeded {
            remembered.retain(|k, _| !k.eq_ignore_ascii_case(name));
        }
    }
    remembered.len() != before
}

/// Forgets the remembered start type of every service that was successfully set back from Manual. Returns whether
/// anything changed.
pub fn forget_restored(remembered: &mut BTreeMap<String, String>, outcomes: &[ActionOutcome]) -> bool {
    let before = remembered.len();
    for o in outcomes.iter().filter(|o| o.ok) {
        if let Action::SetServiceStartType { name, start_type } = &o.action
            && *start_type != StartType::Manual
        {
            remembered.retain(|k, _| !k.eq_ignore_ascii_case(name));
        }
    }
    remembered.len() != before
}

#[cfg(test)]
mod tests {
    use cc_conflicts::{Access, AutostartLocation, Category, Evidence, ServiceKind, ServiceState, Severity};
    use cc_core::{DeviceId, family};

    use super::*;

    fn device(family: &str) -> KnownDevice {
        KnownDevice {
            id: DeviceId::hid(0x31E3, 0x1310, Some("X")),
            name: "Wooting 60HE".into(),
            family: family.into(),
        }
    }

    fn app_item(location: AutostartLocation, name: &str, enabled: bool) -> AutostartItem {
        AutostartItem {
            location,
            name: name.into(),
            command: Some(format!("\"C:\\x\\{name}.exe\"")),
            enabled,
            start_type: None,
            toggleable: location.approval_key().is_some(),
            needs_admin: location.needs_admin(),
        }
    }

    fn finding(id: &str, class: Class, affects: &[&str]) -> Finding {
        Finding {
            entry_id: id.into(),
            name: id.to_uppercase(),
            vendor: "V".into(),
            category: Category::RgbSuite,
            class,
            severity: Severity::High,
            verified: true,
            access: vec![Access::Hid],
            affects: affects.iter().map(|s| (*s).to_owned()).collect(),
            note: "n".into(),
            running: false,
            installed: true,
            evidence: vec![],
            autostart: vec![],
        }
    }

    /// SignalRGB as on the development machine: launcher in HKCU Run (on), service Auto and stopped, the kernel
    /// driver loaded, the app itself running.
    fn signalrgb() -> Finding {
        let mut f = finding("signalrgb", Class::Suite, &[family::WOOTING, family::LOGITECH]);
        f.running = true;
        f.evidence = vec![
            Evidence::Process {
                pid: 42,
                name: "SignalRgb.exe".into(),
                path: None,
                session: Some(1),
            },
            // Another user's SignalRGB: never a close target.
            Evidence::Process {
                pid: 43,
                name: "SignalRgb.exe".into(),
                path: None,
                session: Some(2),
            },
            Evidence::Service {
                name: "SignalRgb.Service".into(),
                state: ServiceState::Stopped,
                kind: ServiceKind::Win32,
            },
            Evidence::Driver {
                name: "SignalRgbDriver".into(),
                state: ServiceState::Running,
                path: None,
            },
        ];
        f.autostart = vec![
            app_item(AutostartLocation::HkcuRun, "SignalRgb", true),
            AutostartItem::service("SignalRgb.Service", Some(StartType::Auto)),
        ];
        f
    }

    /// OpenRGB: service Auto and running.
    fn openrgb() -> Finding {
        let mut f = finding("openrgb", Class::Suite, &["all"]);
        f.running = true;
        f.evidence = vec![Evidence::Service {
            name: "OpenRGB".into(),
            state: ServiceState::Running,
            kind: ServiceKind::Win32,
        }];
        f.autostart = vec![AutostartItem::service("OpenRGB", Some(StartType::Auto))];
        f
    }

    fn ghub() -> Finding {
        let mut f = finding("logitech_g_hub", Class::VendorTool, &[family::LOGITECH]);
        f.running = true;
        f.evidence = vec![Evidence::Process {
            pid: 7,
            name: "lghub.exe".into(),
            path: None,
            session: Some(1),
        }];
        f.autostart = vec![app_item(AutostartLocation::HkcuRun, "LGHUB", true)];
        f
    }

    fn report(findings: Vec<Finding>) -> ScanReport {
        ScanReport {
            findings,
            errors: vec![],
        }
    }

    fn ids(model: &StartupConflicts) -> Vec<&str> {
        model.iter().map(|p| p.entry_id.as_str()).collect()
    }

    #[test]
    fn dialog_lists_suites_that_affect_visible_devices() {
        let r = report(vec![signalrgb(), openrgb(), ghub()]);
        let wooting = [device(family::WOOTING)];
        let model = startup_model(&r, &wooting, &BTreeSet::new(), Some(1));
        assert_eq!(ids(&model), ["signalrgb", "openrgb"], "vendor tools never appear");

        let signal = &model[0];
        assert_eq!(signal.affected_devices.len(), 1);
        assert_eq!(
            signal.close_targets,
            [CloseTarget::Process {
                pid: 42,
                name: "SignalRgb.exe".into()
            }],
            "stopped services and drivers are not close targets"
        );
        assert_eq!(signal.autostart.len(), 2);
        assert!(signal.autostart_on && signal.needs_admin && signal.running);

        let open = &model[1];
        assert_eq!(
            open.close_targets,
            [CloseTarget::Service {
                name: "OpenRGB".into()
            }]
        );
        assert!(open.needs_admin);
    }

    #[test]
    fn no_visible_device_means_no_dialog() {
        let r = report(vec![signalrgb(), openrgb()]);
        assert!(startup_model(&r, &[], &BTreeSet::new(), Some(1)).is_empty());
        // A device of a family only OpenRGB ("all") affects in this fixture.
        let msi = [device(family::MSI_MYSTIC_LIGHT)];
        assert_eq!(
            ids(&startup_model(&r, &msi, &BTreeSet::new(), Some(1))),
            ["openrgb"]
        );
    }

    #[test]
    fn dont_ask_hides_a_program() {
        let r = report(vec![signalrgb(), openrgb()]);
        let dont = BTreeSet::from(["openrgb".to_owned()]);
        assert_eq!(
            ids(&startup_model(&r, &[device(family::WOOTING)], &dont, Some(1))),
            ["signalrgb"]
        );
    }

    #[test]
    fn nothing_to_do_means_not_in_the_dialog() {
        // Only the driver is loaded and autostart is already off.
        let mut f = signalrgb();
        f.evidence.retain(|e| matches!(e, Evidence::Driver { .. }));
        f.autostart = vec![
            app_item(AutostartLocation::HkcuRun, "SignalRgb", false),
            AutostartItem::service("SignalRgb.Service", Some(StartType::Manual)),
        ];
        let r = report(vec![f.clone()]);
        assert!(startup_model(&r, &[device(family::WOOTING)], &BTreeSet::new(), Some(1)).is_empty());

        // Not running, but autostart on: shown, without close targets, with the hint flag.
        f.autostart[0].enabled = true;
        let model = startup_model(
            &report(vec![f]),
            &[device(family::WOOTING)],
            &BTreeSet::new(),
            Some(1),
        );
        assert_eq!(model.len(), 1);
        assert!(model[0].close_targets.is_empty());
        assert!(model[0].autostart_on && !model[0].needs_admin);
    }

    #[test]
    fn proceed_turns_into_actions() {
        let r = report(vec![signalrgb(), openrgb(), ghub()]);
        let visible = [device(family::WOOTING)];
        let request = ResolveRequest {
            items: vec![
                ResolveItem {
                    entry_id: "signalrgb".into(),
                    close: true,
                    disable_autostart: true,
                },
                ResolveItem {
                    entry_id: "openrgb".into(),
                    close: true,
                    disable_autostart: false,
                },
                // Vendor tool: refused even if the frontend asks.
                ResolveItem {
                    entry_id: "logitech_g_hub".into(),
                    close: true,
                    disable_autostart: true,
                },
                ResolveItem {
                    entry_id: "nonexistent".into(),
                    close: true,
                    disable_autostart: true,
                },
            ],
        };
        let plan = plan_resolve(&request, &r, &visible, Some(1));
        let actions: Vec<(&str, &Action)> = plan
            .actions
            .iter()
            .map(|p| (p.entry_id.as_str(), &p.action))
            .collect();
        assert_eq!(
            actions,
            [
                (
                    "signalrgb",
                    &Action::SetStartupApproved {
                        location: AutostartLocation::HkcuRun,
                        name: "SignalRgb".into(),
                        enabled: false
                    }
                ),
                (
                    "signalrgb",
                    &Action::SetServiceStartType {
                        name: "SignalRgb.Service".into(),
                        start_type: StartType::Manual
                    }
                ),
                (
                    "signalrgb",
                    &Action::TerminateProcess {
                        pid: 42,
                        name: "SignalRgb.exe".into()
                    }
                ),
                (
                    "openrgb",
                    &Action::StopService {
                        name: "OpenRGB".into()
                    }
                ),
            ]
        );
        let refused: Vec<&str> = plan.refused.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(refused, ["logitech_g_hub", "nonexistent"]);

        // Both boxes unchecked: nothing to do, nothing refused.
        let plan = plan_resolve(
            &ResolveRequest {
                items: vec![ResolveItem {
                    entry_id: "signalrgb".into(),
                    close: false,
                    disable_autostart: false,
                }],
            },
            &r,
            &visible,
            Some(1),
        );
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn proceed_ignores_dont_ask_but_not_visibility() {
        let r = report(vec![openrgb()]);
        let item = ResolveRequest {
            items: vec![ResolveItem {
                entry_id: "openrgb".into(),
                close: true,
                disable_autostart: false,
            }],
        };
        assert_eq!(
            plan_resolve(&item, &r, &[device(family::WOOTING)], Some(1))
                .actions
                .len(),
            1
        );
        let plan = plan_resolve(&item, &r, &[], Some(1));
        assert!(plan.actions.is_empty());
        assert_eq!(plan.refused.len(), 1);
    }

    #[test]
    fn autostart_page_actions_use_remembered_start_types() {
        let mut f = signalrgb();
        f.autostart = vec![
            app_item(AutostartLocation::HkcuRun, "SignalRgb", false),
            AutostartItem::service("SignalRgb.Service", Some(StartType::Manual)),
        ];
        let r = report(vec![f, ghub()]);
        let none = BTreeMap::new();
        let on = plan_autostart("signalrgb", true, &r, &none).expect("found");
        assert_eq!(
            on.len(),
            1,
            "a Manual service CaseControl did not change is left alone"
        );
        let remembered = BTreeMap::from([("signalrgb.service".to_owned(), "auto".to_owned())]);
        let on = plan_autostart("signalrgb", true, &r, &remembered).expect("found");
        assert!(on.iter().any(|p| p.action
            == Action::SetServiceStartType {
                name: "SignalRgb.Service".into(),
                start_type: StartType::Auto
            }));
        // Vendor tools work on the Conflicts page.
        let off = plan_autostart("logitech_g_hub", false, &r, &none).expect("found");
        assert_eq!(off.len(), 1);
        assert!(plan_autostart("nonexistent", false, &r, &none).is_err());
    }

    #[test]
    fn page_close_targets_must_be_current() {
        let r = report(vec![signalrgb(), openrgb(), ghub()]);
        let ok = plan_close(
            "logitech_g_hub",
            &[CloseTarget::Process {
                pid: 7,
                name: "LGHUB.exe".into(),
            }],
            &r,
            Some(1),
        )
        .expect("current target");
        assert_eq!(
            ok[0].action,
            Action::TerminateProcess {
                pid: 7,
                name: "LGHUB.exe".into()
            }
        );
        let svc = plan_close(
            "openrgb",
            &[CloseTarget::Service {
                name: "OpenRGB".into(),
            }],
            &r,
            Some(1),
        )
        .expect("service");
        assert_eq!(
            svc[0].action,
            Action::StopService {
                name: "OpenRGB".into()
            }
        );
        // Stale pid, a stopped service, a target of another program, an unknown program.
        assert!(
            plan_close(
                "logitech_g_hub",
                &[CloseTarget::Process {
                    pid: 8,
                    name: "lghub.exe".into()
                }],
                &r,
                Some(1)
            )
            .is_err()
        );
        assert!(
            plan_close(
                "signalrgb",
                &[CloseTarget::Service {
                    name: "SignalRgb.Service".into()
                }],
                &r,
                Some(1)
            )
            .is_err()
        );
        assert!(
            plan_close(
                "signalrgb",
                &[CloseTarget::Service {
                    name: "OpenRGB".into()
                }],
                &r,
                Some(1)
            )
            .is_err()
        );
        assert!(plan_close("nope", &[], &r, Some(1)).is_err());
        let json: CloseTarget =
            serde_json::from_value(serde_json::json!({ "kind": "process", "pid": 7, "name": "a.exe" }))
                .expect("parses");
        assert_eq!(
            json,
            CloseTarget::Process {
                pid: 7,
                name: "a.exe".into()
            }
        );
    }

    #[test]
    fn start_types_are_remembered_and_forgotten() {
        let r = report(vec![openrgb()]);
        let off = vec![PlannedAction {
            entry_id: "openrgb".into(),
            action: Action::SetServiceStartType {
                name: "OpenRGB".into(),
                start_type: StartType::Manual,
            },
        }];
        let mut remembered = BTreeMap::new();
        assert_eq!(remember_start_types(&mut remembered, &off, &r), ["OpenRGB"]);
        assert_eq!(remembered["OpenRGB"], "auto");
        // A second turn-off (e.g. after the user re-set it to Auto by hand) keeps the first memory.
        assert!(remember_start_types(&mut remembered, &off, &r).is_empty());

        let on = PlannedAction {
            entry_id: "openrgb".into(),
            action: Action::SetServiceStartType {
                name: "openrgb".into(),
                start_type: StartType::Auto,
            },
        };
        let failed = ActionOutcome::failure(&on, "denied");
        assert!(!forget_restored(&mut remembered, &[failed]));
        assert!(forget_restored(&mut remembered, &[ActionOutcome::success(&on)]));
        assert!(remembered.is_empty());
    }

    #[test]
    fn failed_turn_off_leaves_no_memory() {
        let r = report(vec![openrgb()]);
        let off = vec![PlannedAction {
            entry_id: "openrgb".into(),
            action: Action::SetServiceStartType {
                name: "OpenRGB".into(),
                start_type: StartType::Manual,
            },
        }];
        let mut remembered = BTreeMap::from([("Other".to_owned(), "auto".to_owned())]);
        let added = remember_start_types(&mut remembered, &off, &r);
        assert!(!forget_failed_turn_off(
            &mut remembered,
            &added,
            &[ActionOutcome::success(&off[0])]
        ));
        assert!(remembered.contains_key("OpenRGB"));

        remembered.remove("OpenRGB");
        let added = remember_start_types(&mut remembered, &off, &r);
        let declined = ActionOutcome::failure(&off[0], "the Windows permission prompt was declined");
        assert!(forget_failed_turn_off(&mut remembered, &added, &[declined]));
        assert_eq!(
            remembered.keys().collect::<Vec<_>>(),
            ["Other"],
            "older memories stay"
        );
        // Not run at all (no outcome) counts as failed too.
        let added = remember_start_types(&mut remembered, &off, &r);
        assert!(forget_failed_turn_off(&mut remembered, &added, &[]));
    }

    #[test]
    fn other_sessions_are_never_close_targets() {
        let r = report(vec![signalrgb()]);
        let model = startup_model(&r, &[device(family::WOOTING)], &BTreeSet::new(), Some(1));
        assert!(
            model[0]
                .close_targets
                .iter()
                .all(|t| !matches!(t, CloseTarget::Process { pid: 43, .. }))
        );
        assert!(
            plan_close(
                "signalrgb",
                &[CloseTarget::Process {
                    pid: 43,
                    name: "SignalRgb.exe".into()
                }],
                &r,
                Some(1)
            )
            .is_err()
        );
    }

    #[test]
    fn start_type_names_round_trip() {
        for st in [
            StartType::Boot,
            StartType::System,
            StartType::Auto,
            StartType::Manual,
            StartType::Disabled,
        ] {
            assert_eq!(parse_start_type(start_type_name(st)), Some(st));
        }
        assert_eq!(parse_start_type("Auto"), None);
    }

    #[test]
    fn model_serializes_to_frontend_shape() {
        let r = report(vec![signalrgb()]);
        let model = startup_model(&r, &[device(family::WOOTING)], &BTreeSet::new(), Some(1));
        let json = serde_json::to_value(&model).expect("serializes");
        let p = &json[0];
        assert_eq!(p["entry_id"], serde_json::json!("signalrgb"));
        assert_eq!(p["close_targets"][0]["kind"], serde_json::json!("process"));
        assert_eq!(p["close_targets"][0]["pid"], serde_json::json!(42));
        assert_eq!(p["autostart"][0]["location"], serde_json::json!("hkcu_run"));
        assert_eq!(p["autostart"][1]["start_type"], serde_json::json!("auto"));
        assert_eq!(
            p["affected_devices"][0]["name"],
            serde_json::json!("Wooting 60HE")
        );
        assert_eq!(p["autostart_on"], serde_json::json!(true));

        let result = ActionResult::from(ActionOutcome::failure(
            &PlannedAction {
                entry_id: "openrgb".into(),
                action: Action::StopService {
                    name: "OpenRGB".into(),
                },
            },
            "declined",
        ));
        let json = serde_json::to_value(result).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({
                "entry_id": "openrgb",
                "action": { "kind": "stop_service", "name": "OpenRGB" },
                "ok": false,
                "error": "declined",
                "unknown": false
            })
        );
    }
}
