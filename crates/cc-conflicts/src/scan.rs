//! Matching scanner output against the catalog.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::autostart::{AutostartEntry, AutostartItem, AutostartSource, NoAutostart, file_name};
use crate::catalog::{
    Access, Catalog, Category, Class, DYNAMIC_LIGHTING_ENTRY, Entry, OPENRGB_ENTRY, Severity,
};
use crate::probe::OpenRgbProbe;
use crate::sources::{
    DynamicLighting, FsPaths, Hive, InstalledApp, OpenRgbPortProbe, PathSource, ProcessInfo, ProcessSource,
    RegistrySource, ServiceInfo, ServiceKind, ServiceSource, ServiceState,
};

/// Options for one scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanOptions {
    /// Probe TCP 127.0.0.1:6742 for the OpenRGB SDK server. Off by default (setting `conflicts.check_openrgb_port`).
    pub check_openrgb_port: bool,
}

/// One piece of evidence that ties the machine's state to a catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Evidence {
    /// A running process matched `processes`.
    Process {
        /// Process id.
        pid: u32,
        /// Executable name.
        name: String,
        /// Full image path if readable.
        path: Option<String>,
        /// Windows session of the process, `None` when unknown. Actions only end processes of the caller's session.
        #[serde(default)]
        session: Option<u32>,
    },
    /// A Win32 service matched `services`.
    Service {
        /// Service name.
        name: String,
        /// Current state.
        state: ServiceState,
        /// Always [`ServiceKind::Win32`] in practice; kept for the UI.
        kind: ServiceKind,
    },
    /// A driver service matched `drivers`.
    Driver {
        /// Driver service name.
        name: String,
        /// Current state; `Running` means the driver is loaded.
        state: ServiceState,
        /// Binary path if readable.
        path: Option<String>,
    },
    /// An Uninstall key entry matched `uninstall_names`.
    Installed {
        /// `DisplayName`.
        display_name: String,
        /// `DisplayVersion`.
        version: Option<String>,
        /// Which Uninstall key.
        hive: Hive,
    },
    /// An install path exists (expanded).
    Path(String),
    /// Windows Dynamic Lighting state; attached to the Dynamic Lighting entry and to entries listed as providers.
    DynamicLighting(DynamicLighting),
    /// The OpenRGB SDK server accepted a connection on 127.0.0.1:6742.
    OpenRgbPort,
}

impl Evidence {
    fn is_active(&self, entry_id: &str) -> bool {
        match self {
            Evidence::Process { .. } | Evidence::OpenRgbPort => true,
            Evidence::Service { state, .. } | Evidence::Driver { state, .. } => {
                *state == ServiceState::Running
            }
            Evidence::DynamicLighting(dl) => entry_id == DYNAMIC_LIGHTING_ENTRY && dl.ambient_enabled,
            Evidence::Installed { .. } | Evidence::Path(_) => false,
        }
    }

    fn is_install(&self) -> bool {
        match self {
            Evidence::Installed { .. }
            | Evidence::Path(_)
            | Evidence::Service { .. }
            | Evidence::Driver { .. } => true,
            Evidence::Process { .. } | Evidence::DynamicLighting(_) | Evidence::OpenRgbPort => false,
        }
    }
}

/// A catalog entry that was detected on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Catalog id.
    pub entry_id: String,
    /// Product name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Kind of software.
    pub category: Category,
    /// Suite or vendor tool (decides whether it can appear in the start dialog).
    pub class: Class,
    /// Severity from the catalog.
    pub severity: Severity,
    /// Whether the catalog names are verified.
    pub verified: bool,
    /// Hardware access paths.
    pub access: Vec<Access>,
    /// Affected family ids or `"all"`.
    pub affects: Vec<String>,
    /// Catalog note for the UI.
    pub note: String,
    /// Something is active right now: a process, a running service or loaded driver, the OpenRGB port, or
    /// (for Dynamic Lighting itself) ambient control being on.
    pub running: bool,
    /// Installed: an Uninstall entry, an install path, or a registered service/driver exists.
    pub installed: bool,
    /// Everything that matched.
    pub evidence: Vec<Evidence>,
    /// How it starts with Windows: matched Run / RunOnce values and Startup folder items, and the start type of its
    /// Win32 services.
    pub autostart: Vec<AutostartItem>,
}

/// Result of one scan. A failing source adds a line to `errors`; the other sources still contribute findings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    /// Findings, running first, then by descending severity, then by id.
    pub findings: Vec<Finding>,
    /// Human-readable errors of sources that failed.
    pub errors: Vec<String>,
}

/// Matches the scan sources against a catalog.
pub struct Scanner {
    catalog: Catalog,
    processes: Box<dyn ProcessSource + Send + Sync>,
    services: Box<dyn ServiceSource + Send + Sync>,
    registry: Box<dyn RegistrySource + Send + Sync>,
    paths: Box<dyn PathSource + Send + Sync>,
    openrgb: Box<dyn OpenRgbPortProbe + Send + Sync>,
    autostart: Box<dyn AutostartSource + Send + Sync>,
}

impl fmt::Debug for Scanner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scanner")
            .field("entries", &self.catalog.entries().len())
            .finish_non_exhaustive()
    }
}

impl Scanner {
    /// Creates a scanner over the given sources. Install paths are checked on the real file system and the
    /// OpenRGB probe is the real TCP probe; replace them with [`Scanner::with_paths`] / [`Scanner::with_openrgb_probe`].
    /// No autostart entries are read until [`Scanner::with_autostart`] supplies a source.
    pub fn new(
        catalog: Catalog,
        processes: impl ProcessSource + Send + Sync + 'static,
        services: impl ServiceSource + Send + Sync + 'static,
        registry: impl RegistrySource + Send + Sync + 'static,
    ) -> Self {
        Self {
            catalog,
            processes: Box::new(processes),
            services: Box::new(services),
            registry: Box::new(registry),
            paths: Box::new(FsPaths),
            openrgb: Box::new(OpenRgbProbe),
            autostart: Box::new(NoAutostart),
        }
    }

    /// Replaces the autostart source.
    pub fn with_autostart(mut self, autostart: impl AutostartSource + Send + Sync + 'static) -> Self {
        self.autostart = Box::new(autostart);
        self
    }

    /// Replaces the install-path resolver.
    pub fn with_paths(mut self, paths: impl PathSource + Send + Sync + 'static) -> Self {
        self.paths = Box::new(paths);
        self
    }

    /// Replaces the OpenRGB port probe.
    pub fn with_openrgb_probe(mut self, probe: impl OpenRgbPortProbe + Send + Sync + 'static) -> Self {
        self.openrgb = Box::new(probe);
        self
    }

    /// The catalog this scanner matches against.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Runs all sources and matches them against the catalog.
    pub fn scan(&self, opts: &ScanOptions) -> ScanReport {
        let mut errors = Vec::new();
        let processes = collect("processes", self.processes.processes(), &mut errors).unwrap_or_default();
        let services = collect("services", self.services.services(), &mut errors).unwrap_or_default();
        let installed =
            collect("installed apps", self.registry.installed_apps(), &mut errors).unwrap_or_default();
        let lighting = collect("dynamic lighting", self.registry.dynamic_lighting(), &mut errors).flatten();
        let openrgb_port = opts.check_openrgb_port && self.openrgb.openrgb_listening();
        let autostart =
            collect("autostart", self.autostart.autostart_entries(), &mut errors).unwrap_or_default();

        let input = Input {
            processes: &processes,
            services: &services,
            installed: &installed,
            lighting: lighting.as_ref(),
            openrgb_port,
            paths: self.paths.as_ref(),
            autostart: &autostart,
        };
        let mut findings: Vec<Finding> = self
            .catalog
            .entries()
            .iter()
            .filter_map(|entry| match_entry(entry, &input))
            .collect();
        sort_findings(&mut findings);
        tracing::debug!(
            processes = processes.len(),
            services = services.len(),
            installed = installed.len(),
            autostart = autostart.len(),
            findings = findings.len(),
            errors = errors.len(),
            "conflict scan finished"
        );
        ScanReport { findings, errors }
    }
}

fn collect<T>(what: &str, result: Result<T, crate::ScanError>, errors: &mut Vec<String>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(source = what, error = %e, "conflict scan source failed");
            errors.push(format!("{what}: {e}"));
            None
        }
    }
}

struct Input<'a> {
    processes: &'a [ProcessInfo],
    services: &'a [ServiceInfo],
    installed: &'a [InstalledApp],
    lighting: Option<&'a DynamicLighting>,
    openrgb_port: bool,
    paths: &'a (dyn PathSource + Send + Sync),
    autostart: &'a [AutostartEntry],
}

fn push_unique(evidence: &mut Vec<Evidence>, item: Evidence) {
    if !evidence.contains(&item) {
        evidence.push(item);
    }
}

fn match_entry(entry: &Entry, input: &Input<'_>) -> Option<Finding> {
    let mut evidence = Vec::new();
    let mut autostart = Vec::new();

    for p in input.processes {
        if entry.matches_executable(&p.name, p.exe_path.as_deref()) {
            push_unique(
                &mut evidence,
                Evidence::Process {
                    pid: p.pid,
                    name: p.name.clone(),
                    path: p.exe_path.clone(),
                    session: p.session_id,
                },
            );
        }
    }
    for s in input.services {
        let matches = |patterns: &[String]| {
            patterns
                .iter()
                .any(|pat| glob_match(pat, &s.name) || glob_match(pat, &s.display_name))
        };
        match s.kind {
            ServiceKind::Win32 if matches(&entry.services) => {
                push_unique(
                    &mut evidence,
                    Evidence::Service {
                        name: s.name.clone(),
                        state: s.state,
                        kind: s.kind,
                    },
                );
                let item = AutostartItem::service(&s.name, s.start_type);
                if !autostart.contains(&item) {
                    autostart.push(item);
                }
            }
            ServiceKind::Driver if matches(&entry.drivers) => push_unique(
                &mut evidence,
                Evidence::Driver {
                    name: s.name.clone(),
                    state: s.state,
                    path: s.binary_path.clone(),
                },
            ),
            _ => {}
        }
    }
    for app in input.installed {
        if entry
            .uninstall_names
            .iter()
            .any(|n| contains_ignore_case(&app.display_name, n))
        {
            push_unique(
                &mut evidence,
                Evidence::Installed {
                    display_name: app.display_name.clone(),
                    version: app.version.clone(),
                    hive: app.hive,
                },
            );
        }
    }
    for template in &entry.install_paths {
        if let Some(path) = input.paths.existing(template) {
            push_unique(&mut evidence, Evidence::Path(path));
        }
    }
    if let Some(dl) = input.lighting {
        let is_provider = dl.providers.iter().any(|p| {
            entry
                .lighting_providers
                .iter()
                .any(|prefix| starts_with_ignore_case(p, prefix))
        });
        if entry.id == DYNAMIC_LIGHTING_ENTRY || is_provider {
            push_unique(&mut evidence, Evidence::DynamicLighting(dl.clone()));
        }
    }
    if input.openrgb_port && entry.id == OPENRGB_ENTRY {
        push_unique(&mut evidence, Evidence::OpenRgbPort);
    }
    for item in input.autostart {
        let matched = item
            .executable
            .as_deref()
            .is_some_and(|exe| entry.matches_executable(file_name(exe), Some(exe)));
        if matched {
            let item = AutostartItem::from_entry(item);
            if !autostart.contains(&item) {
                autostart.push(item);
            }
        }
    }

    if evidence.is_empty() && autostart.is_empty() {
        return None;
    }
    Some(Finding {
        entry_id: entry.id.clone(),
        name: entry.name.clone(),
        vendor: entry.vendor.clone(),
        category: entry.category,
        class: entry.class,
        severity: entry.severity,
        verified: entry.verified,
        access: entry.access.clone(),
        affects: entry.affects.clone(),
        note: entry.note.clone(),
        running: evidence.iter().any(|e| e.is_active(&entry.id)),
        installed: !autostart.is_empty() || evidence.iter().any(Evidence::is_install),
        evidence,
        autostart,
    })
}

/// Sorts running findings first, then by descending severity, then by id.
pub fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.running
            .cmp(&a.running)
            .then(b.severity.cmp(&a.severity))
            .then_with(|| a.entry_id.cmp(&b.entry_id))
    });
}

/// Case-insensitive wildcard match; `*` matches any run of characters (including none). No other metacharacters.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    // Iterative wildcard matching with single-star backtracking.
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.to_lowercase().starts_with(&prefix.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autostart::AutostartLocation;
    use crate::sources::{ScanError, StartType};

    #[test]
    fn glob_is_case_insensitive_with_suffix_wildcard() {
        assert!(glob_match("SignalRgb.Service", "signalrgb.service"));
        assert!(glob_match("CorsairLLAccess*", "CorsairLLAccess64"));
        assert!(glob_match("CorsairLLAccess*", "corsairllaccess"));
        assert!(!glob_match("CorsairLLAccess*", "CorsairLLA"));
        assert!(!glob_match("FanControl.Service", "FanControl.Service2"));
        assert!(glob_match(
            "logi_lamparray_service*.exe",
            "logi_lamparray_service.AMD64.exe"
        ));
        assert!(glob_match("Aac*Hal_x86.exe", "AacKbHal_x86.exe"));
        assert!(!glob_match("Aac*Hal_x86.exe", "AacKbHal_x64.exe"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("a", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
    }

    // ---- fake sources reproducing the development machine (docs/research/00) ----

    struct FakeProcesses(Vec<ProcessInfo>);
    impl ProcessSource for FakeProcesses {
        fn processes(&self) -> Result<Vec<ProcessInfo>, ScanError> {
            Ok(self.0.clone())
        }
    }

    struct FakeServices(Vec<ServiceInfo>);
    impl ServiceSource for FakeServices {
        fn services(&self) -> Result<Vec<ServiceInfo>, ScanError> {
            Ok(self.0.clone())
        }
    }

    struct FakeRegistry {
        apps: Vec<InstalledApp>,
        lighting: Option<DynamicLighting>,
    }
    impl RegistrySource for FakeRegistry {
        fn installed_apps(&self) -> Result<Vec<InstalledApp>, ScanError> {
            Ok(self.apps.clone())
        }
        fn dynamic_lighting(&self) -> Result<Option<DynamicLighting>, ScanError> {
            Ok(self.lighting.clone())
        }
    }

    struct FailingServices;
    impl ServiceSource for FailingServices {
        fn services(&self) -> Result<Vec<ServiceInfo>, ScanError> {
            Err(ScanError::Win32 {
                call: "OpenSCManagerW",
                message: "access denied".to_owned(),
            })
        }
    }

    struct NoPaths;
    impl PathSource for NoPaths {
        fn existing(&self, _template: &str) -> Option<String> {
            None
        }
    }

    struct FakePaths;
    impl PathSource for FakePaths {
        fn existing(&self, template: &str) -> Option<String> {
            crate::sources::expand_env(template, |v| {
                (v == "LOCALAPPDATA").then(|| r"C:\Users\max\AppData\Local".to_owned())
            })
            .filter(|p| p.ends_with("VortxEngine"))
        }
    }

    struct Port(bool);
    impl OpenRgbPortProbe for Port {
        fn openrgb_listening(&self) -> bool {
            self.0
        }
    }

    fn process(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_owned(),
            exe_path: None,
            session_id: Some(1),
        }
    }

    fn service(name: &str, kind: ServiceKind, state: ServiceState) -> ServiceInfo {
        ServiceInfo {
            name: name.to_owned(),
            display_name: name.to_owned(),
            kind,
            state,
            start_type: Some(StartType::Auto),
            binary_path: (kind == ServiceKind::Driver)
                .then(|| format!(r"C:\WINDOWS\system32\drivers\{name}.sys")),
        }
    }

    fn app(name: &str, version: Option<&str>, hive: Hive) -> InstalledApp {
        InstalledApp {
            display_name: name.to_owned(),
            version: version.map(str::to_owned),
            publisher: None,
            install_location: None,
            hive,
        }
    }

    fn inventory() -> (FakeProcesses, FakeServices, FakeRegistry) {
        use ServiceKind::{Driver, Win32};
        use ServiceState::{Running, Stopped};
        let processes = FakeProcesses(vec![
            process(18004, "FanControl.exe"),
            process(5572, "lghub_updater.exe"),
            process(5464, "logi_lamparray_service.AMD64.exe"),
            process(4, "System"),
            process(1234, "explorer.exe"),
        ]);
        let services = FakeServices(vec![
            service("FanControl.Service", Win32, Stopped),
            service("SignalRgb.Service", Win32, Stopped),
            service("LGHUBUpdaterService", Win32, Running),
            service("logi_lamparray_service", Win32, Running),
            service("inpoutx64", Driver, Running),
            service("PawnIO", Driver, Running),
            service("SignalRgbDriver", Driver, Running),
            service("Dnscache", Win32, Running),
        ]);
        let registry = FakeRegistry {
            apps: vec![
                app("SignalRgb", Some("2.5.84"), Hive::Hkcu),
                app("FanControl", Some("285"), Hive::HklmNative),
                app("PawnIO", Some("2.0.1"), Hive::HklmNative),
                app("Logitech G HUB", None, Hive::HklmNative),
                app("SteelSeries GG", None, Hive::HklmNative),
                app("7-Zip", None, Hive::HklmNative),
            ],
            lighting: Some(DynamicLighting {
                ambient_enabled: false,
                foreground_controls: false,
                providers: vec!["SignalRGB_4n6rtbnm9e9y2".to_owned(), "WindowsLighting".to_owned()],
                device_count: 3,
            }),
        };
        (processes, services, registry)
    }

    fn scanner() -> Scanner {
        let (p, s, r) = inventory();
        Scanner::new(Catalog::embedded(), p, s, r)
            .with_paths(FakePaths)
            .with_openrgb_probe(Port(true))
    }

    fn find<'a>(report: &'a ScanReport, id: &str) -> &'a Finding {
        report
            .findings
            .iter()
            .find(|f| f.entry_id == id)
            .unwrap_or_else(|| panic!("no finding {id}"))
    }

    #[test]
    fn signalrgb_is_running_through_its_driver_and_is_a_lighting_provider() {
        let report = scanner().scan(&ScanOptions::default());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let f = find(&report, "signalrgb");
        assert!(f.running);
        assert!(f.installed);
        assert_eq!(f.severity, Severity::High);
        assert!(f.evidence.iter().any(|e| matches!(
            e,
            Evidence::Driver { name, state: ServiceState::Running, path: Some(_) } if name == "SignalRgbDriver"
        )));
        assert!(f.evidence.iter().any(|e| matches!(
            e,
            Evidence::Service { name, state: ServiceState::Stopped, .. } if name == "SignalRgb.Service"
        )));
        assert!(f.evidence.iter().any(|e| matches!(
            e,
            Evidence::Installed { display_name, version: Some(v), hive: Hive::Hkcu }
                if display_name == "SignalRgb" && v == "2.5.84"
        )));
        assert!(f.evidence.iter().any(|e| matches!(
            e,
            Evidence::DynamicLighting(dl) if dl.providers.iter().any(|p| p == "SignalRGB_4n6rtbnm9e9y2")
        )));
        assert!(f.evidence.contains(&Evidence::Path(
            r"C:\Users\max\AppData\Local\VortxEngine".to_owned()
        )));
    }

    #[test]
    fn drivers_and_cooperative_tools_get_catalog_severity() {
        let report = scanner().scan(&ScanOptions::default());
        let inpout = find(&report, "inpoutx64");
        assert_eq!(inpout.severity, Severity::High);
        assert!(inpout.running);

        let pawnio = find(&report, "pawnio");
        assert_eq!(pawnio.severity, Severity::Info);
        assert!(pawnio.running);
        assert!(pawnio.installed);

        let fan = find(&report, "fancontrol");
        assert!(fan.running);
        assert_eq!(fan.severity, Severity::Info);
        assert!(
            fan.evidence
                .iter()
                .any(|e| matches!(e, Evidence::Process { pid: 18004, .. }))
        );
        assert!(fan.evidence.iter().any(
            |e| matches!(e, Evidence::Service { name, state: ServiceState::Stopped, .. } if name == "FanControl.Service")
        ));
    }

    #[test]
    fn logitech_dynamic_lighting_and_steelseries() {
        let report = scanner().scan(&ScanOptions::default());
        let ghub = find(&report, "logitech_g_hub");
        assert!(ghub.running);
        assert!(
            ghub.evidence
                .iter()
                .any(|e| matches!(e, Evidence::Process { pid: 5464, .. }))
        );
        assert!(ghub.evidence.iter().any(
            |e| matches!(e, Evidence::Service { name, state: ServiceState::Running, .. } if name == "LGHUBUpdaterService")
        ));

        let dl = find(&report, DYNAMIC_LIGHTING_ENTRY);
        assert!(!dl.running, "ambient lighting is off");
        assert!(matches!(&dl.evidence[..], [Evidence::DynamicLighting(d)] if d.device_count == 3));

        let gg = find(&report, "steelseries_gg");
        assert!(!gg.running);
        assert!(gg.installed);

        // Nothing matches entries for software that is absent.
        assert!(report.findings.iter().all(|f| f.entry_id != "corsair_icue"));
        assert!(report.findings.iter().all(|f| f.entry_id != "winring0"));
    }

    #[test]
    fn findings_sort_running_first_then_severity() {
        let report = scanner().scan(&ScanOptions::default());
        let order: Vec<(bool, Severity, &str)> = report
            .findings
            .iter()
            .map(|f| (f.running, f.severity, f.entry_id.as_str()))
            .collect();
        let mut sorted = order.clone();
        sorted.sort_by_key(|o| (std::cmp::Reverse(o.0), std::cmp::Reverse(o.1), o.2));
        assert_eq!(order, sorted);
        let ids: Vec<&str> = order.iter().map(|o| o.2).collect();
        assert_eq!(
            ids,
            [
                "inpoutx64",
                "logitech_g_hub",
                "signalrgb",
                "fancontrol",
                "pawnio",
                "steelseries_gg",
                "windows_dynamic_lighting",
            ]
        );
    }

    #[test]
    fn openrgb_port_only_when_enabled() {
        let report = scanner().scan(&ScanOptions::default());
        assert!(report.findings.iter().all(|f| f.entry_id != OPENRGB_ENTRY));

        let report = scanner().scan(&ScanOptions {
            check_openrgb_port: true,
        });
        let f = find(&report, OPENRGB_ENTRY);
        assert!(f.running);
        assert_eq!(f.evidence, [Evidence::OpenRgbPort]);
    }

    #[test]
    fn service_matching_uses_glob_case_and_kind() {
        use ServiceKind::{Driver, Win32};
        use ServiceState::Running;
        let services = FakeServices(vec![
            service("corsairllaccess64", Driver, Running),
            service("STEELSERIESGGUPDATESERVICEPROXY", Win32, ServiceState::Stopped),
            // A Win32 service with a driver's name must not count as the driver, and vice versa.
            service("WinRing0x64", Win32, Running),
            service("fancontrol.service", Driver, Running),
        ]);
        let registry = FakeRegistry {
            apps: vec![],
            lighting: None,
        };
        let report = Scanner::new(Catalog::embedded(), FakeProcesses(vec![]), services, registry)
            .with_paths(NoPaths)
            .scan(&ScanOptions::default());
        let corsair = find(&report, "corsair_icue");
        assert!(corsair.running);
        assert!(matches!(&corsair.evidence[..], [Evidence::Driver { .. }]));
        let gg = find(&report, "steelseries_gg");
        assert!(!gg.running);
        assert!(report.findings.iter().all(|f| f.entry_id != "winring0"));
        assert!(report.findings.iter().all(|f| f.entry_id != "fancontrol"));
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.entry_id != DYNAMIC_LIGHTING_ENTRY)
        );
    }

    #[test]
    fn display_name_matches_too() {
        let mut svc = service("RzSvcX", ServiceKind::Win32, ServiceState::Running);
        svc.display_name = "Razer Game Manager Service 3".to_owned();
        let report = Scanner::new(
            Catalog::embedded(),
            FakeProcesses(vec![]),
            FakeServices(vec![svc]),
            FakeRegistry {
                apps: vec![],
                lighting: None,
            },
        )
        .with_paths(NoPaths)
        .scan(&ScanOptions::default());
        assert!(find(&report, "razer_synapse").running);
    }

    #[test]
    fn failing_source_is_reported_and_others_still_match() {
        let (p, _, r) = inventory();
        let report = Scanner::new(Catalog::embedded(), p, FailingServices, r)
            .with_paths(NoPaths)
            .scan(&ScanOptions::default());
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].starts_with("services:"));
        assert!(find(&report, "fancontrol").running);
        assert!(report.findings.iter().all(|f| f.entry_id != "inpoutx64"));
    }

    struct FakeAutostart(Vec<AutostartEntry>);
    impl AutostartSource for FakeAutostart {
        fn autostart_entries(&self) -> Result<Vec<AutostartEntry>, ScanError> {
            Ok(self.0.clone())
        }
    }

    fn run_entry(
        location: AutostartLocation,
        name: &str,
        command: &str,
        approved: Option<bool>,
    ) -> AutostartEntry {
        AutostartEntry {
            location,
            name: name.to_owned(),
            command: command.to_owned(),
            executable: crate::autostart::command_executable(command),
            approved,
        }
    }

    /// The development machine's autostart entries (2026-11-16) plus an OpenRGB Startup folder shortcut.
    fn autostart() -> FakeAutostart {
        use AutostartLocation::{CommonStartupFolder, HkcuRun, HklmRun};
        FakeAutostart(vec![
            run_entry(
                HkcuRun,
                "SignalRgb",
                r#""C:\Users\max\AppData\Local\VortxEngine\SignalRgbLauncher.exe" --silent"#,
                Some(false),
            ),
            run_entry(
                HkcuRun,
                "LGHUB",
                r#""C:\Program Files\LGHUB\system_tray\lghub_system_tray.exe" --minimized"#,
                None,
            ),
            run_entry(
                HkcuRun,
                "Discord",
                r#""C:\Users\max\AppData\Local\Discord\Update.exe" --processStart Discord.exe"#,
                Some(false),
            ),
            run_entry(
                HklmRun,
                "SteelSeriesGG",
                r#""C:\Program Files\SteelSeries\GG\SteelSeriesGG.exe" -dataPath="C:\ProgramData\SteelSeries\GG""#,
                Some(false),
            ),
            run_entry(
                CommonStartupFolder,
                "OpenRGB.lnk",
                r#""C:\Program Files\OpenRGB\OpenRGB.exe" --startminimized"#,
                Some(true),
            ),
        ])
    }

    #[test]
    fn autostart_entries_attach_to_their_findings() {
        use ServiceKind::Win32;
        let services = FakeServices(vec![
            ServiceInfo {
                start_type: Some(StartType::Auto),
                ..service("OpenRGB", Win32, ServiceState::Running)
            },
            ServiceInfo {
                start_type: Some(StartType::Auto),
                ..service("SignalRgb.Service", Win32, ServiceState::Stopped)
            },
        ]);
        let report = Scanner::new(
            Catalog::embedded(),
            FakeProcesses(vec![]),
            services,
            FakeRegistry {
                apps: vec![],
                lighting: None,
            },
        )
        .with_paths(NoPaths)
        .with_autostart(autostart())
        .scan(&ScanOptions::default());
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let signal = find(&report, "signalrgb");
        assert_eq!(signal.class, Class::Suite);
        assert!(!signal.running);
        assert!(signal.installed);
        let run = signal
            .autostart
            .iter()
            .find(|i| i.location == AutostartLocation::HkcuRun)
            .expect("run entry");
        assert_eq!(run.name, "SignalRgb");
        assert!(!run.enabled, "switched off in StartupApproved");
        assert!(run.toggleable && !run.needs_admin);
        let svc = signal
            .autostart
            .iter()
            .find(|i| i.location == AutostartLocation::Service)
            .expect("service item");
        assert!(svc.enabled && svc.needs_admin);
        assert_eq!(svc.start_type, Some(StartType::Auto));

        let ghub = find(&report, "logitech_g_hub");
        assert_eq!(ghub.class, Class::VendorTool);
        assert!(matches!(&ghub.autostart[..], [i] if i.name == "LGHUB" && i.enabled));

        let openrgb = find(&report, OPENRGB_ENTRY);
        assert!(openrgb.running, "service running");
        let names: Vec<(&str, bool)> = openrgb
            .autostart
            .iter()
            .map(|i| (i.name.as_str(), i.needs_admin))
            .collect();
        assert_eq!(names, [("OpenRGB", true), ("OpenRGB.lnk", true)]);

        let gg = find(&report, "steelseries_gg");
        assert!(matches!(&gg.autostart[..], [i] if !i.enabled && i.needs_admin));

        // Nothing else matched Discord.
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.autostart.iter().all(|i| i.name != "Discord"))
        );
    }

    #[test]
    fn generic_process_names_need_their_path() {
        let processes = FakeProcesses(vec![
            process(1, "CMService.exe"),
            ProcessInfo {
                exe_path: Some(r"C:\Program Files\Razer\GameManagerService.exe".into()),
                ..process(2, "GameManagerService.exe")
            },
        ]);
        let report = Scanner::new(
            Catalog::embedded(),
            processes,
            FakeServices(vec![]),
            FakeRegistry {
                apps: vec![],
                lighting: None,
            },
        )
        .with_paths(NoPaths)
        .with_autostart(FakeAutostart(vec![run_entry(
            AutostartLocation::HkcuRun,
            "Other",
            r#""C:\Tools\RunLed.exe""#,
            None,
        )]))
        .scan(&ScanOptions::default());
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.entry_id != "coolermaster_masterplus")
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.entry_id != "gigabyte_rgb_fusion")
        );
        let razer = find(&report, "razer_synapse");
        assert!(matches!(
            &razer.evidence[..],
            [Evidence::Process {
                pid: 2,
                session: Some(1),
                ..
            }]
        ));
    }

    #[test]
    fn report_serializes() {
        let report = scanner().scan(&ScanOptions {
            check_openrgb_port: true,
        });
        let text = toml::to_string(&ScanReport {
            findings: report.findings.clone(),
            errors: vec![],
        });
        assert!(text.is_ok(), "{text:?}");
    }
}
