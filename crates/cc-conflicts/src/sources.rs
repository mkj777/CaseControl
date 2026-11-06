//! Scanner inputs: plain data types and the source traits the [`Scanner`](crate::Scanner) reads from.
//!
//! Real Windows implementations live in the private `platform` module (`WindowsProcesses`, `WindowsServices`,
//! `WindowsRegistry`); tests inject fakes.

use serde::{Deserialize, Serialize};

/// A scan source failed as a whole (individual inaccessible items are skipped, not reported).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// A Win32 call failed.
    #[error("{call} failed: {message}")]
    Win32 {
        /// The API that failed.
        call: &'static str,
        /// System error text.
        message: String,
    },
    /// Registry access failed.
    #[error("registry: {0}")]
    Registry(String),
    /// The source is not available on this platform.
    #[error("not supported on this platform")]
    Unsupported,
}

/// A running process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// Executable file name, e.g. `FanControl.exe`.
    pub name: String,
    /// Full image path, `None` when the process could not be opened (protected or other session).
    pub exe_path: Option<String>,
}

/// Whether a service is a user-mode service or a kernel driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    /// `SERVICE_WIN32_OWN_PROCESS` / `SERVICE_WIN32_SHARE_PROCESS`.
    Win32,
    /// `SERVICE_KERNEL_DRIVER` / `SERVICE_FILE_SYSTEM_DRIVER`.
    Driver,
}

/// Current state of a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// `SERVICE_RUNNING`.
    Running,
    /// `SERVICE_STOPPED`.
    Stopped,
    /// Any pending or paused state, with the raw `SERVICE_STATUS_CURRENT_STATE` value.
    Other(u32),
}

/// Configured start type of a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartType {
    /// Loaded by the boot loader.
    Boot,
    /// Loaded during kernel initialization.
    System,
    /// Started automatically.
    Auto,
    /// Started on demand.
    Manual,
    /// Disabled.
    Disabled,
}

/// A service or driver service registered with the service control manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Service key name.
    pub name: String,
    /// Display name.
    pub display_name: String,
    /// User-mode service or driver.
    pub kind: ServiceKind,
    /// Current state.
    pub state: ServiceState,
    /// Start type, `None` when the configuration could not be read.
    pub start_type: Option<StartType>,
    /// Binary path, `None` when the configuration could not be read.
    pub binary_path: Option<String>,
}

/// Which Uninstall key an installed app was found under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hive {
    /// `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall`.
    HklmNative,
    /// `HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall`.
    HklmWow6432,
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall`.
    Hkcu,
}

/// An entry of an Uninstall key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledApp {
    /// `DisplayName`.
    pub display_name: String,
    /// `DisplayVersion`.
    pub version: Option<String>,
    /// `Publisher`.
    pub publisher: Option<String>,
    /// `InstallLocation`.
    pub install_location: Option<String>,
    /// Where it was found.
    pub hive: Hive,
}

/// State of Windows Dynamic Lighting for the current user (`HKCU\Software\Microsoft\Lighting`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicLighting {
    /// `AmbientLightingEnabled` = 1: background control is on.
    pub ambient_enabled: bool,
    /// `ControlledByForegroundApp` = 1: the foreground app always wins.
    pub foreground_controls: bool,
    /// Registered background lighting providers in priority order (values of `Providers`).
    pub providers: Vec<String>,
    /// Number of LampArray devices Windows knows (`Devices` subkeys).
    pub device_count: usize,
}

/// Enumerates running processes.
pub trait ProcessSource {
    /// All processes visible to the current user.
    fn processes(&self) -> Result<Vec<ProcessInfo>, ScanError>;
}

/// Enumerates services and driver services.
pub trait ServiceSource {
    /// All services and drivers the current user may enumerate.
    fn services(&self) -> Result<Vec<ServiceInfo>, ScanError>;
}

/// Reads the registry locations the scanner needs.
pub trait RegistrySource {
    /// Entries of the three Uninstall keys.
    fn installed_apps(&self) -> Result<Vec<InstalledApp>, ScanError>;
    /// Dynamic Lighting state, `None` when the key does not exist.
    fn dynamic_lighting(&self) -> Result<Option<DynamicLighting>, ScanError>;
}

/// Resolves catalog install paths.
pub trait PathSource {
    /// Expands `%VAR%` placeholders in `template` and returns the expanded path if it exists.
    fn existing(&self, template: &str) -> Option<String>;
}

/// Checks whether the OpenRGB SDK server is listening.
pub trait OpenRgbPortProbe {
    /// `true` when something accepts connections on 127.0.0.1:6742.
    fn openrgb_listening(&self) -> bool;
}

/// [`PathSource`] backed by the process environment and the file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct FsPaths;

impl PathSource for FsPaths {
    fn existing(&self, template: &str) -> Option<String> {
        let expanded = expand_env(template, |name| std::env::var(name).ok())?;
        std::path::Path::new(&expanded).exists().then_some(expanded)
    }
}

/// Expands `%NAME%` placeholders using `lookup`. Returns `None` if a variable is unknown or a `%` is unmatched.
pub fn expand_env(template: &str, lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        let name = &after[..end];
        if name.is_empty() {
            out.push('%');
        } else {
            out.push_str(&lookup(name)?);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "LOCALAPPDATA" => Some(r"C:\Users\u\AppData\Local".to_owned()),
            "ProgramFiles(x86)" => Some(r"C:\Program Files (x86)".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn expands_variables() {
        assert_eq!(
            expand_env(r"%LOCALAPPDATA%\VortxEngine", env).as_deref(),
            Some(r"C:\Users\u\AppData\Local\VortxEngine")
        );
        assert_eq!(
            expand_env(r"%ProgramFiles(x86)%\MSI\MSI Center", env).as_deref(),
            Some(r"C:\Program Files (x86)\MSI\MSI Center")
        );
        assert_eq!(expand_env(r"C:\plain", env).as_deref(), Some(r"C:\plain"));
        assert_eq!(expand_env("100%%", env).as_deref(), Some("100%"));
    }

    #[test]
    fn unknown_or_unterminated_variables_fail() {
        assert_eq!(expand_env(r"%NOPE%\x", env), None);
        assert_eq!(expand_env(r"%LOCALAPPDATA\x", env), None);
    }
}
