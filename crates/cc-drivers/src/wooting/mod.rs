// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! Wooting keyboards with V3 firmware (usage page 0xFF55); in v1 only the Wooting 60HE (ARM), `31E3:1310`.
//!
//! Ported from `reference/openrgb/WootingKeyboardController/` (V3 controller, 60% layout, detector). One matrix
//! zone with the 60% key map, one "Direct" mode with per-key colours, and `release()` sends Wooting's "reset all"
//! command so the keyboard shows its own profile lighting again.
//!
//! What this module deliberately does not do: no hardware effects (the reference exposes none for V3), no V1/V2
//! firmware, no other Wooting models, and no command that touches key mappings, actuation or stored profiles.

mod controller;
pub mod layout;
pub mod protocol;

use std::collections::HashMap;

use cc_core::{
    Backend, Bus, ColorMode, DetectContext, Detected, DeviceDescription, DeviceId, DeviceInfo, DeviceKind,
    HotplugHandle, HotplugSender, Led, Mode, ModeCaps, Zone, ZoneKind, family,
};
use cc_hid::HidDeviceInfo;

pub use controller::WootingController;

/// Display name, from the reference's detector registration for `WOOTING_60HE_ARM_PID`.
pub const NAME_60HE_ARM: &str = "Wooting 60HE (ARM)";
/// Id of the "Direct" mode (`Direct.value = 0xFFFF` in the reference).
pub const DIRECT_MODE_ID: u32 = 0xFFFF;

/// The backend for this family.
pub fn backend() -> Box<dyn Backend> {
    Box::new(WootingBackend)
}

#[derive(Debug)]
struct WootingBackend;

impl Backend for WootingBackend {
    fn name(&self) -> &'static str {
        family::WOOTING
    }

    fn bus(&self) -> Bus {
        Bus::Hid
    }

    fn detect(&self, ctx: &DetectContext) -> Vec<Detected> {
        if !ctx.is_enabled(family::WOOTING) {
            return Vec::new();
        }
        let devices = match cc_hid::enumerate() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(target: "cc_drivers::wooting", error = %e, "HID enumeration failed");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for (id, info) in candidates(&devices) {
            if ctx.is_skipped(&id) {
                tracing::debug!(target: "cc_drivers::wooting", %id, "already controlled, skipped");
                continue;
            }
            match cc_hid::open(&info.path) {
                Ok(transport) => {
                    let (desc, hw_index) = description(id, info);
                    tracing::info!(target: "cc_drivers::wooting", id = %desc.info.id, path = %info.path, "detected");
                    out.push(Detected::Device(Box::new(WootingController::new(
                        transport, desc, hw_index,
                    ))));
                }
                Err(e) => out.push(Detected::Failed {
                    family: family::WOOTING.to_owned(),
                    location: info.path.clone(),
                    error: e.into(),
                }),
            }
        }
        out
    }

    fn subscribe_hotplug(&self, _tx: HotplugSender) -> Option<Box<dyn HotplugHandle>> {
        None
    }
}

/// The stable id of an enumerated 60HE (ARM) interface.
///
/// Built from the base pid `0x1310` whatever emulation variant (`1310`/`1311`/`1312`) enumerated, so a profile
/// stays attached when the user switches gamepad emulation. The enumerated pid stays visible in
/// `DeviceInfo::location` (the HID path) for diagnostics.
pub fn device_id(info: &HidDeviceInfo) -> DeviceId {
    DeviceId::hid(info.vid, protocol::PID_60HE_ARM, info.serial.as_deref())
}

/// Picks the interfaces to drive from an enumeration and computes their ids, without any I/O.
///
/// Matches the reference's detector for the 60HE (ARM): vendor `31E3`, product `1310`/`1311`/`1312` (gamepad
/// emulation variants), and only the V3 configuration interface (usage page 0xFF55). The reference opens every
/// interface of the product and closes the non-V3 ones again; this filters by usage page first and opens nothing
/// else. If one serial shows up on several 0xFF55 collections only the first is kept. Serial-less devices that
/// would share an id get ordinals in enumeration order.
pub fn candidates(devices: &[HidDeviceInfo]) -> Vec<(DeviceId, &HidDeviceInfo)> {
    let matching: Vec<&HidDeviceInfo> = devices
        .iter()
        .filter(|d| {
            d.vid == protocol::VID
                && protocol::PIDS_60HE_ARM.contains(&d.pid)
                && d.usage_page == protocol::USAGE_PAGE_V3
        })
        .collect();

    let mut picked: Vec<(DeviceId, &HidDeviceInfo)> = Vec::new();
    for d in matching {
        let id = device_id(d);
        // Same rule as `DeviceId::hid`: empty, whitespace-only and all-zero serials count as absent.
        let has_serial = d
            .serial
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty() && !s.chars().all(|c| c == '0'));
        if has_serial && picked.iter().any(|(p, _)| *p == id) {
            tracing::debug!(target: "cc_drivers::wooting", %id, path = %d.path, "duplicate 0xFF55 collection ignored");
            continue;
        }
        picked.push((id, d));
    }

    let mut totals: HashMap<DeviceId, u32> = HashMap::new();
    for (id, _) in &picked {
        *totals.entry(id.clone()).or_default() += 1;
    }
    let mut seen: HashMap<DeviceId, u32> = HashMap::new();
    picked
        .into_iter()
        .map(|(id, d)| {
            if totals[&id] > 1 {
                let n = seen.entry(id.clone()).or_default();
                let with = id.clone().with_ordinal(*n);
                *n += 1;
                (with, d)
            } else {
                (id, d)
            }
        })
        .collect()
}

/// Builds the device description for an enumerated 60HE (ARM) and the hardware index of each LED.
pub fn description(id: DeviceId, info: &HidDeviceInfo) -> (DeviceDescription, Vec<u8>) {
    let mut leds = Vec::new();
    let mut hw_index = Vec::new();
    let mut map = Vec::with_capacity(layout::ROWS * layout::COLUMNS);
    for cell in layout::KEYS_60.iter().flatten() {
        match cell {
            Some((hw, name)) => {
                map.push(Some(leds.len() as u32));
                leds.push(Led {
                    name: (*name).to_owned(),
                });
                hw_index.push(*hw);
            }
            None => map.push(None),
        }
    }
    let desc = DeviceDescription {
        info: DeviceInfo {
            id,
            name: NAME_60HE_ARM.to_owned(),
            vendor: info.manufacturer.clone().unwrap_or_else(|| "Wooting".to_owned()),
            kind: DeviceKind::Keyboard,
            bus: Bus::Hid,
            location: info.path.clone(),
            serial: info.serial.clone(),
            firmware: None,
        },
        zones: vec![Zone {
            // The reference names the zone `name + " zone"`.
            name: format!("{NAME_60HE_ARM} zone"),
            kind: ZoneKind::Matrix {
                width: layout::COLUMNS as u32,
                height: layout::ROWS as u32,
                map,
            },
            start: 0,
            len: leds.len(),
            resizable: None,
            brightness_only: false,
        }],
        leds,
        modes: vec![Mode {
            id: DIRECT_MODE_ID,
            name: "Direct".to_owned(),
            caps: ModeCaps::PER_LED_COLOR,
            color_mode: ColorMode::PerLed,
            speed: None,
            brightness: None,
            colors: None,
        }],
        direct_mode: Some(DIRECT_MODE_ID),
        keepalive: None,
    };
    (desc, hw_index)
}

#[cfg(test)]
mod tests;
