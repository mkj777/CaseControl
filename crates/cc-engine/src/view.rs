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
    /// What a takeover (brightness, Connect all) drives the device with: what the engine drives it with now, else its
    /// last setting, the mode it shows on its own, or the default colour, in that order. `None` when it cannot be
    /// taken over (faulted, or neither a direct mode nor a mode that takes a colour and nothing else to use). It does
    /// not consider whether the app hides the device.
    pub takeover: Option<Takeover>,
}

/// What a takeover drives a device with (see [`DeviceView::takeover`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Takeover {
    /// The hardware mode it uses, or `None` for direct colours.
    pub mode_id: Option<u32>,
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

/// One device a request failed for: an entry of [`EngineHandle::apply`](crate::EngineHandle::apply), or a release of
/// [`EngineHandle::release_all`](crate::EngineHandle::release_all).
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

/// Result of [`EngineHandle::release_all`](crate::EngineHandle::release_all).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReleaseReport {
    /// How many devices were driven (or being taken over) and are released now.
    pub released: usize,
    /// Devices whose release failed.
    pub failed: Vec<ApplyFailure>,
}

/// How [`EngineHandle::shutdown_with`](crate::EngineHandle::shutdown_with) leaves the devices (ADR 0001 #42).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownMode {
    /// Hand every device back to its own lighting (explicit Quit, installer, today's default).
    Release,
    /// Keep CaseControl's colours on the devices (Windows shutdown, restart, sign-out): every device is sent its
    /// latest output once more, then its thread stops without a release. The "unreleased" markers stay set.
    Keep,
}

/// Why a device was not saved to or restored (see [`PersistResult::Skipped`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The app hides the device (it is not controlled).
    Hidden,
    /// The device is faulted.
    Faulted,
    /// Its driver cannot save to it (or not this kind of look).
    NotCapable,
    /// The engine does not drive it (never set, or released), so there is no look to save.
    NotDriven,
    /// Every zone of the look is hidden or empty.
    NothingToSave,
    /// Its driver cannot tell the device's original saved state completely, so it is never written.
    OriginalUnknown,
    /// The device's memory was written the maximum number of times today.
    DailyCap,
    /// There is no backup to write back.
    NoBackup,
    /// The device is not connected; its backup is written back when it is next connected.
    Absent,
    /// The call's time budget ran out before the device's turn (also: its worker took the request up too late).
    TimeBudget,
    /// Writing devices' memory is switched off for this engine ([`EngineConfig::device_memory_writes`]), e.g. in the
    /// start-up instance before sign-in.
    ///
    /// [`EngineConfig::device_memory_writes`]: crate::EngineConfig::device_memory_writes
    WritesDisabled,
    /// The records of saving to the devices (backup, counts) cannot be stored durably this session (unreadable or
    /// foreign `device-memory.toml`, no machine id, in-memory state), so nothing is written to the device.
    StateUnavailable,
}

/// What happened to one device in [`EngineHandle::persist_all`](crate::EngineHandle::persist_all) or
/// [`EngineHandle::restore_saved`](crate::EngineHandle::restore_saved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PersistResult {
    /// Its memory was written (the look saved, or the backup written back).
    Written,
    /// The look equals the one saved last; nothing was written.
    Unchanged,
    /// Not written, for a reason that is not an error.
    Skipped {
        /// Why.
        reason: SkipReason,
    },
    /// Writing failed or a safety gate refused it; the device's memory was not (completely) written.
    Failed {
        /// What went wrong.
        error: String,
    },
    /// The device did not answer within the time budget. The request is still queued or running and may complete;
    /// its records (backup, fingerprint, counts, pending restore) are updated when it does.
    Unconfirmed,
}

/// One device's result in a [`PersistReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistEntry {
    /// The device.
    pub id: DeviceId,
    /// What happened.
    pub result: PersistResult,
}

/// Result of [`EngineHandle::persist_all`](crate::EngineHandle::persist_all) and
/// [`EngineHandle::restore_saved`](crate::EngineHandle::restore_saved), one entry per device considered, in id
/// order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PersistReport {
    /// Per-device results.
    pub devices: Vec<PersistEntry>,
}

impl PersistReport {
    /// The result of device `id`, if it was considered.
    pub fn result(&self, id: &DeviceId) -> Option<&PersistResult> {
        self.devices.iter().find(|e| e.id == *id).map(|e| &e.result)
    }
}
