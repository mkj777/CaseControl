//! Conflict scan results as the Conflicts page shows them, and the checks that tie an action click back to the
//! finding it came from (so the frontend cannot ask to end an arbitrary pid or stop an arbitrary service).

use cc_conflicts::{AFFECTS_ALL, Evidence, Finding, ScanReport};
use cc_core::DeviceId;
use cc_engine::DeviceView;
use serde::Serialize;

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
}

/// Result of a conflict scan for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConflictScan {
    /// Findings, running first, then by descending severity.
    pub findings: Vec<FindingView>,
    /// Scan sources that failed.
    pub errors: Vec<String>,
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
pub fn scan_view(report: &ScanReport, devices: &[KnownDevice]) -> ConflictScan {
    ConflictScan {
        findings: report
            .findings
            .iter()
            .map(|f| FindingView {
                affected_devices: affected_devices(f, devices),
                finding: f.clone(),
            })
            .collect(),
        errors: report.errors.clone(),
    }
}

fn finding<'a>(report: &'a ScanReport, entry_id: &str) -> Result<&'a Finding, String> {
    report
        .findings
        .iter()
        .find(|f| f.entry_id == entry_id)
        .ok_or_else(|| format!("no finding {entry_id:?} in the last scan; scan again"))
}

/// The executable name of process `pid` as recorded in finding `entry_id` of the last scan.
pub fn process_name(report: &ScanReport, entry_id: &str, pid: u32) -> Result<String, String> {
    finding(report, entry_id)?
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::Process { pid: p, name, .. } if *p == pid => Some(name.clone()),
            _ => None,
        })
        .ok_or_else(|| format!("process {pid} is not part of finding {entry_id:?}"))
}

/// Checks that service or driver `name` is part of finding `entry_id` of the last scan.
pub fn check_service(report: &ScanReport, entry_id: &str, name: &str) -> Result<(), String> {
    let found = finding(report, entry_id)?.evidence.iter().any(|e| match e {
        Evidence::Service { name: n, .. } | Evidence::Driver { name: n, .. } => n.eq_ignore_ascii_case(name),
        _ => false,
    });
    if found {
        Ok(())
    } else {
        Err(format!("service {name:?} is not part of finding {entry_id:?}"))
    }
}

#[cfg(test)]
mod tests {
    use cc_conflicts::{Access, Category, ServiceKind, ServiceState, Severity};
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
                },
                Evidence::Service {
                    name: "LGHUBUpdaterService".into(),
                    state: ServiceState::Running,
                    kind: ServiceKind::Win32,
                },
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
    fn actions_must_match_the_last_scan() {
        let report = ScanReport {
            findings: vec![sample_finding(&[family::LOGITECH])],
            errors: vec![],
        };
        assert_eq!(process_name(&report, "ghub", 42).as_deref(), Ok("lghub.exe"));
        assert!(process_name(&report, "ghub", 43).is_err());
        assert!(process_name(&report, "other", 42).is_err());
        assert!(check_service(&report, "ghub", "lghubupdaterservice").is_ok());
        assert!(check_service(&report, "ghub", "Spooler").is_err());
    }

    #[test]
    fn view_flattens_finding() {
        let report = ScanReport {
            findings: vec![sample_finding(&[family::LOGITECH])],
            errors: vec!["services: denied".into()],
        };
        let json = serde_json::to_value(scan_view(&report, &devices())).expect("serializes");
        let f = &json["findings"][0];
        assert_eq!(f["entry_id"], serde_json::json!("ghub"));
        assert_eq!(f["severity"], serde_json::json!("high"));
        assert_eq!(f["evidence"][0]["type"], serde_json::json!("process"));
        assert_eq!(f["evidence"][0]["data"]["pid"], serde_json::json!(42));
        assert_eq!(f["evidence"][1]["data"]["state"], serde_json::json!("running"));
        assert_eq!(f["affected_devices"][0]["name"], serde_json::json!("G560"));
        assert_eq!(json["errors"][0], serde_json::json!("services: denied"));
    }
}
