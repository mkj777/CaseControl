//! Conflict scan results as the Conflicts page shows them. Actions are checked against a fresh scan in
//! `conflict_actions` (so the frontend cannot ask to end an arbitrary pid or stop an arbitrary service).

use std::collections::{BTreeMap, BTreeSet};

use cc_conflicts::{AFFECTS_ALL, Finding, ScanReport, autostart_actions};
use cc_core::DeviceId;
use cc_engine::DeviceView;
use serde::Serialize;

use crate::startup::{CloseTarget, StartupConflicts, close_targets, remembered_start_type, startup_model};

/// One of our devices a finding may interfere with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AffectedDevice {
    /// Device id.
    pub id: DeviceId,
    /// Display name.
    pub name: String,
}

/// A finding plus the devices under our control it affects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindingView {
    /// The finding as `cc-conflicts` reports it (fields flattened into this object).
    #[serde(flatten)]
    pub finding: Finding,
    /// Detected devices whose family the finding lists in `affects` (all of them for `"all"`).
    pub affected_devices: Vec<AffectedDevice>,
    /// The user ticked "Don't ask again" for it (it never shows in the start dialog).
    pub dont_ask: bool,
    /// What End process / Stop service can end: its processes in this session and its running Win32 services.
    pub close_targets: Vec<CloseTarget>,
    /// At least one of its autostart items is on and can be turned off ("Turn off" on the Conflicts page).
    pub autostart_on: bool,
    /// At least one autostart item is off and can be turned back on ("Re-enable"): a switched-off Run / Startup
    /// folder entry, or a service CaseControl set to Manual.
    pub can_reenable: bool,
}

/// Result of a conflict scan for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConflictScan {
    /// Findings, running first, then by descending severity.
    pub findings: Vec<FindingView>,
    /// Scan sources that failed.
    pub errors: Vec<String>,
    /// The start dialog model for this scan (empty when there is nothing to ask). Drives the persistent hint.
    pub startup: StartupConflicts,
}

/// What a conflict view needs besides the scan.
#[derive(Debug, Clone, Copy)]
pub struct ViewContext<'a> {
    /// Every controlled device (for the Conflicts page's "affects").
    pub devices: &'a [KnownDevice],
    /// Controlled devices the user did not hide (for the start dialog).
    pub visible: &'a [KnownDevice],
    /// `conflicts.dont_ask`.
    pub dont_ask: &'a BTreeSet<String>,
    /// `conflicts.previous_start_types`.
    pub remembered: &'a BTreeMap<String, String>,
    /// The app's Windows session (processes of other sessions are never offered for closing).
    pub session: Option<u32>,
}

/// A device under our control, as far as conflict matching needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownDevice {
    /// Device id.
    pub id: DeviceId,
    /// Display name.
    pub name: String,
    /// Family id of the backend that detected it (`DeviceView::family`).
    pub family: String,
}

impl From<&DeviceView> for KnownDevice {
    fn from(view: &DeviceView) -> Self {
        Self {
            id: view.description.info.id.clone(),
            name: view.description.info.name.clone(),
            family: view.family.clone(),
        }
    }
}

/// The devices in `devices` that `finding` affects.
pub fn affected_devices(finding: &Finding, devices: &[KnownDevice]) -> Vec<AffectedDevice> {
    let all = finding.affects.iter().any(|a| a == AFFECTS_ALL);
    devices
        .iter()
        .filter(|d| all || finding.affects.contains(&d.family))
        .map(|d| AffectedDevice {
            id: d.id.clone(),
            name: d.name.clone(),
        })
        .collect()
}

/// Builds the frontend view of a scan report.
pub fn scan_view(report: &ScanReport, ctx: &ViewContext<'_>) -> ConflictScan {
    ConflictScan {
        findings: report
            .findings
            .iter()
            .map(|f| FindingView {
                affected_devices: affected_devices(f, ctx.devices),
                dont_ask: ctx.dont_ask.contains(&f.entry_id),
                close_targets: close_targets(f, ctx.session),
                autostart_on: f.autostart.iter().any(|i| i.can_turn_off()),
                can_reenable: !autostart_actions(f, true, |name| remembered_start_type(ctx.remembered, name))
                    .is_empty(),
                finding: f.clone(),
            })
            .collect(),
        errors: report.errors.clone(),
        startup: startup_model(report, ctx.visible, ctx.dont_ask, ctx.session),
    }
}

#[cfg(test)]
mod tests {
    use cc_conflicts::{
        Access, AutostartItem, AutostartLocation, Category, Class, Evidence, ServiceKind, ServiceState,
        Severity, StartType,
    };
    use cc_core::family;

    use super::*;

    fn device(id: DeviceId, name: &str, family: &str) -> KnownDevice {
        KnownDevice {
            id,
            name: name.into(),
            family: family.into(),
        }
    }

    fn sample_finding(affects: &[&str]) -> Finding {
        Finding {
            entry_id: "ghub".into(),
            name: "G HUB".into(),
            vendor: "Logitech".into(),
            category: Category::VendorRgb,
            class: Class::VendorTool,
            severity: Severity::High,
            verified: true,
            access: vec![Access::Hid],
            affects: affects.iter().map(|s| (*s).to_owned()).collect(),
            note: String::new(),
            running: true,
            installed: true,
            evidence: vec![
                Evidence::Process {
                    pid: 42,
                    name: "lghub.exe".into(),
                    path: None,
                    session: Some(1),
                },
                Evidence::Service {
                    name: "LGHUBUpdaterService".into(),
                    state: ServiceState::Running,
                    kind: ServiceKind::Win32,
                },
            ],
            autostart: vec![
                AutostartItem {
                    location: AutostartLocation::HkcuRun,
                    name: "LGHUB".into(),
                    command: Some("x".into()),
                    enabled: true,
                    start_type: None,
                    toggleable: true,
                    needs_admin: false,
                },
                AutostartItem::service("LGHUBUpdaterService", Some(StartType::Manual)),
            ],
        }
    }

    fn devices() -> Vec<KnownDevice> {
        vec![
            device(DeviceId::hid(0x046D, 0x0A78, None), "G560", family::LOGITECH),
            device(
                DeviceId::hid(0x31E3, 0x1310, Some("X")),
                "Wooting",
                family::WOOTING,
            ),
            device(
                DeviceId::nvapi(0x2C02_10DE, 0x2095_10DE),
                "RTX 5080",
                family::NVIDIA_ILLUMINATION,
            ),
        ]
    }

    #[test]
    fn affected_by_family_and_all() {
        let names = |f: &Finding| -> Vec<String> {
            affected_devices(f, &devices())
                .into_iter()
                .map(|d| d.name)
                .collect()
        };
        assert_eq!(names(&sample_finding(&[family::LOGITECH])), ["G560"]);
        assert_eq!(names(&sample_finding(&[AFFECTS_ALL])).len(), 3);
        assert!(names(&sample_finding(&[family::MSI_MYSTIC_LIGHT])).is_empty());
    }

    #[test]
    fn view_flattens_finding() {
        let report = ScanReport {
            findings: vec![sample_finding(&[family::LOGITECH])],
            errors: vec!["services: denied".into()],
        };
        let devices = devices();
        let dont_ask = BTreeSet::from(["ghub".to_owned()]);
        let remembered = BTreeMap::from([("LGHUBUpdaterService".to_owned(), "auto".to_owned())]);
        let ctx = ViewContext {
            devices: &devices,
            visible: &devices,
            dont_ask: &dont_ask,
            remembered: &remembered,
            session: Some(1),
        };
        let json = serde_json::to_value(scan_view(&report, &ctx)).expect("serializes");
        let f = &json["findings"][0];
        assert_eq!(f["entry_id"], serde_json::json!("ghub"));
        assert_eq!(f["severity"], serde_json::json!("high"));
        assert_eq!(f["evidence"][0]["type"], serde_json::json!("process"));
        assert_eq!(f["evidence"][0]["data"]["pid"], serde_json::json!(42));
        assert_eq!(f["evidence"][1]["data"]["state"], serde_json::json!("running"));
        assert_eq!(f["affected_devices"][0]["name"], serde_json::json!("G560"));
        assert_eq!(json["errors"][0], serde_json::json!("services: denied"));
        assert_eq!(f["class"], serde_json::json!("vendor_tool"));
        assert_eq!(f["autostart"][0]["name"], serde_json::json!("LGHUB"));
        assert_eq!(f["autostart"][0]["enabled"], serde_json::json!(true));
        assert_eq!(f["dont_ask"], serde_json::json!(true));
        assert_eq!(f["autostart_on"], serde_json::json!(true));
        assert_eq!(
            f["can_reenable"],
            serde_json::json!(true),
            "remembered Manual service"
        );
        assert_eq!(
            json["startup"],
            serde_json::json!([]),
            "vendor tools never reach the dialog"
        );
        assert_eq!(
            f["close_targets"][0],
            serde_json::json!({ "kind": "process", "pid": 42, "name": "lghub.exe" })
        );
        assert_eq!(
            f["close_targets"][1],
            serde_json::json!({ "kind": "service", "name": "LGHUBUpdaterService" })
        );
    }
}
