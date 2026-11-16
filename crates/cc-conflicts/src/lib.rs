//! Data layer of CaseControl's conflict view and start dialog.
//!
//! What this crate does:
//! - Ships an embedded, validated [`Catalog`] of software that competes for RGB devices or low-level hardware
//!   access (RGB suites, vendor RGB tools, hardware monitors, fan control, kernel drivers, Windows Dynamic Lighting).
//!   Every entry is a [`Class::Suite`] (may appear in the start dialog) or a [`Class::VendorTool`].
//! - Detects that software without elevation through scan sources behind traits ([`ProcessSource`],
//!   [`ServiceSource`], [`RegistrySource`], [`PathSource`], [`OpenRgbPortProbe`], [`AutostartSource`]); the Windows
//!   implementations are `WindowsProcesses`, `WindowsServices`, `WindowsRegistry` and `WindowsAutostart`.
//! - Matches the scan against the catalog in [`Scanner::scan`] and returns [`Finding`]s with their [`Evidence`] and
//!   [`AutostartItem`]s (Run / RunOnce values, Startup folder items, service start types, each with an enabled
//!   flag).
//! - Offers opt-in actions for an explicit UI click (ADR 0001 #12, #31): [`terminate_process`],
//!   [`request_stop_service`] and the [`Action`]s of [`close_actions`] / [`autostart_actions`], run through
//!   [`SystemOps`] (`WindowsOps` on Windows). Everything that needs administrator rights runs in one elevated helper
//!   invocation of the app's own executable ([`run_elevated_helper`]), which re-validates each action against a
//!   fresh scan ([`run_elevated_request`]).
//!
//! What it deliberately does not do:
//! - No action without a user click, and no action on anything a scan does not tie to a catalog entry.
//! - Autostart is only ever switched reversibly: `StartupApproved` values (as Task Manager does) and service start
//!   type Auto <-> Manual. It never deletes Run values, services or files, never disables a service, never changes
//!   drivers, never uninstalls software, and runs no shell in the elevated helper.
//! - No kernel driver, no `EnumDeviceDrivers` (useless unelevated on Windows 11 24H2); loaded drivers are seen
//!   through the service control manager.
//! - No network: the only socket is the optional loopback probe of the OpenRGB SDK port 6742, off by default
//!   ([`ScanOptions::check_openrgb_port`], ADR 0001 #13).
//! - Store/MSIX packages and their startup tasks are not enumerated, so Store-only apps (HyperX NGENUITY) are only
//!   partly detectable. Scheduled tasks are not read.
//!
//! All `unsafe` code lives in the private `sys` module (Windows only).

mod actions;
mod autostart;
mod catalog;
mod elevation;
#[cfg(windows)]
mod platform;
mod probe;
mod resolve;
mod scan;
mod sources;
#[cfg(windows)]
#[allow(unsafe_code)]
mod sys;

pub use actions::{ActionError, request_stop_service, terminate_process};
pub use autostart::{
    ApprovalKey, AutostartEntry, AutostartItem, AutostartLocation, AutostartSource, NoAutostart,
    STARTUP_APPROVED, ShellLink, command_executable, encode_startup_approved, file_name, filetime,
    parse_shell_link, parse_startup_approved,
};
pub use catalog::{
    AFFECTS_ALL, Access, Catalog, CatalogError, Category, Class, DYNAMIC_LIGHTING_ENTRY, Entry,
    OPENRGB_ENTRY, Severity,
};
/// Device family ids the catalog's `affects` lists refer to (re-exported from `cc-core`).
pub use cc_core::family;
pub use elevation::{
    ELEVATED_ACTIONS_FLAG, ElevationError, HELPER_TIMEOUT, helper_parameters, helper_request_path,
    is_helper_invocation, result_path, run_elevated_helper,
};
#[cfg(windows)]
pub use platform::{
    SERVICE_STOP_WAIT, WindowsAutostart, WindowsOps, WindowsProcesses, WindowsRegistry, WindowsServices,
};
pub use probe::{OPENRGB_SDK_PORT, OpenRgbProbe, PROBE_TIMEOUT};
pub use resolve::{
    Action, ActionOutcome, ELEVATED_PROTOCOL_VERSION, ElevatedRequest, ElevatedResponse,
    MAX_ELEVATED_ACTIONS, PlannedAction, SystemOps, autostart_actions, check_action, close_actions, execute,
    run_elevated_request,
};
pub use scan::{Evidence, Finding, ScanOptions, ScanReport, Scanner, glob_match, sort_findings};
pub use sources::{
    DynamicLighting, FsPaths, Hive, InstalledApp, OpenRgbPortProbe, PathSource, ProcessInfo, ProcessSource,
    RegistrySource, ScanError, ServiceInfo, ServiceKind, ServiceSource, ServiceState, StartType, expand_env,
};

#[cfg(windows)]
impl Scanner {
    /// A scanner over the embedded catalog and the real Windows sources, including autostart entries.
    pub fn windows() -> Self {
        Scanner::new(
            Catalog::embedded(),
            WindowsProcesses,
            WindowsServices,
            WindowsRegistry,
        )
        .with_autostart(WindowsAutostart)
    }
}
