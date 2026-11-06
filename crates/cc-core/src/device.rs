//! Devices: identity, LED layout (zones and LEDs) and the full description a controller exposes.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::id::DeviceId;
use crate::mode::{ColorMode, Mode, ModeCaps, ModeState};

/// The transport a device is reached through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Bus {
    /// USB / Bluetooth HID.
    Hid,
    /// NVIDIA GPU via NvAPI.
    NvApi,
    /// SMBus / I2C; reserved for a later tier.
    Smbus,
}

/// What kind of device this is. The OpenRGB `device_type` list, in OpenRGB's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    /// Mainboard with on-board LEDs and/or RGB headers.
    Motherboard,
    /// Memory modules.
    Dram,
    /// Graphics card.
    Gpu,
    /// CPU or case cooler.
    Cooler,
    /// LED strip.
    LedStrip,
    /// Keyboard.
    Keyboard,
    /// Mouse.
    Mouse,
    /// Mouse mat.
    Mousemat,
    /// Headset.
    Headset,
    /// Headset stand.
    HeadsetStand,
    /// Gamepad.
    Gamepad,
    /// Light.
    Light,
    /// Speaker.
    Speaker,
    /// Virtual device.
    Virtual,
    /// Storage drive.
    Storage,
    /// Case.
    Case,
    /// Microphone.
    Microphone,
    /// Other accessory.
    Accessory,
    /// Keypad.
    Keypad,
    /// Laptop.
    Laptop,
    /// Monitor.
    Monitor,
    /// Anything else.
    Unknown,
}

/// Identity of a device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable id.
    pub id: DeviceId,
    /// Display name.
    pub name: String,
    /// Vendor name.
    pub vendor: String,
    /// Device kind.
    pub kind: DeviceKind,
    /// Transport.
    pub bus: Bus,
    /// Human-readable location (e.g. HID path or PCI slot); informational only, never used as identity.
    pub location: String,
    /// Serial number, if the device reports one.
    pub serial: Option<String>,
    /// Firmware version, if known.
    pub firmware: Option<String>,
}

/// The layout of a zone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ZoneKind {
    /// One LED, or LEDs that always show the same color.
    Single,
    /// A strip of LEDs.
    Linear,
    /// A grid of LEDs.
    Matrix {
        /// Columns.
        width: u32,
        /// Rows.
        height: u32,
        /// Row-major `width * height` cells; each is a zone-relative LED index or `None` for an empty cell.
        map: Vec<Option<u32>>,
    },
}

/// A named, contiguous slice of a device's LED buffer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Zone {
    /// Display name.
    pub name: String,
    /// Layout.
    pub kind: ZoneKind,
    /// Index of the zone's first LED in the device buffer.
    pub start: usize,
    /// Number of LEDs.
    pub len: usize,
    /// Allowed lengths if the zone can be resized (e.g. ARGB headers).
    pub resizable: Option<RangeInclusive<usize>>,
}

impl Zone {
    /// The zone's LED indices in the device buffer.
    ///
    /// Saturates instead of overflowing, so an unvalidated zone cannot panic here.
    pub fn range(&self) -> Range<usize> {
        self.start..self.start.saturating_add(self.len)
    }
}

/// One LED.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Led {
    /// Display name.
    pub name: String,
}

/// Everything a controller exposes about its device: identity, LED layout and modes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceDescription {
    /// Identity.
    pub info: DeviceInfo,
    /// Zones, sorted by `start`, covering `0..leds.len()` exactly.
    pub zones: Vec<Zone>,
    /// The flat LED buffer.
    pub leds: Vec<Led>,
    /// Available modes.
    pub modes: Vec<Mode>,
    /// Id of the mode that shows static per-LED colors with no hardware effect, if the device has one.
    pub direct_mode: Option<u32>,
    /// How often the device must be refreshed to stay in software control; serialized as whole milliseconds, so
    /// sub-millisecond precision is dropped when serializing.
    #[serde(with = "duration_ms", default)]
    pub keepalive: Option<Duration>,
}

impl DeviceDescription {
    /// Number of LEDs.
    pub fn led_count(&self) -> usize {
        self.leds.len()
    }

    /// The LED indices of zone `zone` in the device buffer.
    ///
    /// # Panics
    ///
    /// If `zone` is not a valid zone index.
    pub fn zone_range(&self, zone: usize) -> Range<usize> {
        self.zones[zone].range()
    }

    /// Looks a mode up by id.
    pub fn mode(&self, id: u32) -> Option<&Mode> {
        self.modes.iter().find(|m| m.id == id)
    }

    /// Checks the model invariants.
    ///
    /// Zones are sorted by `start`, contiguous, non-overlapping and cover `0..leds.len()` exactly; a resizable zone's
    /// length lies inside its `resizable` range; every matrix map has `width * height` cells and every index in it is
    /// below the zone length; mode ids are unique; every mode's color mode has its matching capability
    /// ([`ColorMode::PerLed`] needs [`ModeCaps::PER_LED_COLOR`], [`ColorMode::ModeSpecific`] needs
    /// [`ModeCaps::MODE_SPECIFIC_COLOR`], [`ColorMode::Random`] needs [`ModeCaps::RANDOM_COLOR`]); mode-specific-color
    /// modes declare a color count range; `direct_mode` names an existing mode whose color mode is
    /// [`ColorMode::PerLed`].
    pub fn validate(&self) -> Result<(), ModelError> {
        self.validate_zones()?;
        self.validate_modes()
    }

    /// Checks that `state` is valid for this device: the mode exists and [`Mode::validate_state`] accepts it.
    pub fn validate_state(&self, state: &ModeState) -> Result<(), ModelError> {
        self.mode(state.mode)
            .ok_or(ModelError::UnknownMode { id: state.mode })?
            .validate_state(state, self.led_count())
    }

    fn validate_zones(&self) -> Result<(), ModelError> {
        let led_count = self.led_count();
        let mut expected = 0;
        for (i, zone) in self.zones.iter().enumerate() {
            if let Some(prev) = i.checked_sub(1).map(|p| &self.zones[p])
                && zone.start < prev.start
            {
                return Err(ModelError::ZoneUnsorted {
                    zone: i,
                    start: zone.start,
                    prev: i - 1,
                    prev_start: prev.start,
                });
            }
            if zone.start > expected {
                return Err(ModelError::ZoneGap {
                    zone: i,
                    start: zone.start,
                    expected,
                });
            }
            if zone.start < expected {
                return Err(ModelError::ZoneOverlap {
                    zone: i,
                    start: zone.start,
                    expected,
                });
            }
            let end = zone.start.saturating_add(zone.len);
            if end > led_count {
                return Err(ModelError::ZonePastEnd {
                    zone: i,
                    end,
                    led_count,
                });
            }
            if let Some(allowed) = &zone.resizable
                && !allowed.contains(&zone.len)
            {
                return Err(ModelError::LenOutsideResizable {
                    zone: i,
                    len: zone.len,
                    min: *allowed.start(),
                    max: *allowed.end(),
                });
            }
            if let ZoneKind::Matrix { width, height, map } = &zone.kind {
                if u64::from(*width) * u64::from(*height) != map.len() as u64 {
                    return Err(ModelError::MatrixMapSize {
                        zone: i,
                        width: *width,
                        height: *height,
                        cells: map.len(),
                    });
                }
                for (cell, index) in map.iter().enumerate() {
                    if let Some(index) = *index
                        && index as usize >= zone.len
                    {
                        return Err(ModelError::MatrixIndexOutOfRange {
                            zone: i,
                            cell,
                            index,
                            len: zone.len,
                        });
                    }
                }
            }
            expected = end;
        }
        if expected != led_count {
            return Err(ModelError::ZonesIncomplete {
                covered: expected,
                led_count,
            });
        }
        Ok(())
    }

    fn validate_modes(&self) -> Result<(), ModelError> {
        let mut seen = HashMap::with_capacity(self.modes.len());
        for (i, mode) in self.modes.iter().enumerate() {
            if let Some(first) = seen.insert(mode.id, i) {
                return Err(ModelError::DuplicateModeId {
                    id: mode.id,
                    first,
                    second: i,
                });
            }
            let required_cap = match mode.color_mode {
                ColorMode::None => ModeCaps::empty(),
                ColorMode::PerLed => ModeCaps::PER_LED_COLOR,
                ColorMode::ModeSpecific => ModeCaps::MODE_SPECIFIC_COLOR,
                ColorMode::Random => ModeCaps::RANDOM_COLOR,
            };
            if !mode.caps.contains(required_cap) {
                return Err(ModelError::ColorModeCapMismatch { mode: mode.id });
            }
            if mode.color_mode == ColorMode::ModeSpecific && mode.colors.is_none() {
                return Err(ModelError::MissingColorRange { mode: mode.id });
            }
        }
        if let Some(id) = self.direct_mode {
            let mode = self.mode(id).ok_or(ModelError::UnknownMode { id })?;
            if mode.color_mode != ColorMode::PerLed {
                return Err(ModelError::DirectModeNotPerLed { id });
            }
        }
        Ok(())
    }
}

/// Serde adapter: `Option<Duration>` as optional whole milliseconds (`u64`).
mod duration_ms {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(d) => {
                let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
                serializer.serialize_some(&ms)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(deserializer)?.map(Duration::from_millis))
    }
}

/// A valid three-zone description used by tests across the crate.
#[cfg(test)]
pub(crate) fn sample_description() -> DeviceDescription {
    let led = |n: usize| Led {
        name: format!("LED {n}"),
    };
    DeviceDescription {
        info: DeviceInfo {
            id: DeviceId::hid(0x1462, 0x7D75, None),
            name: "Test board".into(),
            vendor: "MSI".into(),
            kind: DeviceKind::Motherboard,
            bus: Bus::Hid,
            location: "HID: test".into(),
            serial: None,
            firmware: Some("1.0".into()),
        },
        zones: vec![
            Zone {
                name: "Board".into(),
                kind: ZoneKind::Single,
                start: 0,
                len: 1,
                resizable: None,
            },
            Zone {
                name: "ARGB 1".into(),
                kind: ZoneKind::Linear,
                start: 1,
                len: 3,
                resizable: Some(0..=120),
            },
            Zone {
                name: "Keys".into(),
                kind: ZoneKind::Matrix {
                    width: 2,
                    height: 2,
                    map: vec![Some(0), Some(1), None, Some(2)],
                },
                start: 4,
                len: 3,
                resizable: None,
            },
        ],
        leds: (0..7).map(led).collect(),
        modes: vec![
            Mode {
                id: 0,
                name: "Direct".into(),
                caps: ModeCaps::PER_LED_COLOR,
                color_mode: ColorMode::PerLed,
                speed: None,
                brightness: None,
                colors: None,
            },
            Mode {
                id: 1,
                name: "Breathing".into(),
                caps: ModeCaps::SPEED | ModeCaps::BRIGHTNESS | ModeCaps::MODE_SPECIFIC_COLOR,
                color_mode: ColorMode::ModeSpecific,
                speed: Some(0..=4),
                brightness: Some(0..=100),
                colors: Some(1..=2),
            },
            Mode {
                id: 2,
                name: "Rainbow".into(),
                caps: ModeCaps::SPEED | ModeCaps::DIRECTION_LR | ModeCaps::RANDOM_COLOR,
                color_mode: ColorMode::Random,
                speed: Some(0..=4),
                brightness: None,
                colors: None,
            },
            Mode {
                id: 3,
                name: "Off".into(),
                caps: ModeCaps::empty(),
                color_mode: ColorMode::None,
                speed: None,
                brightness: None,
                colors: None,
            },
        ],
        direct_mode: Some(0),
        keepalive: Some(Duration::from_millis(2500)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;
    use crate::mode::Direction;

    #[test]
    fn accepts_valid_description() {
        let d = sample_description();
        assert_eq!(d.validate(), Ok(()));
        assert_eq!(d.led_count(), 7);
        assert_eq!(d.zone_range(0), 0..1);
        assert_eq!(d.zone_range(1), 1..4);
        assert_eq!(d.zone_range(2), 4..7);
    }

    #[test]
    fn accepts_empty_resizable_zone_and_no_direct_mode() {
        let mut d = sample_description();
        d.zones[1].len = 0;
        d.zones[2].start = 1;
        d.leds.truncate(4);
        d.direct_mode = None;
        assert_eq!(d.validate(), Ok(()));
    }

    #[test]
    fn rejects_gap() {
        let mut d = sample_description();
        d.zones[1].start = 2;
        assert_eq!(
            d.validate(),
            Err(ModelError::ZoneGap {
                zone: 1,
                start: 2,
                expected: 1
            })
        );
    }

    #[test]
    fn rejects_overlap() {
        let mut d = sample_description();
        d.zones[2].start = 3;
        assert_eq!(
            d.validate(),
            Err(ModelError::ZoneOverlap {
                zone: 2,
                start: 3,
                expected: 4
            })
        );
    }

    #[test]
    fn rejects_unsorted() {
        let mut d = sample_description();
        d.zones.swap(1, 2);
        assert!(matches!(d.validate(), Err(ModelError::ZoneGap { zone: 1, .. })));
        let mut d = sample_description();
        d.zones[2].start = 0;
        assert_eq!(
            d.validate(),
            Err(ModelError::ZoneUnsorted {
                zone: 2,
                start: 0,
                prev: 1,
                prev_start: 1
            })
        );
    }

    #[test]
    fn rejects_zone_past_led_count() {
        let mut d = sample_description();
        d.zones[2].len = 4;
        assert_eq!(
            d.validate(),
            Err(ModelError::ZonePastEnd {
                zone: 2,
                end: 8,
                led_count: 7
            })
        );
    }

    #[test]
    fn rejects_uncovered_tail() {
        let mut d = sample_description();
        d.leds.push(Led { name: "x".into() });
        assert_eq!(
            d.validate(),
            Err(ModelError::ZonesIncomplete {
                covered: 7,
                led_count: 8
            })
        );
    }

    #[test]
    fn rejects_matrix_index_out_of_range() {
        let mut d = sample_description();
        d.zones[2].kind = ZoneKind::Matrix {
            width: 2,
            height: 2,
            map: vec![Some(0), Some(1), Some(3), None],
        };
        assert_eq!(
            d.validate(),
            Err(ModelError::MatrixIndexOutOfRange {
                zone: 2,
                cell: 2,
                index: 3,
                len: 3
            })
        );
    }

    #[test]
    fn rejects_matrix_map_size() {
        let mut d = sample_description();
        d.zones[2].kind = ZoneKind::Matrix {
            width: 3,
            height: 2,
            map: vec![Some(0), Some(1), Some(2)],
        };
        assert_eq!(
            d.validate(),
            Err(ModelError::MatrixMapSize {
                zone: 2,
                width: 3,
                height: 2,
                cells: 3
            })
        );
    }

    #[test]
    fn rejects_direct_mode_not_per_led() {
        let mut d = sample_description();
        d.direct_mode = Some(1);
        assert_eq!(d.validate(), Err(ModelError::DirectModeNotPerLed { id: 1 }));
    }

    #[test]
    fn rejects_unknown_direct_mode() {
        let mut d = sample_description();
        d.direct_mode = Some(42);
        assert_eq!(d.validate(), Err(ModelError::UnknownMode { id: 42 }));
    }

    #[test]
    fn rejects_duplicate_mode_ids() {
        let mut d = sample_description();
        d.modes[3].id = 1;
        assert_eq!(
            d.validate(),
            Err(ModelError::DuplicateModeId {
                id: 1,
                first: 1,
                second: 3
            })
        );
    }

    #[test]
    fn rejects_mode_specific_without_range() {
        let mut d = sample_description();
        d.modes[1].colors = None;
        assert_eq!(d.validate(), Err(ModelError::MissingColorRange { mode: 1 }));
    }

    #[test]
    fn rejects_len_outside_resizable() {
        let mut d = sample_description();
        d.zones[1].resizable = Some(4..=8);
        assert_eq!(
            d.validate(),
            Err(ModelError::LenOutsideResizable {
                zone: 1,
                len: 3,
                min: 4,
                max: 8
            })
        );
    }

    #[test]
    fn rejects_color_mode_without_cap() {
        let mut d = sample_description();
        d.modes[2].caps = ModeCaps::SPEED | ModeCaps::DIRECTION_LR;
        assert_eq!(d.validate(), Err(ModelError::ColorModeCapMismatch { mode: 2 }));
        let mut d = sample_description();
        d.modes[0].caps = ModeCaps::empty();
        assert_eq!(d.validate(), Err(ModelError::ColorModeCapMismatch { mode: 0 }));
        let mut d = sample_description();
        d.modes[1].caps = ModeCaps::SPEED | ModeCaps::BRIGHTNESS;
        assert_eq!(d.validate(), Err(ModelError::ColorModeCapMismatch { mode: 1 }));
    }

    #[test]
    fn zone_range_saturates() {
        let z = Zone {
            name: "x".into(),
            kind: ZoneKind::Linear,
            start: usize::MAX - 1,
            len: 5,
            resizable: None,
        };
        assert_eq!(z.range(), usize::MAX - 1..usize::MAX);
    }

    fn json_str<T: Serialize>(v: &T) -> String {
        serde_json::to_string(v).expect("serialize")
    }

    #[test]
    fn json_names_of_enums_are_pinned() {
        const KINDS: [(DeviceKind, &str); 22] = [
            (DeviceKind::Motherboard, "Motherboard"),
            (DeviceKind::Dram, "Dram"),
            (DeviceKind::Gpu, "Gpu"),
            (DeviceKind::Cooler, "Cooler"),
            (DeviceKind::LedStrip, "LedStrip"),
            (DeviceKind::Keyboard, "Keyboard"),
            (DeviceKind::Mouse, "Mouse"),
            (DeviceKind::Mousemat, "Mousemat"),
            (DeviceKind::Headset, "Headset"),
            (DeviceKind::HeadsetStand, "HeadsetStand"),
            (DeviceKind::Gamepad, "Gamepad"),
            (DeviceKind::Light, "Light"),
            (DeviceKind::Speaker, "Speaker"),
            (DeviceKind::Virtual, "Virtual"),
            (DeviceKind::Storage, "Storage"),
            (DeviceKind::Case, "Case"),
            (DeviceKind::Microphone, "Microphone"),
            (DeviceKind::Accessory, "Accessory"),
            (DeviceKind::Keypad, "Keypad"),
            (DeviceKind::Laptop, "Laptop"),
            (DeviceKind::Monitor, "Monitor"),
            (DeviceKind::Unknown, "Unknown"),
        ];
        // Exhaustive match without a wildcard: a new variant fails to compile until KINDS and this match are updated.
        let position = |k: DeviceKind| match k {
            DeviceKind::Motherboard => 0,
            DeviceKind::Dram => 1,
            DeviceKind::Gpu => 2,
            DeviceKind::Cooler => 3,
            DeviceKind::LedStrip => 4,
            DeviceKind::Keyboard => 5,
            DeviceKind::Mouse => 6,
            DeviceKind::Mousemat => 7,
            DeviceKind::Headset => 8,
            DeviceKind::HeadsetStand => 9,
            DeviceKind::Gamepad => 10,
            DeviceKind::Light => 11,
            DeviceKind::Speaker => 12,
            DeviceKind::Virtual => 13,
            DeviceKind::Storage => 14,
            DeviceKind::Case => 15,
            DeviceKind::Microphone => 16,
            DeviceKind::Accessory => 17,
            DeviceKind::Keypad => 18,
            DeviceKind::Laptop => 19,
            DeviceKind::Monitor => 20,
            DeviceKind::Unknown => 21,
        };
        for (i, (kind, name)) in KINDS.iter().enumerate() {
            assert_eq!(position(*kind), i, "{name} out of order");
            let json = json_str(kind);
            assert_eq!(json, format!("\"{name}\""));
            let back: DeviceKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, *kind);
        }

        for (v, name) in [(Bus::Hid, "Hid"), (Bus::NvApi, "NvApi"), (Bus::Smbus, "Smbus")] {
            assert_eq!(json_str(&v), format!("\"{name}\""));
        }
        for (v, name) in [
            (ColorMode::None, "None"),
            (ColorMode::PerLed, "PerLed"),
            (ColorMode::ModeSpecific, "ModeSpecific"),
            (ColorMode::Random, "Random"),
        ] {
            assert_eq!(json_str(&v), format!("\"{name}\""));
        }
        for (v, name) in [
            (Direction::Left, "Left"),
            (Direction::Right, "Right"),
            (Direction::Up, "Up"),
            (Direction::Down, "Down"),
            (Direction::Horizontal, "Horizontal"),
            (Direction::Vertical, "Vertical"),
        ] {
            assert_eq!(json_str(&v), format!("\"{name}\""));
        }
    }

    #[test]
    fn validate_state_looks_up_mode() {
        let d = sample_description();
        let direct = ModeState {
            mode: 0,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![Rgb::WHITE; 7],
        };
        assert_eq!(d.validate_state(&direct), Ok(()));
        let short = ModeState {
            colors: vec![Rgb::WHITE; 6],
            ..direct.clone()
        };
        assert!(matches!(
            d.validate_state(&short),
            Err(ModelError::ColorCount {
                mode: 0,
                actual: 6,
                ..
            })
        ));
        let rainbow = ModeState {
            mode: 2,
            speed: Some(4),
            brightness: None,
            direction: Some(Direction::Right),
            colors: vec![],
        };
        assert_eq!(d.validate_state(&rainbow), Ok(()));
        let unknown = ModeState { mode: 9, ..rainbow };
        assert_eq!(d.validate_state(&unknown), Err(ModelError::UnknownMode { id: 9 }));
    }

    #[test]
    fn json_round_trip_with_keepalive_in_ms() {
        let d = sample_description();
        let json = serde_json::to_value(&d).expect("serialize");
        assert_eq!(json["keepalive"], serde_json::json!(2500));
        assert_eq!(json["info"]["id"], serde_json::json!("hid:1462:7d75"));
        assert_eq!(
            json["modes"][1]["caps"],
            serde_json::json!("SPEED | BRIGHTNESS | MODE_SPECIFIC_COLOR")
        );
        let back: DeviceDescription = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, d);
    }

    #[test]
    fn json_keepalive_none_and_missing() {
        let mut d = sample_description();
        d.keepalive = None;
        let mut json = serde_json::to_value(&d).expect("serialize");
        assert_eq!(json["keepalive"], serde_json::Value::Null);
        let back: DeviceDescription = serde_json::from_value(json.clone()).expect("deserialize null");
        assert_eq!(back, d);
        json.as_object_mut().expect("object").remove("keepalive");
        let back: DeviceDescription = serde_json::from_value(json).expect("deserialize missing");
        assert_eq!(back, d);
    }
}
