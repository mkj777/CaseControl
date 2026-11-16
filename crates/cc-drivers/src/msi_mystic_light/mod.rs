//! MSI Mystic Light motherboards (USB HID), 185-byte class.
//!
//! Port of OpenRGB's `MSIMotherboardController` for the board this project targets, the MAG B650 TOMAHAWK WIFI
//! (MS-7D75, `1462:7D75`), and the other boards of the same class in the reference's tested list.
//!
//! # Protocol
//!
//! - Detection matches the reference's tested detector table (usage page 0x0001, usage 0x00) and identifies the
//!   controller class by the length `get_feature_report(0x52)` returns. Only the 185-byte class is ported; the
//!   162-, 112- and 761-byte classes fail detection with [`Detected::Failed`]. The 761-byte path of the reference
//!   sends a 761-byte feature report during detection; nothing like it runs here.
//! - On open the driver queries the firmware versions (output reports `01 B0 ..` / `01 B6 ..`), applies the
//!   reference's firmware gate (`GetFirmwareVersionInvalid`) and reads the configuration report 0x52 (185 bytes).
//! - Hardware modes rewrite that configuration (`SetMode`, `SetZoneColor`) and send it as feature report 0x52, preceded
//!   by the board's current configuration, exactly like the reference's `Update`. The save-to-flash byte (offset 184)
//!   is zero except in [`Controller::save_to_hardware`](cc_core::Controller::save_to_hardware).
//! - Direct mode first sends the enable message (a 0x52 report with the per-LED effect 0x25 on the on-board zone),
//!   then one 725-byte feature report 0x53 per frame. While every ARGB header is within 40 LEDs (JRAINBOW1/2) and
//!   120 LEDs (JRAINBOW3/JCORSAIR) all LEDs travel in that single "sync" report; larger headers switch to one report
//!   per header with the reference's 13 ms gap between them.
//!
//! # Release
//!
//! `release()` re-reads the board's current configuration, sends it back, then sends the configuration read at open
//! with the save byte forced to zero. Nothing is written to flash, so the board's saved (power-on) state is never
//! touched. If the configuration read at open was itself a per-LED state left by another program (on-board effect
//! 0x25), `release()` restores exactly that: the board stays in per-LED mode and keeps showing the last per-LED frame
//! it received (ours) until the next power cycle or until another program takes over; the flash configuration
//! still applies after a power cycle. A warning is logged at open in that case.
//!
//! # Zones
//!
//! Zones follow the reference board table. ARGB headers (JRAINBOW1/2/3) start with the LED count the board reports
//! in its configuration (`cycle_or_led_num`), clamped to the zone's range; JCORSAIR has no count byte and starts at 0.
//! (OpenRGB starts every header at 0.) Headers are resized with
//! [`Controller::resize_zone`](cc_core::Controller::resize_zone) (JRAINBOW1 up to 200, JRAINBOW2/3 and JCORSAIR up
//! to 240 LEDs); the engine applies saved sizes that way. The header size is also written into the configuration's
//! LED count byte, so hardware effects run on exactly the configured number of LEDs.
//!
//! # Not done here
//!
//! No zone-based direct mode (boards whose reference configuration uses it fail detection), no hot-plug
//! subscription (the engine owns hot-plug), no threads, no DMI lookup (board names come from the board table).

mod boards;
mod controller;
pub mod protocol;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use cc_core::{Backend, Bus, DetectContext, Detected, DeviceError, DeviceId, HotplugHandle, HotplugSender};
use cc_hid::{HidDeviceInfo, HidError, HidTransport};

pub use boards::{
    BOARD_CONFIGS, BoardConfig, DETECTORS, DirectSupport, board_config, firmware_version_invalid,
};
pub use controller::{
    Candidate, FW_READ_TIMEOUT, Firmware, MysticLight185, NON_SYNC_PACKET_DELAY, board_id, read_config_len,
};

/// Family id of this backend.
pub const FAMILY: &str = cc_core::family::MSI_MYSTIC_LIGHT;

/// The backend for this family.
pub fn backend() -> Box<dyn Backend> {
    Box::new(MysticLightBackend)
}

/// Mystic Light detection over `cc_hid`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MysticLightBackend;

impl Backend for MysticLightBackend {
    fn name(&self) -> &'static str {
        FAMILY
    }

    fn bus(&self) -> Bus {
        Bus::Hid
    }

    fn detect(&self, ctx: &DetectContext) -> Vec<Detected> {
        if !ctx.is_enabled(FAMILY) {
            return Vec::new();
        }
        let devices = match cc_hid::enumerate() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(target: "cc_drivers::msi", error = %e, "HID enumeration failed");
                return Vec::new();
            }
        };
        detect_with(ctx, &devices, cc_hid::open)
    }

    fn subscribe_hotplug(&self, _tx: HotplugSender) -> Option<Box<dyn HotplugHandle>> {
        None
    }
}

/// The enumerated interfaces the detector table accepts, with ids computed from enumeration data only
/// (`DeviceId::hid`, plus an ordinal when several boards with the same vid/pid report no serial).
pub fn candidates(devices: &[HidDeviceInfo]) -> Vec<Candidate> {
    let matched: Vec<&HidDeviceInfo> = devices
        .iter()
        .filter(|d| DETECTORS.iter().any(|m| m.matches(d)))
        .collect();
    let base_id = |d: &HidDeviceInfo| DeviceId::hid(d.vid, d.pid, d.serial.as_deref());

    let mut totals: HashMap<DeviceId, u32> = HashMap::new();
    for d in &matched {
        *totals.entry(base_id(d)).or_default() += 1;
    }
    let mut seen: HashMap<DeviceId, u32> = HashMap::new();
    matched
        .into_iter()
        .map(|d| {
            let base = base_id(d);
            let id = if totals.get(&base).copied().unwrap_or(0) > 1 {
                let n = seen.entry(base.clone()).or_default();
                let id = base.with_ordinal(*n);
                *n += 1;
                id
            } else {
                base
            };
            Candidate {
                path: d.path.clone(),
                vid: d.vid,
                pid: d.pid,
                serial: d.serial.clone(),
                id,
            }
        })
        .collect()
}

/// Detection on a given device list with a given opener; [`MysticLightBackend::detect`] passes `cc_hid::enumerate`
/// and `cc_hid::open`. Candidates whose id `ctx` skips are neither opened nor returned.
pub fn detect_with<F>(ctx: &DetectContext, devices: &[HidDeviceInfo], mut open: F) -> Vec<Detected>
where
    F: FnMut(&str) -> Result<Box<dyn HidTransport>, HidError>,
{
    if !ctx.is_enabled(FAMILY) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for candidate in candidates(devices) {
        if ctx.is_skipped(&candidate.id) {
            continue;
        }
        let result = open(&candidate.path)
            .map_err(DeviceError::from)
            .and_then(|dev| MysticLight185::open(dev, &candidate));
        match result {
            Ok(controller) => {
                tracing::info!(target: "cc_drivers::msi", id = %candidate.id, "Mystic Light controller ready");
                out.push(Detected::Device(Box::new(controller)));
            }
            Err(error) => {
                tracing::warn!(target: "cc_drivers::msi", id = %candidate.id, %error, "Mystic Light detection failed");
                out.push(Detected::Failed {
                    family: FAMILY.to_owned(),
                    location: candidate.path,
                    error,
                });
            }
        }
    }
    out
}
