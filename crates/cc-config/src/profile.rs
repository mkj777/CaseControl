//! Profile types shared with `cc-engine`. Defined by the orchestrator; their shape is a contract between crates.

use std::collections::BTreeMap;

use cc_core::{DeviceId, ModeState, Rgb};
use serde::{Deserialize, Serialize};

/// What one device shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeviceSetting {
    /// Static per-LED colors in the device's direct mode. `colors.len()` was the LED count when the setting was made;
    /// on apply, a shorter buffer is padded with its last color and a longer one truncated.
    Direct {
        /// One color per LED, in the device's flat LED order.
        colors: Vec<Rgb>,
    },
    /// A hardware mode (effect) with its parameters.
    Mode {
        /// Mode id and parameters as defined by the device description.
        state: ModeState,
    },
}

/// A named set of device settings. Devices not listed are left as they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Display name; also the basis for the file name.
    pub name: String,
    /// Setting per device.
    #[serde(default)]
    pub devices: BTreeMap<DeviceId, DeviceSetting>,
}

impl DeviceSetting {
    /// This setting adapted to a device with `led_count` LEDs.
    ///
    /// A [`DeviceSetting::Direct`] buffer is padded with its last color or truncated to `led_count`; an empty buffer
    /// becomes all black. A [`DeviceSetting::Mode`] is returned unchanged (the device validates its colors).
    pub fn fit_to(&self, led_count: usize) -> DeviceSetting {
        match self {
            DeviceSetting::Direct { colors } => {
                let fill = colors.last().copied().unwrap_or(Rgb::BLACK);
                let mut colors: Vec<Rgb> = colors.iter().copied().take(led_count).collect();
                colors.resize(led_count, fill);
                DeviceSetting::Direct { colors }
            }
            DeviceSetting::Mode { .. } => self.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use cc_core::Direction;

    use super::*;

    const RED: Rgb = Rgb::new(0xFF, 0, 0);
    const GREEN: Rgb = Rgb::new(0, 0xFF, 0);

    fn direct(colors: Vec<Rgb>) -> DeviceSetting {
        DeviceSetting::Direct { colors }
    }

    fn sample() -> Profile {
        let mut devices = BTreeMap::new();
        devices.insert(
            DeviceId::hid(0x31E3, 0x1310, Some("A1B2C3")),
            direct(vec![RED, GREEN, Rgb::new(0x12, 0x34, 0x56)]),
        );
        devices.insert(
            DeviceId::nvapi(0x2C02_10DE, 0x2095_10DE),
            DeviceSetting::Mode {
                state: ModeState {
                    mode: 3,
                    speed: Some(2),
                    brightness: Some(100),
                    direction: Some(Direction::Left),
                    colors: vec![RED, GREEN],
                },
            },
        );
        devices.insert(
            DeviceId::hid(0x046D, 0x0A78, None),
            DeviceSetting::Mode {
                state: ModeState {
                    mode: 0,
                    speed: None,
                    brightness: None,
                    direction: None,
                    colors: vec![],
                },
            },
        );
        Profile {
            name: "Evening".into(),
            devices,
        }
    }

    /// The on-disk format of a profile. Changing this string means changing the file format users have on disk.
    const SAMPLE_TOML: &str = r##"name = "Evening"

[devices."hid:046d:0a78"]
kind = "mode"

[devices."hid:046d:0a78".state]
mode = 0
colors = []

[devices."hid:31e3:1310:A1B2C3"]
kind = "direct"
colors = ["#FF0000", "#00FF00", "#123456"]

[devices."nvapi:10de:2c02:10de:2095"]
kind = "mode"

[devices."nvapi:10de:2c02:10de:2095".state]
mode = 3
speed = 2
brightness = 100
direction = "Left"
colors = ["#FF0000", "#00FF00"]
"##;

    #[test]
    fn toml_snapshot() {
        let text = toml::to_string(&sample()).expect("serialize");
        assert_eq!(text, SAMPLE_TOML);
    }

    #[test]
    fn round_trip() {
        let p = sample();
        let text = toml::to_string(&p).expect("serialize");
        assert_eq!(toml::from_str::<Profile>(&text).expect("parse"), p);
        assert_eq!(toml::from_str::<Profile>(SAMPLE_TOML).expect("parse snapshot"), p);
    }

    #[test]
    fn profile_without_devices_loads() {
        let p: Profile = toml::from_str("name = \"Empty\"").expect("parse");
        assert!(p.devices.is_empty());
    }

    #[test]
    fn fit_to_pads_truncates_and_fills_black() {
        let d = direct(vec![RED, GREEN]);
        assert_eq!(d.fit_to(2), d);
        assert_eq!(d.fit_to(4), direct(vec![RED, GREEN, GREEN, GREEN]));
        assert_eq!(d.fit_to(1), direct(vec![RED]));
        assert_eq!(d.fit_to(0), direct(vec![]));
        assert_eq!(direct(vec![]).fit_to(3), direct(vec![Rgb::BLACK; 3]));
    }

    #[test]
    fn fit_to_leaves_modes_alone() {
        let p = sample();
        for setting in p.devices.values() {
            if let DeviceSetting::Mode { .. } = setting {
                assert_eq!(&setting.fit_to(1), setting);
            }
        }
    }
}
