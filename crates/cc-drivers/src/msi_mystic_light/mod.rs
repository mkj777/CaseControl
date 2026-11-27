// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
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
//!   is zero except in [`Controller::save_to_hardware`](cc_core::Controller::save_to_hardware),
//!   [`Controller::persist_look`](cc_core::Controller::persist_look) and
//!   [`Controller::restore_saved_state`](cc_core::Controller::restore_saved_state) (see "Save to the board").
//! - Direct mode first sends the enable message (a 0x52 report with the per-LED effect 0x25 on the on-board zone),
//!   then one 725-byte feature report 0x53 per frame. While every ARGB header is within 40 LEDs (JRAINBOW1/2) and
//!   120 LEDs (JRAINBOW3/JCORSAIR) all LEDs travel in that single "sync" report; larger headers switch to one report
//!   per header with the reference's 13 ms gap between them.
//!
//! # Release
//!
//! `release()` re-reads the board's current configuration, sends it back, then sends the configuration read at open
//! with the save byte forced to zero. Nothing is written to flash, so the board's saved (power-on) state is not
//! touched (after a save, see below, the saved packet is what it hands back).
//!
//! Last good own lighting (ADR 0001 #38). What happens at open depends on the engine's persisted "unreleased" marker
//! ([`DetectContext::is_unreleased`](cc_core::DetectContext::is_unreleased): CaseControl wrote to the board and did not
//! hand it back, e.g. after a crash or sleep):
//!
//! - Marker set: the readback is CaseControl's state and is never reported as own lighting. If the detect context
//!   holds a remembered state for the board that passes [`protocol::validate_backup`] (185 bytes, report id 0x52,
//!   save byte 0, every zone record a savable effect with documented bits), it is restored right away (the two
//!   reports of a release), so the board and `hardware_mode()` agree, and `release()` restores it later. Without one,
//!   `release()` restores the readback and [`Controller::shows_own_lighting`](cc_core::Controller::shows_own_lighting)
//!   is `false`, so the marker stays. The decision is the shared [`cc_core::OwnLighting`] seam.
//! - Marker not set, readback per-LED (any zone in effect 0x25): another program drives the board. Nothing is stored
//!   or restored; an untouched release sends nothing, a release after CaseControl drove it restores the per-LED
//!   readback (warning at open).
//! - Marker not set, readback not per-LED: it is reported through
//!   [`Controller::own_lighting`](cc_core::Controller::own_lighting) (save byte 0) so the engine remembers it.
//!
//! A marked board with a valid remembered own lighting that the engine is about to drive
//! ([`DetectContext::has_setting_pending`](cc_core::DetectContext::has_setting_pending), ADR 0001 #42) is not restored
//! at open, so it does not flash the old look before the setting arrives; `release()` still restores it.
//!
//! # Save to the board (ADR 0001 #42)
//!
//! [`Controller::persist_look`](cc_core::Controller::persist_look) writes one configuration packet to flash. It is
//! built on a trusted base only: the backup of the original (the own lighting seen before CaseControl's first write of
//! the session, validated) or what this session last wrote to flash; never on the working configuration, a direct-mode
//! readback or a state CaseControl left, so neither the per-LED enable message nor this session's volatile records
//! reach flash. A board without a trusted base is never written. On the base the looked-for zones are set with the
//! same `apply_mode` / `apply_zone_color` calls a hardware-mode update makes: a zone colour as Static at level 10
//! (brightness baked into the colour), a hardware mode exactly as `set_mode` writes it, off as Static at brightness 0;
//! zones the look leaves out keep the base's bytes, and every header's LED count byte stays the base's (research
//! §1.4: unchanged; the configured header sizes are never written to flash). Gates (research §1.4), all
//! `DeviceError::Refused`: the firmware gate as at open; 185 bytes, report 0x52;
//! every zone record of the whole packet a savable effect `1..=39` other than 0x25, brightness at most 10, speed at
//! most 2 and only documented flag bits ([`protocol::check_saved_records`]); LED counts within the maxima; every
//! byte outside the saved zones' records and the on-board sync/mirror bits ([`protocol::save_change_mask`]) equal to
//! the base; outside direct mode a fresh readback must equal the working configuration. Sequence: the packet with
//! save 0 (this also leaves direct mode) and a readback that must equal it, then the reference's two reports with save
//! 1, then a verifying readback; afterwards (also after a failure) direct mode is taken again with the last frame. The
//! packet is the fingerprint, the next trusted base, and what `release()` hands back and the own lighting.
//!
//! A manual save ([`Controller::save_to_hardware`](cc_core::Controller::save_to_hardware)) is the same write of the
//! hardware mode `set_mode` last sent; it is refused in direct mode or before any hardware mode was set.
//!
//! A restore ([`Controller::restore_saved_state`](cc_core::Controller::restore_saved_state)) validates the backup
//! ([`protocol::validate_backup`]), leaves direct mode with it (volatile) when the board is driven, reads the board
//! back (it must not be in per-LED mode) and writes that readback with only the backed-up records replaced
//! ([`protocol::replace_records`], LED counts from the readback, gated like a save). When nothing was sent to the
//! board since open and the readback already holds the backup, nothing is written.
//!
//! Off: [`Controller::off_mode`](cc_core::Controller::off_mode) is Static, so brightness 0 is Static at brightness 0
//! (a hardware state) instead of black frames in per-LED mode.
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
        let result = open(&candidate.path).map_err(DeviceError::from).and_then(|dev| {
            MysticLight185::open_with_stored(
                dev,
                &candidate,
                ctx.stored_own_lighting(&candidate.id),
                ctx.is_unreleased(&candidate.id),
                ctx.has_setting_pending(&candidate.id),
            )
        });
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
