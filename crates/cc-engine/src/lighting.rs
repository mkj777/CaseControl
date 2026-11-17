//! Brightness (ADR 0001 #33, #35): what a device is actually sent for what the engine wants it to show.
//!
//! The engine keeps every setting unscaled (colour buffer, [`DeviceSetting`], `current_settings()`), so turning a
//! brightness back up restores the colours exactly. Only the output that goes to the device is scaled:
//!
//! - **Direct colours:** every channel is multiplied by device brightness x master brightness and rounded. At a
//!   factor of 0 the device is sent black.
//! - **Hardware modes with a brightness parameter** ([`ModeCaps::BRIGHTNESS`]): the same factor scales the mode's own
//!   brightness inside its range, from the range minimum (factor 0) up to the stored brightness (factor 1; the range
//!   maximum when the state has none). A mode that declares the capability without a range is taken as `0..=100`.
//! - **Hardware modes without one** are sent unchanged; device and master brightness do not affect them.
//! - **Off** (a factor of 0, i.e. device or master brightness 0; ADR 0001 #35) in a hardware mode: the device's mode
//!   named "Off" if it has one that takes no colours, else black in its direct mode; a device with neither keeps its
//!   mode at the lowest brightness.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

use cc_config::DeviceSetting;
use cc_core::{ColorMode, DeviceDescription, Mode, ModeCaps, ModeState, Rgb};

/// Full brightness in percent.
pub(crate) const FULL: u8 = 100;

/// What the engine drives a device with, unscaled. Direct colours live in the slot's colour buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Want {
    /// The colour buffer in direct mode.
    Direct,
    /// A hardware mode as the user set it (or as the device showed it on its own when a brightness change took it
    /// over).
    Mode(ModeState),
}

/// What goes to the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Out {
    /// A direct colour frame, one colour per LED.
    Direct(Vec<Rgb>),
    /// A hardware mode.
    Mode(ModeState),
}

/// One output for a device: what is sent (`out`) and the unscaled setting the engine records once it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Output {
    /// The unscaled setting (recorded as the device's setting after the write).
    pub stored: DeviceSetting,
    /// What the device is sent.
    pub out: Out,
}

/// The two brightness levels that multiply into one device's output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Levels {
    /// Device brightness, percent.
    pub device: u8,
    /// Master brightness, percent.
    pub master: u8,
}

impl Levels {
    /// Everything at full: the output equals the stored setting.
    #[cfg(test)]
    pub(crate) const FULL: Levels = Levels {
        device: FULL,
        master: FULL,
    };

    /// The product of the levels, 0 to 1.
    pub(crate) fn factor(&self) -> f64 {
        let pct = |v: u8| f64::from(v.min(FULL)) / f64::from(FULL);
        pct(self.device) * pct(self.master)
    }
}

/// `colors` with every channel multiplied by `factor` (0 to 1) and rounded. A factor of 1 returns them unchanged.
pub(crate) fn scale_colors(colors: &[Rgb], factor: f64) -> Vec<Rgb> {
    let k = factor.clamp(0.0, 1.0);
    let ch = |c: u8| -> u8 {
        // In 0..=255 by construction (c <= 255, k <= 1); the cast only drops the fraction rounding removed.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = (f64::from(c) * k).round() as u8;
        v
    };
    colors
        .iter()
        .map(|c| Rgb::new(ch(c.r), ch(c.g), ch(c.b)))
        .collect()
}

/// The brightness range a mode's brightness lives in, if the mode has a brightness parameter.
pub(crate) fn brightness_range(mode: &Mode) -> Option<RangeInclusive<u32>> {
    mode.caps
        .contains(ModeCaps::BRIGHTNESS)
        .then(|| mode.brightness.clone().unwrap_or(0..=100))
}

/// The mode brightness for `factor`: from the range minimum (0) up to `stored` (1; the range maximum without one).
pub(crate) fn scaled_brightness(range: &RangeInclusive<u32>, stored: Option<u32>, factor: f64) -> u32 {
    let (min, max) = (*range.start(), *range.end());
    let top = stored.unwrap_or(max).clamp(min, max);
    let span = f64::from(top - min) * factor.clamp(0.0, 1.0);
    // In 0..=span by construction; the cast only drops the fraction rounding removed.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let step = span.round() as u32;
    min + step
}

/// What a device shows when it is off: its "Off" mode (one that takes no colours), else black in direct mode.
pub(crate) fn off_output(desc: &DeviceDescription) -> Option<Out> {
    let off = desc.modes.iter().find(|m| {
        m.name.eq_ignore_ascii_case("off") && matches!(m.color_mode, ColorMode::None | ColorMode::Random)
    });
    if let Some(m) = off {
        let state = ModeState {
            mode: m.id,
            speed: None,
            brightness: None,
            direction: None,
            colors: Vec::new(),
        };
        if desc.validate_state(&state).is_ok() {
            return Some(Out::Mode(state));
        }
    }
    desc.direct_mode
        .map(|_| Out::Direct(vec![Rgb::BLACK; desc.led_count()]))
}

/// What device `desc` is sent for `want` (direct colours from `colors`) at `levels`.
pub(crate) fn output(desc: &DeviceDescription, want: &Want, colors: &[Rgb], levels: Levels) -> Output {
    let factor = levels.factor();
    match want {
        Want::Direct => Output {
            stored: DeviceSetting::Direct {
                colors: colors.to_vec(),
            },
            out: Out::Direct(scale_colors(colors, factor)),
        },
        Want::Mode(state) => {
            let stored = DeviceSetting::Mode { state: state.clone() };
            let range = desc.mode(state.mode).and_then(brightness_range);
            if factor <= 0.0
                && let Some(off) = off_output(desc)
            {
                return Output { stored, out: off };
            }
            let out = match range {
                Some(range) => ModeState {
                    brightness: Some(scaled_brightness(&range, state.brightness, factor)),
                    ..state.clone()
                },
                None => state.clone(),
            };
            Output {
                stored,
                out: Out::Mode(out),
            }
        }
    }
}

/// Sets the LEDs of every zone of `desc` named in `zones` to black: the app's hidden zones (ADR 0001 #26), which the
/// engine keeps black in every direct output (its dark zones, see
/// [`EngineHandle::set_dark_zones`](crate::EngineHandle::set_dark_zones)). Slices beyond the end of `colors` are
/// ignored, so a buffer that does not match the layout cannot panic here.
pub fn mask_hidden_zones(desc: &DeviceDescription, zones: &BTreeSet<String>, colors: &mut [Rgb]) {
    if zones.is_empty() {
        return;
    }
    let len = colors.len();
    for zone in desc.zones.iter().filter(|z| zones.contains(&z.name)) {
        let range = zone.range();
        let (start, end) = (range.start.min(len), range.end.min(len));
        colors[start..end].fill(Rgb::BLACK);
    }
}

/// What a takeover drives a device with when it has neither a setting nor a hardware readback (ADR 0001 #36):
/// `color` on every LED in its direct mode, else its first mode that takes a colour (per LED, or mode-wide with room
/// for at least one) set to `color`, with speed in the middle and brightness at the top of their ranges. LEDs of the
/// zones named in `dark` stay black where colours are per LED. `None` when the device has neither a direct mode nor
/// such a mode.
pub(crate) fn default_setting(
    desc: &DeviceDescription,
    color: Rgb,
    dark: &BTreeSet<String>,
) -> Option<DeviceSetting> {
    let mut leds = vec![color; desc.led_count()];
    mask_hidden_zones(desc, dark, &mut leds);
    if desc.direct_mode.is_some() {
        return Some(DeviceSetting::Direct { colors: leds });
    }
    desc.modes.iter().find_map(|m| {
        let colors = match m.color_mode {
            ColorMode::PerLed => leds.clone(),
            ColorMode::ModeSpecific => {
                let range = m.colors.as_ref().filter(|r| *r.end() >= 1)?;
                vec![color; (*range.start()).max(1)]
            }
            ColorMode::None | ColorMode::Random => return None,
        };
        // An inverted range holds no value, so the state below fails validation and the mode is skipped.
        let state = ModeState {
            mode: m.id,
            speed: m
                .caps
                .contains(ModeCaps::SPEED)
                .then(|| {
                    m.speed
                        .as_ref()
                        .map(|r| r.start() + r.end().saturating_sub(*r.start()) / 2)
                })
                .flatten(),
            brightness: m
                .caps
                .contains(ModeCaps::BRIGHTNESS)
                .then(|| m.brightness.as_ref().map(|r| *r.end()))
                .flatten(),
            direction: None,
            colors,
        };
        desc.validate_state(&state)
            .is_ok()
            .then_some(DeviceSetting::Mode { state })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(
        id: u32,
        name: &str,
        caps: ModeCaps,
        color_mode: ColorMode,
        brightness: Option<RangeInclusive<u32>>,
    ) -> Mode {
        Mode {
            id,
            name: name.into(),
            caps,
            color_mode,
            speed: None,
            brightness,
            colors: (color_mode == ColorMode::ModeSpecific).then_some(1..=1),
        }
    }

    fn state(mode: u32, brightness: Option<u32>, colors: Vec<Rgb>) -> ModeState {
        ModeState {
            mode,
            speed: None,
            brightness,
            direction: None,
            colors,
        }
    }

    fn desc(modes: Vec<Mode>, direct: Option<u32>) -> DeviceDescription {
        use cc_core::{Bus, DeviceId, DeviceInfo, DeviceKind, Led, Zone, ZoneKind};
        DeviceDescription {
            info: DeviceInfo {
                id: DeviceId::hid(1, 2, None),
                name: "T".into(),
                vendor: "T".into(),
                kind: DeviceKind::Light,
                bus: Bus::Hid,
                location: "p".into(),
                serial: None,
                firmware: None,
            },
            zones: vec![Zone {
                name: "Z".into(),
                kind: ZoneKind::Linear,
                start: 0,
                len: 2,
                resizable: None,
            }],
            leds: vec![Led { name: "a".into() }, Led { name: "b".into() }],
            modes,
            direct_mode: direct,
            keepalive: None,
        }
    }

    fn levels(device: u8, master: u8) -> Levels {
        Levels { device, master }
    }

    #[test]
    fn factor_multiplies_and_clamps() {
        assert_eq!(Levels::FULL.factor(), 1.0);
        assert_eq!(levels(50, 50).factor(), 0.25);
        assert_eq!(levels(100, 50).factor(), 0.5);
        assert_eq!(levels(0, 100).factor(), 0.0);
        assert_eq!(levels(100, 0).factor(), 0.0);
        assert_eq!(levels(200, 100).factor(), 1.0);
    }

    #[test]
    fn colors_scale_and_round() {
        let c = [Rgb::new(255, 128, 1), Rgb::new(10, 0, 3)];
        assert_eq!(scale_colors(&c, 1.0), c.to_vec());
        assert_eq!(scale_colors(&c, 0.0), vec![Rgb::BLACK; 2]);
        // 255 * 0.5 = 127.5 -> 128, 128 * 0.5 = 64, 1 * 0.5 = 0.5 -> 1 (round half away from zero).
        assert_eq!(
            scale_colors(&c, 0.5),
            vec![Rgb::new(128, 64, 1), Rgb::new(5, 0, 2)]
        );
        assert_eq!(
            scale_colors(&c, 0.25),
            vec![Rgb::new(64, 32, 0), Rgb::new(3, 0, 1)]
        );
    }

    #[test]
    fn mode_brightness_maps_into_the_range() {
        let r = 1..=100;
        assert_eq!(scaled_brightness(&r, None, 1.0), 100);
        assert_eq!(scaled_brightness(&r, None, 0.0), 1);
        assert_eq!(scaled_brightness(&r, Some(51), 0.5), 26);
        assert_eq!(
            scaled_brightness(&r, Some(500), 1.0),
            100,
            "stored value clamps to the range"
        );
        assert_eq!(scaled_brightness(&(0..=4), Some(4), 0.5), 2);
    }

    #[test]
    fn direct_output_scales_and_keeps_stored_colours() {
        let d = desc(
            vec![mode(
                0,
                "Direct",
                ModeCaps::PER_LED_COLOR,
                ColorMode::PerLed,
                None,
            )],
            Some(0),
        );
        let colors = [Rgb::new(200, 100, 50), Rgb::WHITE];
        let o = output(&d, &Want::Direct, &colors, levels(50, 100));
        assert_eq!(
            o.stored,
            DeviceSetting::Direct {
                colors: colors.to_vec()
            }
        );
        assert_eq!(
            o.out,
            Out::Direct(vec![Rgb::new(100, 50, 25), Rgb::new(128, 128, 128)])
        );
        let full = output(&d, &Want::Direct, &colors, Levels::FULL);
        assert_eq!(full.out, Out::Direct(colors.to_vec()));
        let off = output(&d, &Want::Direct, &colors, levels(0, 100));
        assert_eq!(off.out, Out::Direct(vec![Rgb::BLACK; 2]));
    }

    #[test]
    fn hardware_mode_brightness_and_off_fallbacks() {
        let breathing = mode(
            2,
            "Breathing",
            ModeCaps::BRIGHTNESS | ModeCaps::MODE_SPECIFIC_COLOR,
            ColorMode::ModeSpecific,
            Some(1..=100),
        );
        let wave = mode(3, "Wave", ModeCaps::SPEED, ColorMode::None, None);
        let off = mode(9, "Off", ModeCaps::empty(), ColorMode::None, None);
        let direct = mode(0, "Direct", ModeCaps::PER_LED_COLOR, ColorMode::PerLed, None);
        let red = Rgb::new(255, 0, 0);
        let want = Want::Mode(state(2, Some(80), vec![red]));

        let d = desc(
            vec![direct.clone(), breathing.clone(), wave.clone(), off.clone()],
            Some(0),
        );
        let o = output(&d, &want, &[], levels(50, 100));
        assert_eq!(
            o.stored,
            DeviceSetting::Mode {
                state: state(2, Some(80), vec![red])
            }
        );
        // 1 + (80 - 1) * 0.5 = 40.5 -> 41
        assert_eq!(o.out, Out::Mode(state(2, Some(41), vec![red])));
        assert_eq!(
            output(&d, &want, &[], Levels::FULL).out,
            Out::Mode(state(2, Some(80), vec![red]))
        );
        // Without a brightness parameter the mode goes out unchanged.
        let w = Want::Mode(state(3, None, vec![]));
        assert_eq!(
            output(&d, &w, &[], levels(10, 10)).out,
            Out::Mode(state(3, None, vec![]))
        );
        // Off: the "Off" mode, else direct black, else the lowest brightness.
        assert_eq!(
            output(&d, &want, &[], levels(0, 100)).out,
            Out::Mode(state(9, None, vec![]))
        );
        // Master brightness 0 is off too, and so is 0 for a mode without a brightness parameter.
        assert_eq!(
            output(&d, &want, &[], levels(100, 0)).out,
            Out::Mode(state(9, None, vec![]))
        );
        assert_eq!(
            output(&d, &w, &[], levels(0, 100)).out,
            Out::Mode(state(9, None, vec![]))
        );
        let d = desc(vec![direct, breathing.clone()], Some(0));
        assert_eq!(
            output(&d, &want, &[], levels(0, 100)).out,
            Out::Direct(vec![Rgb::BLACK; 2])
        );
        let d = desc(vec![breathing], None);
        assert_eq!(
            output(&d, &want, &[], levels(0, 100)).out,
            Out::Mode(state(2, Some(1), vec![red]))
        );
    }

    /// Board (1 LED), "A" (3 LEDs), "B" (2 LEDs).
    fn zoned(direct: bool, modes: Vec<Mode>) -> DeviceDescription {
        use cc_core::{Led, Zone, ZoneKind};
        let zone = |name: &str, start, len| Zone {
            name: name.into(),
            kind: ZoneKind::Linear,
            start,
            len,
            resizable: None,
        };
        let mut d = desc(modes, direct.then_some(0));
        d.zones = vec![zone("Board", 0, 1), zone("A", 1, 3), zone("B", 4, 2)];
        d.leds = (0..6).map(|i| Led { name: format!("{i}") }).collect();
        d
    }

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    const R: Rgb = Rgb::new(255, 0, 0);
    const B: Rgb = Rgb::BLACK;

    #[test]
    fn mask_blacks_out_named_zones_only_and_tolerates_short_buffers() {
        let d = zoned(true, Vec::new());
        let mut colors = vec![R; 6];
        mask_hidden_zones(&d, &names(&["Board", "B"]), &mut colors);
        assert_eq!(colors, vec![B, R, R, R, B, B]);

        let mut colors = vec![R; 6];
        mask_hidden_zones(&d, &BTreeSet::new(), &mut colors);
        mask_hidden_zones(&d, &names(&["NOPE"]), &mut colors);
        assert_eq!(colors, vec![R; 6], "no or unknown zones change nothing");

        let mut short = vec![R; 5];
        mask_hidden_zones(&d, &names(&["B"]), &mut short);
        assert_eq!(short, vec![R, R, R, R, B]);
        let mut empty: Vec<Rgb> = Vec::new();
        mask_hidden_zones(&d, &names(&["B"]), &mut empty);
        assert!(empty.is_empty());
    }

    fn colored(id: u32, colors: RangeInclusive<usize>) -> Mode {
        Mode {
            colors: Some(colors),
            ..mode(
                id,
                "M",
                ModeCaps::MODE_SPECIFIC_COLOR | ModeCaps::BRIGHTNESS,
                ColorMode::ModeSpecific,
                Some(1..=10),
            )
        }
    }

    #[test]
    fn default_setting_is_direct_colour_with_dark_zones_black() {
        let direct = mode(0, "Direct", ModeCaps::PER_LED_COLOR, ColorMode::PerLed, None);
        let d = zoned(true, vec![direct]);
        assert_eq!(
            default_setting(&d, R, &names(&["A"])),
            Some(DeviceSetting::Direct {
                colors: vec![R, B, B, B, R, R]
            })
        );
    }

    #[test]
    fn default_setting_skips_modes_without_room_for_a_colour() {
        // 0..=0 colours (no colour at all) and an inverted range are skipped; 1..=3 gets one colour.
        #[allow(clippy::reversed_empty_ranges)]
        let inverted = colored(2, 3..=1);
        let d = zoned(false, vec![colored(1, 0..=0), inverted, colored(3, 1..=3)]);
        assert_eq!(
            default_setting(&d, R, &BTreeSet::new()),
            Some(DeviceSetting::Mode {
                state: state(3, Some(10), vec![R])
            })
        );
        let d = zoned(false, vec![colored(1, 0..=0)]);
        assert_eq!(default_setting(&d, R, &BTreeSet::new()), None);
        // Per-LED colours of a hardware mode keep the dark zones black.
        let per_led = mode(5, "Static", ModeCaps::PER_LED_COLOR, ColorMode::PerLed, None);
        let d = zoned(false, vec![per_led]);
        assert_eq!(
            default_setting(&d, R, &names(&["Board"])),
            Some(DeviceSetting::Mode {
                state: state(5, None, vec![B, R, R, R, R, R])
            })
        );
    }
}
