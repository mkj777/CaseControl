//! Real Windows scan sources and the real [`SystemOps`]. The sources work without elevation; items the current user
//! cannot read are skipped. [`WindowsOps`] is the only code that changes the system and runs only for an action the
//! user clicked.

use std::path::Path;
use std::time::SystemTime;

use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE, Type};

use crate::actions::ActionError;
use crate::autostart::{
    AutostartEntry, AutostartLocation, AutostartSource, command_executable, encode_startup_approved,
    filetime, parse_shell_link, parse_startup_approved,
};
use crate::elevation::SERVICE_STOP_WAIT;
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
                session_id: sys::process_session(pid),
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

/// Largest Startup folder file read to resolve a shortcut.
const MAX_LINK_SIZE: u64 = 1 << 20;

/// Run / RunOnce keys of HKCU, HKLM and HKLM `WOW6432Node` and the per-user and common Startup folders, each with
/// its `StartupApproved` state. Read-only. The source of the normal (unelevated) scan: the common Startup folder is
/// listed wherever the known-folder API puts it. The elevated helper uses [`WindowsHelperAutostart`].
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsAutostart;

/// [`WindowsAutostart`] for the elevated helper: the common Startup folder is listed only when it is owned by SYSTEM,
/// Administrators or TrustedInstaller, because the helper acts on those items with
/// administrator rights while `%ProgramData%` comes from the user's environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsHelperAutostart;

fn hive(machine: bool) -> &'static Key {
    if machine { LOCAL_MACHINE } else { CURRENT_USER }
}

/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`: a key or value that does not exist.
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

/// Removes CaseControl's own legacy autostart entry: the HKCU `Run` value `name` and its HKCU
/// `StartupApproved\Run` value (ADR 0001 #42: the desktop app's autostart moved to a Task Scheduler logon task).
/// HKCU only, so no administrator rights are needed; nothing else is touched. A value or key that does not exist
/// counts as removed. Returns whether a value was removed.
pub fn remove_own_run_entry(name: &str) -> Result<bool, String> {
    let mut removed = false;
    let approval = AutostartLocation::HkcuRun
        .approval_key()
        .map(|k| k.path())
        .ok_or_else(|| "HKCU Run entries have no StartupApproved key".to_owned())?;
    for path in [RUN.to_owned(), approval] {
        let key = match CURRENT_USER.options().read().write().open(&path) {
            Ok(key) => key,
            Err(e) if e.code().0 == NOT_FOUND => continue,
            Err(e) => return Err(format!("cannot open HKCU\\{path}: {}", e.message())),
        };
        match key.remove_value(name) {
            Ok(()) => removed = true,
            Err(e) if e.code().0 == NOT_FOUND => {}
            Err(e) => return Err(format!("cannot remove HKCU\\{path}\\{name}: {}", e.message())),
        }
    }
    Ok(removed)
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

/// Whether `owner` is an owner only the machine's administrators can give a folder: SYSTEM, Administrators or
/// TrustedInstaller. A folder a standard user creates is owned by that user.
fn is_machine_owner(owner: &str) -> bool {
    use cc_system::acl::{ADMINISTRATORS_SID, SYSTEM_SID, TRUSTED_INSTALLER_SID};
    [SYSTEM_SID, ADMINISTRATORS_SID, TRUSTED_INSTALLER_SID]
        .iter()
        .any(|t| t.eq_ignore_ascii_case(owner))
}

/// The Startup folder `which`, resolved with `SHGetKnownFolderPath` instead of a `%ProgramData%` / `%APPDATA%`
/// template. The known-folder API still expands `%ProgramData%` from the process environment, which the user
/// controls also in the elevated helper (user environment variables), so with `helper` the common folder is accepted
/// only when its owner is SYSTEM, Administrators or TrustedInstaller: a folder the user made and pointed
/// `%ProgramData%` at is owned by the user and refused. Without `helper` (the normal scan, which acts on nothing
/// with administrator rights) the folder is used as resolved. `None` (logged) when it cannot be resolved or is
/// refused.
fn resolve_startup_folder(which: sys::StartupFolder, helper: bool) -> Option<std::path::PathBuf> {
    let Some(dir) = sys::startup_folder(which) else {
        tracing::warn!(
            ?which,
            "the Startup folder cannot be resolved; its items are not listed"
        );
        return None;
    };
    if helper && which == sys::StartupFolder::Common {
        match cc_system::file_owner_sid(&dir) {
            Ok(owner) if is_machine_owner(&owner) => {}
            other => {
                tracing::warn!(
                    dir = %dir.display(),
                    owner = ?other,
                    "the common Startup folder is not owned by the system or administrators; its items are not listed"
                );
                return None;
            }
        }
    }
    Some(dir)
}

/// Reads the Startup folder `which` (see [`resolve_startup_folder`]).
fn read_startup_folder(
    which: sys::StartupFolder,
    location: AutostartLocation,
    helper: bool,
    out: &mut Vec<AutostartEntry>,
) {
    let Some(dir) = resolve_startup_folder(which, helper) else {
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

/// Every autostart entry; `helper` as in [`resolve_startup_folder`].
fn autostart_entries(helper: bool) -> Vec<AutostartEntry> {
    {
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
        read_startup_folder(
            sys::StartupFolder::User,
            AutostartLocation::UserStartupFolder,
            helper,
            &mut out,
        );
        read_startup_folder(
            sys::StartupFolder::Common,
            AutostartLocation::CommonStartupFolder,
            helper,
            &mut out,
        );
        out
    }
}

impl AutostartSource for WindowsAutostart {
    fn autostart_entries(&self) -> Result<Vec<AutostartEntry>, ScanError> {
        Ok(autostart_entries(false))
    }
}

impl AutostartSource for WindowsHelperAutostart {
    fn autostart_entries(&self) -> Result<Vec<AutostartEntry>, ScanError> {
        Ok(autostart_entries(true))
    }
}

/// The real [`SystemOps`]: checked process termination, SCM stop / start type, `StartupApproved` writes, and the
/// start-up task actions of the elevated helper ([`crate::boot_task`] with [`crate::WindowsBootTaskEnv`]).
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

    fn register_boot_task(&self, ctx: &crate::HelperContext) -> Result<(), String> {
        crate::boot_task::register_boot_task(&crate::WindowsBootTaskEnv::default(), ctx)
    }

    fn remove_boot_task(&self, ctx: &crate::HelperContext) -> Result<(), String> {
        crate::boot_task::remove_boot_task(&crate::WindowsBootTaskEnv::default(), ctx)
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
    fn startup_folders_resolve() {
        let common = resolve_startup_folder(sys::StartupFolder::Common, true).expect("common startup");
        let user = resolve_startup_folder(sys::StartupFolder::User, true).expect("user startup");
        assert_eq!(
            resolve_startup_folder(sys::StartupFolder::Common, false).as_ref(),
            Some(&common)
        );
        let lower = |p: &std::path::Path| p.to_string_lossy().to_ascii_lowercase();
        for dir in [&common, &user] {
            assert!(dir.is_absolute(), "{}", dir.display());
            assert!(
                lower(dir).ends_with(r"\start menu\programs\startup"),
                "{}",
                dir.display()
            );
            assert!(!lower(dir).contains('%'), "{}", dir.display());
        }
    }

    #[test]
    fn machine_owners() {
        for sid in [
            "S-1-5-18",
            "S-1-5-32-544",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
        ] {
            assert!(is_machine_owner(sid), "{sid}");
        }
        for sid in ["S-1-5-21-1-2-3-1001", "S-1-5-32-545", "S-1-1-0", ""] {
            assert!(!is_machine_owner(sid), "{sid}");
        }
    }

    /// A user can point `%ProgramData%` at a folder of their own (user environment variables reach the elevated
    /// helper too), and the known-folder API follows it. The helper's scan must then not list that folder as the
    /// common Startup folder; the normal scan lists it as before (it acts on nothing with administrator rights). Runs this test again in a child process with `%ProgramData%` pointing at a folder this
    /// test creates (owned by the current user) under the temp folder. Nothing outside that folder is written.
    #[test]
    fn a_redirected_common_startup_folder_is_refused() {
        const CHILD: &str = "CC_TEST_STARTUP_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let raw = sys::startup_folder(sys::StartupFolder::Common);
            let accepted = resolve_startup_folder(sys::StartupFolder::Common, true);
            let normal = resolve_startup_folder(sys::StartupFolder::Common, false);
            #[allow(clippy::print_stdout)]
            {
                println!("RAW={:?}", raw);
                println!("ACCEPTED={:?}", accepted);
                println!("NORMAL={:?}", normal);
            }
            return;
        }
        let root = std::env::temp_dir().join(format!("cc-programdata-{}", std::process::id()));
        let startup = root.join(r"Microsoft\Windows\Start Menu\Programs\StartUp");
        std::fs::create_dir_all(&startup).expect("fake folder");
        let fake_owner = cc_system::file_owner_sid(&startup).expect("owner of the fake folder");
        let out = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .args([
                "--exact",
                "platform::tests::a_redirected_common_startup_folder_is_refused",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env("ProgramData", &root)
            .output()
            .expect("child test run");
        let _ = std::fs::remove_dir_all(&root);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let line = |key: &str| {
            stdout
                .lines()
                .find_map(|l| l.split_once(key).map(|(_, v)| v))
                .unwrap_or_else(|| panic!("child output: {stdout}"))
                .to_owned()
        };
        if is_machine_owner(&fake_owner) {
            // An elevated test run creates folders owned by Administrators; the case cannot be simulated then.
            return;
        }
        // The known-folder API follows the variable ...
        assert!(line("RAW=").contains("cc-programdata-"), "{stdout}");
        // ... the helper refuses the user's folder ...
        assert_eq!(line("ACCEPTED="), "None", "{stdout}");
        // ... and the normal scan still lists it.
        assert!(line("NORMAL=").contains("cc-programdata-"), "{stdout}");
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
