//! Read-only snapshots the engine hands out: device views, apply reports, detection failures.

use cc_config::DeviceSetting;
use cc_core::{DeviceDescription, DeviceError, DeviceId, ModeState, Rgb};
use serde::Serialize;

/// Whether the engine is driving a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceStatus {
    /// Under software control: CaseControl is outputting a setting (also at brightness 0).
    Active,
    /// Showing its own lighting: never taken since it was detected, or handed back. Any lighting control (colours,
    /// mode, brightness) takes control again.
    Released,
    /// Taken out of service; the engine no longer talks to it until it is re-plugged or rescanned after removal.
    Faulted {
        /// What went wrong.
        error: String,
    },
}

/// Everything the UI needs to show one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceView {
    /// Identity, layout and modes (reflects zone resizes).
    pub description: DeviceDescription,
    /// Control status.
    pub status: DeviceStatus,
    /// What the device was last actually set to (recorded by its thread after the write), if anything.
    pub setting: Option<DeviceSetting>,
    /// The engine's colour buffer, one entry per LED. Starts with the device's own per-LED colours where its driver
    /// can read them (see `hardware`), else black.
    pub colors: Vec<Rgb>,
    /// What the device shows on its own (its hardware mode), as its driver read it, if it can (see
    /// `cc_core::Controller::hardware_mode`). This is what a released device shows, and what a brightness change
    /// adopts as the setting of a device that has none.
    pub hardware: Option<ModeState>,
    /// Family id of the backend that detected the device (`Backend::name()`).
    pub family: String,
    /// Device brightness in percent (ADR 0001 #33); `colors` and `setting` are unscaled.
    pub brightness: u8,
}

/// Master brightness (ADR 0001 #33).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LightingView {
    /// Master brightness in percent.
    pub master_brightness: u8,
}

/// One device a backend found but could not bring up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectFailure {
    /// Family (backend) that found it.
    pub family: String,
    /// Where it was found, for diagnostics.
    pub location: String,
    /// Why it failed.
    pub error: DeviceError,
}

/// One entry of [`EngineHandle::apply`](crate::EngineHandle::apply) that failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyFailure {
    /// The device.
    pub id: DeviceId,
    /// Why the setting was not applied.
    pub error: String,
}

/// Result of [`EngineHandle::apply`](crate::EngineHandle::apply).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    /// Devices the setting was sent to.
    pub applied: Vec<DeviceId>,
    /// Devices not present; their settings are kept as pending and applied when they appear.
    pub missing: Vec<DeviceId>,
    /// Devices that rejected the setting.
    pub failed: Vec<ApplyFailure>,
}
