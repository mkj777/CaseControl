//! Hidden devices and zones (ADR 0001 #26, #27).
//!
//! A hidden device is not controlled: it is released when hidden, every write command refuses it and profiles skip
//! it. A hidden zone is kept black in direct colour: every direct buffer the app sends has the slices of the hidden
//! zones set to black. Hardware modes act on the whole board and are left alone. The flags live in
//! `cc_config::DeviceOverrides` (by device id and zone name); this module holds the pure functions over them.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use cc_config::{DeviceSetting, Settings};
use cc_core::{DeviceDescription, DeviceId, Rgb};
use serde::Serialize;

/// What is hidden of one device, as the frontend sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HiddenEntry {
    /// The whole device is hidden.
    pub device: bool,
    /// Names of the hidden zones, sorted.
    pub zones: Vec<String>,
}

/// Every device with something hidden, keyed by device id (devices with nothing hidden are left out).
pub fn hidden_view(settings: &Settings) -> BTreeMap<DeviceId, HiddenEntry> {
    settings
        .devices
        .iter()
        .filter(|(_, ov)| ov.hidden || !ov.hidden_zones.is_empty())
        .map(|(id, ov)| {
            (
                id.clone(),
                HiddenEntry {
                    device: ov.hidden,
                    zones: ov.hidden_zones.iter().cloned().collect(),
                },
            )
        })
        .collect()
}

/// The ids of every hidden device (the engine's excluded set: brightness and All off never take them over).
pub fn hidden_devices(settings: &Settings) -> HashSet<DeviceId> {
    settings
        .devices
        .iter()
        .filter(|(_, ov)| ov.hidden)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Whether device `id` is hidden.
pub fn is_device_hidden(settings: &Settings, id: &DeviceId) -> bool {
    settings.devices.get(id).is_some_and(|ov| ov.hidden)
}

/// The names of device `id`'s hidden zones (empty if none).
pub fn hidden_zones(settings: &Settings, id: &DeviceId) -> BTreeSet<String> {
    settings
        .devices
        .get(id)
        .map(|ov| ov.hidden_zones.clone())
        .unwrap_or_default()
}

/// The error a write command returns for a hidden device.
pub fn hidden_device_error(id: &DeviceId) -> String {
    format!("Device {id} is hidden, so CaseControl does not control it. Unhide it first.")
}

/// Hides or unhides device `id`. Returns whether anything changed. An entry left with nothing in it is removed.
pub fn set_device_hidden(settings: &mut Settings, id: &DeviceId, hidden: bool) -> bool {
    let entry = settings.devices.entry(id.clone()).or_default();
    let changed = entry.hidden != hidden;
    entry.hidden = hidden;
    prune(settings, id);
    changed
}

/// Hides or unhides zone `zone` of device `id`. Returns whether anything changed. An entry left with nothing in it
/// is removed.
pub fn set_zone_hidden(settings: &mut Settings, id: &DeviceId, zone: &str, hidden: bool) -> bool {
    let entry = settings.devices.entry(id.clone()).or_default();
    let changed = if hidden {
        entry.hidden_zones.insert(zone.to_owned())
    } else {
        entry.hidden_zones.remove(zone)
    };
    prune(settings, id);
    changed
}

/// Drops the overrides of `id` if they hold nothing (so hiding and unhiding leaves no empty table behind).
fn prune(settings: &mut Settings, id: &DeviceId) {
    if settings
        .devices
        .get(id)
        .is_some_and(|ov| *ov == cc_config::DeviceOverrides::default())
    {
        settings.devices.remove(id);
    }
}

/// Indices of the zones of `desc` whose names are in `hidden`.
pub fn hidden_zone_indices(desc: &DeviceDescription, hidden: &BTreeSet<String>) -> Vec<usize> {
    desc.zones
        .iter()
        .enumerate()
        .filter(|(_, z)| hidden.contains(&z.name))
        .map(|(i, _)| i)
        .collect()
}

/// Sets the LEDs of every zone of `desc` named in `hidden` to black. Slices beyond the end of `colors` are ignored,
/// so a buffer that does not match the layout cannot panic here (the engine rejects it afterwards).
pub fn mask_hidden_zones(desc: &DeviceDescription, hidden: &BTreeSet<String>, colors: &mut [Rgb]) {
    if hidden.is_empty() {
        return;
    }
    let len = colors.len();
    for zone in desc.zones.iter().filter(|z| hidden.contains(&z.name)) {
        let range = zone.range();
        let (start, end) = (range.start.min(len), range.end.min(len));
        colors[start..end].fill(Rgb::BLACK);
    }
}

/// The full buffer for writing `zone_colors` into zone `zone` of `buffer`, with the hidden zones masked. Fails for a
/// hidden zone, an unknown zone or a wrong colour count.
pub fn zone_write(
    desc: &DeviceDescription,
    hidden: &BTreeSet<String>,
    buffer: &[Rgb],
    zone: usize,
    zone_colors: &[Rgb],
) -> Result<Vec<Rgb>, String> {
    let z = desc.zones.get(zone).ok_or_else(|| format!("no zone {zone}"))?;
    if hidden.contains(&z.name) {
        return Err(format!(
            "Zone {} is hidden, so it stays off. Unhide it first.",
            z.name
        ));
    }
    let range = z.range();
    if zone_colors.len() != range.len() {
        return Err(format!(
            "{} colors for zone {zone} with {} LEDs",
            zone_colors.len(),
            range.len()
        ));
    }
    let mut out = buffer.to_vec();
    out.resize(desc.led_count(), Rgb::BLACK);
    out.get_mut(range)
        .ok_or_else(|| format!("zone {zone} lies outside the device buffer"))?
        .copy_from_slice(zone_colors);
    mask_hidden_zones(desc, hidden, &mut out);
    Ok(out)
}

/// A profile's device settings with the hidden things taken out: hidden devices are dropped (returned as the second
/// value), `Direct` settings of present devices are fitted to the device and get their hidden zones masked.
/// `describe` returns the description of a device that is present. Absent devices keep their setting unchanged
/// (their layout is unknown until they appear).
pub fn visible_profile(
    settings: &Settings,
    devices: &BTreeMap<DeviceId, DeviceSetting>,
    describe: impl Fn(&DeviceId) -> Option<DeviceDescription>,
) -> (BTreeMap<DeviceId, DeviceSetting>, Vec<DeviceId>) {
    let mut out = BTreeMap::new();
    let mut skipped = Vec::new();
    for (id, setting) in devices {
        if is_device_hidden(settings, id) {
            skipped.push(id.clone());
            continue;
        }
        let hidden = hidden_zones(settings, id);
        let setting = match (setting, hidden.is_empty()) {
            (DeviceSetting::Direct { .. }, false) => match describe(id) {
                Some(desc) => match setting.fit_to(desc.led_count()) {
                    DeviceSetting::Direct { mut colors } => {
                        mask_hidden_zones(&desc, &hidden, &mut colors);
                        DeviceSetting::Direct { colors }
                    }
                    other => other,
                },
                None => setting.clone(),
            },
            _ => setting.clone(),
        };
        out.insert(id.clone(), setting);
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use cc_core::{Bus, DeviceInfo, DeviceKind, Led, ModeState, Zone, ZoneKind};

    use super::*;

    const RED: Rgb = Rgb::new(0xFF, 0, 0);
    const GREEN: Rgb = Rgb::new(0, 0xFF, 0);
    const B: Rgb = Rgb::BLACK;

    fn board_id() -> DeviceId {
        DeviceId::hid(0x1462, 0x7D75, None)
    }

    fn zone(name: &str, start: usize, len: usize) -> Zone {
        Zone {
            name: name.into(),
            kind: ZoneKind::Linear,
            start,
            len,
            resizable: None,
        }
    }

    /// JRGB1 (1 LED), JRAINBOW1 (3 LEDs), JRAINBOW2 (2 LEDs): 6 LEDs.
    fn board() -> DeviceDescription {
        DeviceDescription {
            info: DeviceInfo {
                id: board_id(),
                name: "Board".into(),
                vendor: "MSI".into(),
                kind: DeviceKind::Motherboard,
                bus: Bus::Hid,
                location: "path".into(),
                serial: None,
                firmware: None,
            },
            zones: vec![
                zone("JRGB1", 0, 1),
                zone("JRAINBOW1", 1, 3),
                zone("JRAINBOW2", 4, 2),
            ],
            leds: (0..6).map(|i| Led { name: format!("{i}") }).collect(),
            modes: Vec::new(),
            direct_mode: Some(0),
            keepalive: None,
        }
    }

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn mask_blacks_out_hidden_zones_only() {
        let mut colors = vec![RED; 6];
        mask_hidden_zones(&board(), &names(&["JRGB1", "JRAINBOW2"]), &mut colors);
        assert_eq!(colors, vec![B, RED, RED, RED, B, B]);
    }

    #[test]
    fn mask_without_hidden_zones_changes_nothing() {
        let mut colors = vec![RED, GREEN, RED, GREEN, RED, GREEN];
        let before = colors.clone();
        mask_hidden_zones(&board(), &BTreeSet::new(), &mut colors);
        assert_eq!(colors, before);
        mask_hidden_zones(&board(), &names(&["NOPE"]), &mut colors);
        assert_eq!(colors, before, "unknown zone names are ignored");
    }

    #[test]
    fn mask_tolerates_a_short_buffer() {
        let mut colors = vec![RED; 5];
        mask_hidden_zones(&board(), &names(&["JRAINBOW2"]), &mut colors);
        assert_eq!(colors, vec![RED, RED, RED, RED, B]);
        let mut empty: Vec<Rgb> = Vec::new();
        mask_hidden_zones(&board(), &names(&["JRAINBOW2"]), &mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn zone_indices_by_name() {
        assert_eq!(
            hidden_zone_indices(&board(), &names(&["JRAINBOW2", "JRGB1"])),
            vec![0, 2]
        );
        assert!(hidden_zone_indices(&board(), &BTreeSet::new()).is_empty());
    }

    #[test]
    fn zone_write_masks_and_refuses_hidden_zone() {
        let hidden = names(&["JRGB1"]);
        let buffer = vec![GREEN; 6];
        let out = zone_write(&board(), &hidden, &buffer, 2, &[RED, RED]).expect("visible zone");
        assert_eq!(out, vec![B, GREEN, GREEN, GREEN, RED, RED]);
        assert!(zone_write(&board(), &hidden, &buffer, 0, &[RED]).is_err());
        assert!(
            zone_write(&board(), &hidden, &buffer, 1, &[RED]).is_err(),
            "wrong count"
        );
        assert!(
            zone_write(&board(), &hidden, &buffer, 9, &[RED]).is_err(),
            "no zone"
        );
    }

    #[test]
    fn hiding_and_unhiding_a_device() {
        let mut s = Settings::default();
        assert!(set_device_hidden(&mut s, &board_id(), true));
        assert!(is_device_hidden(&s, &board_id()));
        assert!(!set_device_hidden(&mut s, &board_id(), true), "already hidden");
        assert!(set_device_hidden(&mut s, &board_id(), false));
        assert!(!is_device_hidden(&s, &board_id()));
        assert!(s.devices.is_empty(), "an empty entry is removed");
    }

    #[test]
    fn hiding_zones_keeps_other_overrides() {
        let mut s = Settings::default();
        crate::prefs::record_zone_size(&mut s, &board_id(), "JRAINBOW1", 30);
        assert!(set_zone_hidden(&mut s, &board_id(), "JRGB1", true));
        assert!(set_zone_hidden(&mut s, &board_id(), "JRAINBOW2", true));
        assert!(!set_zone_hidden(&mut s, &board_id(), "JRGB1", true));
        assert_eq!(hidden_zones(&s, &board_id()), names(&["JRGB1", "JRAINBOW2"]));
        assert!(set_zone_hidden(&mut s, &board_id(), "JRGB1", false));
        assert!(set_zone_hidden(&mut s, &board_id(), "JRAINBOW2", false));
        assert!(hidden_zones(&s, &board_id()).is_empty());
        assert_eq!(
            s.devices[&board_id()].zone_sizes["JRAINBOW1"],
            30,
            "zone sizes stay"
        );
    }

    #[test]
    fn view_lists_only_devices_with_something_hidden() {
        let mut s = Settings::default();
        crate::prefs::record_zone_size(&mut s, &board_id(), "JRAINBOW1", 30);
        assert!(hidden_view(&s).is_empty());
        set_zone_hidden(&mut s, &board_id(), "JRGB1", true);
        let mouse = DeviceId::hid(0x046D, 0xC33C, None);
        set_device_hidden(&mut s, &mouse, true);
        let v = hidden_view(&s);
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[&board_id()],
            HiddenEntry {
                device: false,
                zones: vec!["JRGB1".into()]
            }
        );
        assert!(v[&mouse].device);
        let json = serde_json::to_value(&v).expect("serializes");
        assert_eq!(
            json["hid:1462:7d75"],
            serde_json::json!({ "device": false, "zones": ["JRGB1"] })
        );
    }

    #[test]
    fn profile_skips_hidden_devices_and_masks_hidden_zones() {
        let mut s = Settings::default();
        let mouse = DeviceId::hid(0x046D, 0xC33C, None);
        let absent = DeviceId::hid(0x31E3, 0x1310, None);
        set_device_hidden(&mut s, &mouse, true);
        set_zone_hidden(&mut s, &board_id(), "JRAINBOW2", true);
        set_zone_hidden(&mut s, &absent, "Keys", true);
        let mode = DeviceSetting::Mode {
            state: ModeState {
                mode: 1,
                speed: None,
                brightness: None,
                direction: None,
                colors: Vec::new(),
            },
        };
        let devices = BTreeMap::from([
            // Shorter than the device: fitted (padded with the last colour) before masking.
            (
                board_id(),
                DeviceSetting::Direct {
                    colors: vec![RED, GREEN],
                },
            ),
            (mouse.clone(), DeviceSetting::Direct { colors: vec![RED] }),
            (absent.clone(), DeviceSetting::Direct { colors: vec![RED] }),
        ]);
        let (out, skipped) = visible_profile(&s, &devices, |id| (*id == board_id()).then(board));
        assert_eq!(skipped, vec![mouse]);
        assert_eq!(
            out[&board_id()],
            DeviceSetting::Direct {
                colors: vec![RED, GREEN, GREEN, GREEN, B, B]
            }
        );
        assert_eq!(
            out[&absent],
            DeviceSetting::Direct { colors: vec![RED] },
            "absent: unchanged"
        );
        assert_eq!(out.len(), 2);

        let modes = BTreeMap::from([(board_id(), mode.clone())]);
        let (out, _) = visible_profile(&s, &modes, |_| Some(board()));
        assert_eq!(out[&board_id()], mode, "hardware modes are not touched");
    }
}
