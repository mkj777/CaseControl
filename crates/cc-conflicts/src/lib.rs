//! Data layer of CaseControl's conflict view.
//!
//! What this crate does:
//! - Ships an embedded, validated [`Catalog`] of software that competes for RGB devices or low-level hardware
//!   access (RGB suites, vendor RGB tools, hardware monitors, fan control, kernel drivers, Windows Dynamic Lighting).
//! - Detects that software without elevation through scan sources behind traits ([`ProcessSource`],
//!   [`ServiceSource`], [`RegistrySource`], [`PathSource`], [`OpenRgbPortProbe`]); the Windows implementations are
//!   `WindowsProcesses`, `WindowsServices` and `WindowsRegistry`.
//! - Matches the scan against the catalog in [`Scanner::scan`] and returns [`Finding`]s with their [`Evidence`].
//! - Offers two opt-in actions for an explicit UI click: [`terminate_process`] (same user, re-checks the name) and
//!   [`request_stop_service`] (`sc.exe stop` behind a UAC prompt).
//!
//! What it deliberately does not do:
//! - No elevation except the user-triggered service stop, no kernel driver, no `EnumDeviceDrivers` (useless
//!   unelevated on Windows 11 24H2); loaded drivers are seen through the service control manager.
//! - Never disables autostart, deletes services, uninstalls software or writes the registry (ADR 0001 #12).
//! - No network: the only socket is the optional loopback probe of the OpenRGB SDK port 6742, off by default
//!   ([`ScanOptions::check_openrgb_port`], ADR 0001 #13).
//! - Store/MSIX packages are not enumerated, so Store-only apps (HyperX NGENUITY) are only partly detectable.
//!
//! All `unsafe` code lives in the private `sys` module (Windows only).

mod actions;
mod catalog;
#[cfg(windows)]
mod platform;
mod probe;
mod scan;
mod sources;
#[cfg(windows)]
#[allow(unsafe_code)]
mod sys;

pub use actions::{ActionError, request_stop_service, terminate_process};
pub use catalog::{
    AFFECTS_ALL, Access, Catalog, CatalogError, Category, DYNAMIC_LIGHTING_ENTRY, Entry, OPENRGB_ENTRY,
    Severity,
};
/// Device family ids the catalog's `affects` lists refer to (re-exported from `cc-core`).
pub use cc_core::family;
#[cfg(windows)]
pub use platform::{WindowsProcesses, WindowsRegistry, WindowsServices};
pub use probe::{OPENRGB_SDK_PORT, OpenRgbProbe, PROBE_TIMEOUT};
pub use scan::{Evidence, Finding, ScanOptions, ScanReport, Scanner, glob_match, sort_findings};
pub use sources::{
    DynamicLighting, FsPaths, Hive, InstalledApp, OpenRgbPortProbe, PathSource, ProcessInfo, ProcessSource,
    RegistrySource, ScanError, ServiceInfo, ServiceKind, ServiceSource, ServiceState, StartType, expand_env,
};

#[cfg(windows)]
impl Scanner {
    /// A scanner over the embedded catalog and the real Windows sources.
    pub fn windows() -> Self {
        Scanner::new(
            Catalog::embedded(),
            WindowsProcesses,
            WindowsServices,
            WindowsRegistry,
        )
    }
}
