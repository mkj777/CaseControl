//! Real Windows scan sources. All of them work without elevation; items the current user cannot read are skipped.

use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};

use crate::sources::{
    DynamicLighting, Hive, InstalledApp, ProcessInfo, ProcessSource, RegistrySource, ScanError, ServiceInfo,
    ServiceSource,
};
use crate::sys;

/// Processes from a Toolhelp snapshot; image paths via `QueryFullProcessImageNameW` where the process can be opened.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsProcesses;

impl ProcessSource for WindowsProcesses {
    fn processes(&self) -> Result<Vec<ProcessInfo>, ScanError> {
        Ok(sys::process_list()?
            .into_iter()
            .map(|(pid, name)| ProcessInfo {
                pid,
                exe_path: sys::process_image_path(pid),
                name,
            })
            .collect())
    }
}

/// Services and driver services from the service control manager (`EnumServicesStatusExW`).
///
/// Reading the configuration opens every service once with `SERVICE_QUERY_CONFIG`, a few hundred cheap calls.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsServices;

impl ServiceSource for WindowsServices {
    fn services(&self) -> Result<Vec<ServiceInfo>, ScanError> {
        sys::service_list()
    }
}

/// Uninstall keys and the Dynamic Lighting key, read with the `windows-registry` crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsRegistry;

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL_WOW64: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
const LIGHTING: &str = r"Software\Microsoft\Lighting";

fn read_uninstall(root: &Key, path: &str, hive: Hive, out: &mut Vec<InstalledApp>) {
    // A missing Uninstall key (common for HKCU on fresh machines) simply contributes nothing.
    let Ok(key) = root.open(path) else { return };
    let Ok(names) = key.keys() else { return };
    for name in names {
        let Ok(sub) = key.open(&name) else { continue };
        let Ok(display_name) = sub.get_string("DisplayName") else {
            continue;
        };
        if display_name.trim().is_empty() {
            continue;
        }
        let text = |value: &str| sub.get_string(value).ok().filter(|s| !s.is_empty());
        out.push(InstalledApp {
            display_name,
            version: text("DisplayVersion"),
            publisher: text("Publisher"),
            install_location: text("InstallLocation"),
            hive,
        });
    }
}

impl RegistrySource for WindowsRegistry {
    fn installed_apps(&self) -> Result<Vec<InstalledApp>, ScanError> {
        let mut out = Vec::new();
        read_uninstall(LOCAL_MACHINE, UNINSTALL, Hive::HklmNative, &mut out);
        read_uninstall(LOCAL_MACHINE, UNINSTALL_WOW64, Hive::HklmWow6432, &mut out);
        read_uninstall(CURRENT_USER, UNINSTALL, Hive::Hkcu, &mut out);
        Ok(out)
    }

    fn dynamic_lighting(&self) -> Result<Option<DynamicLighting>, ScanError> {
        let Ok(key) = CURRENT_USER.open(LIGHTING) else {
            return Ok(None);
        };
        let flag = |name: &str| key.get_u32(name).map(|v| v != 0).unwrap_or(false);

        // Providers are values named "1", "2", ... holding the provider id; the name is the priority.
        let mut providers: Vec<(u32, String)> = match key.open("Providers") {
            Ok(p) => p
                .values()
                .map_err(|e| ScanError::Registry(e.message()))?
                .filter_map(|(name, value)| {
                    let id = String::try_from(value).ok()?;
                    Some((name.parse().unwrap_or(u32::MAX), id))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        providers.sort();

        let device_count = key
            .open("Devices")
            .and_then(|d| d.keys().map(Iterator::count))
            .unwrap_or(0);
        Ok(Some(DynamicLighting {
            ambient_enabled: flag("AmbientLightingEnabled"),
            foreground_controls: flag("ControlledByForegroundApp"),
            providers: providers.into_iter().map(|(_, id)| id).collect(),
            device_count,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Catalog, ScanOptions, Scanner};

    #[test]
    fn real_processes_ok() {
        WindowsProcesses.processes().expect("process snapshot");
    }

    #[test]
    fn real_services_ok() {
        WindowsServices.services().expect("service enumeration");
    }

    #[test]
    fn real_registry_ok() {
        WindowsRegistry.installed_apps().expect("uninstall keys");
        WindowsRegistry.dynamic_lighting().expect("lighting key");
    }

    #[test]
    fn real_scan_ok() {
        let scanner = Scanner::new(
            Catalog::embedded(),
            WindowsProcesses,
            WindowsServices,
            WindowsRegistry,
        );
        let report = scanner.scan(&ScanOptions::default());
        // Errors are not asserted: CI runners may restrict SCM or registry access. Print them for the log.
        #[allow(clippy::print_stderr)]
        for e in &report.errors {
            eprintln!("real_scan_ok: scan source error: {e}");
        }
    }
}
