//! Real Windows scan sources and the real [`SystemOps`]. The sources work without elevation; items the current user
//! cannot read are skipped. [`WindowsOps`] is the only code that changes the system and runs only for an action the
//! user clicked.

use std::path::Path;
use std::time::{Duration, SystemTime};

use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE, Type};

use crate::actions::ActionError;
use crate::autostart::{
    AutostartEntry, AutostartLocation, AutostartSource, command_executable, encode_startup_approved,
    filetime, parse_shell_link, parse_startup_approved,
};
use crate::resolve::SystemOps;
use crate::sources::{
    DynamicLighting, Hive, InstalledApp, ProcessInfo, ProcessSource, RegistrySource, ScanError, ServiceInfo,
    ServiceSource, StartType, expand_env,
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

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const RUN_WOW64: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE_WOW64: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce";
const USER_STARTUP: &str = r"%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup";
const COMMON_STARTUP: &str = r"%ProgramData%\Microsoft\Windows\Start Menu\Programs\StartUp";

/// Largest Startup folder file read to resolve a shortcut.
const MAX_LINK_SIZE: u64 = 1 << 20;

/// Run / RunOnce keys of HKCU, HKLM and HKLM `WOW6432Node` and the per-user and common Startup folders, each with
/// its `StartupApproved` state. Read-only.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsAutostart;

fn hive(machine: bool) -> &'static Key {
    if machine { LOCAL_MACHINE } else { CURRENT_USER }
}

/// The `StartupApproved` state of `name` at `location`; `None` when there is no value.
fn approved(location: AutostartLocation, name: &str) -> Option<bool> {
    let key = location.approval_key()?;
    let reg = hive(key.machine).open(key.path()).ok()?;
    let value = reg.get_value(name).ok()?;
    parse_startup_approved(&value)
}

fn read_run_key(root: &Key, path: &str, location: AutostartLocation, out: &mut Vec<AutostartEntry>) {
    let Ok(key) = root.open(path) else { return };
    let Ok(values) = key.values() else { return };
    for (name, value) in values {
        // REG_SZ and REG_EXPAND_SZ convert; other types are not commands.
        let Ok(command) = String::try_from(value) else {
            continue;
        };
        let executable = command_executable(&command)
            .map(|exe| expand_env(&exe, |v| std::env::var(v).ok()).unwrap_or(exe));
        out.push(AutostartEntry {
            approved: approved(location, &name),
            location,
            name,
            command,
            executable,
        });
    }
}

fn read_startup_folder(template: &str, location: AutostartLocation, out: &mut Vec<AutostartEntry>) {
    let Some(dir) = expand_env(template, |v| std::env::var(v).ok()) else {
        return;
    };
    let Ok(items) = std::fs::read_dir(&dir) else {
        return;
    };
    for item in items.flatten() {
        let path = item.path();
        if !path.is_file() {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let (command, executable) = startup_target(&path);
        out.push(AutostartEntry {
            approved: approved(location, &name),
            location,
            name,
            command,
            executable,
        });
    }
}

/// Command and executable of a Startup folder item: the shortcut target for `.lnk` files, the file itself otherwise.
fn startup_target(path: &Path) -> (String, Option<String>) {
    let own = path.display().to_string();
    let is_link = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
    if !is_link {
        return (own.clone(), Some(own));
    }
    let small = std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_LINK_SIZE);
    let link = small
        .then(|| std::fs::read(path).ok())
        .flatten()
        .and_then(|bytes| parse_shell_link(&bytes));
    match link.and_then(|l| l.target.map(|t| (t, l.arguments))) {
        Some((target, args)) => {
            let command = match args {
                Some(a) => format!("\"{target}\" {a}"),
                None => format!("\"{target}\""),
            };
            (command, Some(target))
        }
        None => (own, None),
    }
}

impl AutostartSource for WindowsAutostart {
    fn autostart_entries(&self) -> Result<Vec<AutostartEntry>, ScanError> {
        let mut out = Vec::new();
        read_run_key(CURRENT_USER, RUN, AutostartLocation::HkcuRun, &mut out);
        read_run_key(CURRENT_USER, RUN_ONCE, AutostartLocation::HkcuRunOnce, &mut out);
        read_run_key(LOCAL_MACHINE, RUN, AutostartLocation::HklmRun, &mut out);
        read_run_key(LOCAL_MACHINE, RUN_ONCE, AutostartLocation::HklmRunOnce, &mut out);
        read_run_key(LOCAL_MACHINE, RUN_WOW64, AutostartLocation::HklmRun32, &mut out);
        read_run_key(
            LOCAL_MACHINE,
            RUN_ONCE_WOW64,
            AutostartLocation::HklmRunOnce32,
            &mut out,
        );
        read_startup_folder(USER_STARTUP, AutostartLocation::UserStartupFolder, &mut out);
        read_startup_folder(COMMON_STARTUP, AutostartLocation::CommonStartupFolder, &mut out);
        Ok(out)
    }
}

/// How long [`WindowsOps::stop_service`] waits for a service to reach the stopped state.
pub const SERVICE_STOP_WAIT: Duration = Duration::from_secs(10);

/// The real [`SystemOps`]: checked process termination, SCM stop / start type, `StartupApproved` writes.
///
/// Call it only for actions the user clicked and that [`check_action`](crate::check_action) accepted. HKLM writes
/// and service changes need administrator rights (the elevated helper); HKCU writes and same-user terminations work
/// in the app process.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsOps;

impl SystemOps for WindowsOps {
    fn terminate_process(&self, pid: u32, name: &str, reason: &str) -> Result<(), ActionError> {
        sys::terminate_checked(pid, name, reason)
    }

    fn stop_service(&self, name: &str) -> Result<(), String> {
        sys::stop_service(name, SERVICE_STOP_WAIT)
    }

    fn set_service_start_type(&self, name: &str, start_type: StartType) -> Result<(), String> {
        match start_type {
            StartType::Auto => sys::set_service_start_type(name, true),
            StartType::Manual => sys::set_service_start_type(name, false),
            other => Err(format!("start type {other:?} is not allowed")),
        }
    }

    fn set_startup_approved(
        &self,
        location: AutostartLocation,
        name: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let key = location
            .approval_key()
            .ok_or_else(|| format!("{location:?} entries cannot be switched"))?;
        let value = encode_startup_approved(enabled, filetime(SystemTime::now()));
        hive(key.machine)
            .create(key.path())
            .and_then(|k| k.set_bytes(name, Type::Bytes, &value))
            .map_err(|e| format!("cannot write StartupApproved value {name:?}: {}", e.message()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Catalog, ScanOptions, Scanner};

    #[test]
    fn real_autostart_ok() {
        WindowsAutostart.autostart_entries().expect("autostart entries");
    }

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
