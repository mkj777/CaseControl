//! Read-only snapshots the engine hands out: device views, apply reports, detection failures.

use cc_config::DeviceSetting;
use cc_core::{DeviceDescription, DeviceError, DeviceId, Rgb};
use serde::Serialize;

/// Whether the engine is driving a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceStatus {
    /// Under software control (or ready to be).
    Active,
    /// Handed back to its hardware mode; the next colour or mode call takes control again.
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
    /// The last setting the engine was asked to show, if any.
    pub setting: Option<DeviceSetting>,
    /// The engine's colour buffer, one entry per LED.
    pub colors: Vec<Rgb>,
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
