//! Brightness and All off (ADR 0001 #33): what a device is actually sent for what the engine wants it to show.
//!
//! The engine keeps every setting unscaled (colour buffer, [`DeviceSetting`], `current_settings()`), so turning a
//! brightness back up restores the colours exactly. Only the output that goes to the device is scaled:
//!
//! - **Direct colours:** every channel is multiplied by device brightness x master brightness x fade factor and
//!   rounded. At a factor of 0 the device is sent black.
//! - **Hardware modes with a brightness parameter** ([`ModeCaps::BRIGHTNESS`]): the same factor scales the mode's own
//!   brightness inside its range, from the range minimum (factor 0) up to the stored brightness (factor 1; the range
//!   maximum when the state has none). A mode that declares the capability without a range is taken as `0..=100`.
//! - **Hardware modes without one** are sent unchanged; device and master brightness do not affect them.
//! - **Off** (fade factor 0) in a hardware mode: the device's mode named "Off" if it has one that takes no colours,
//!   else black in its direct mode; a device with neither keeps its mode at the lowest brightness.

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
    /// A hardware mode as the user set it.
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

/// The three brightness levels that multiply into one device's output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Levels {
    /// Device brightness, percent.
    pub device: u8,
    /// Master brightness, percent.
    pub master: u8,
    /// Fade factor, 0 (off) to 1 (on).
    pub fade: f64,
}

impl Levels {
    /// Everything at full: the output equals the stored setting.
    #[cfg(test)]
    pub(crate) const FULL: Levels = Levels {
        device: FULL,
        master: FULL,
        fade: 1.0,
    };

    /// The product of the three levels, 0 to 1.
    pub(crate) fn factor(&self) -> f64 {
        let pct = |v: u8| f64::from(v.min(FULL)) / f64::from(FULL);
        pct(self.device) * pct(self.master) * self.fade.clamp(0.0, 1.0)
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
            if levels.fade <= 0.0
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

    fn levels(device: u8, master: u8, fade: f64) -> Levels {
        Levels { device, master, fade }
    }

    #[test]
    fn factor_multiplies_and_clamps() {
        assert_eq!(Levels::FULL.factor(), 1.0);
        assert_eq!(levels(50, 50, 1.0).factor(), 0.25);
        assert_eq!(levels(100, 100, 0.5).factor(), 0.5);
        assert_eq!(levels(0, 100, 1.0).factor(), 0.0);
        assert_eq!(levels(200, 100, 3.0).factor(), 1.0);
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
        let o = output(&d, &Want::Direct, &colors, levels(50, 100, 1.0));
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
        let off = output(&d, &Want::Direct, &colors, levels(100, 100, 0.0));
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
        let o = output(&d, &want, &[], levels(50, 100, 1.0));
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
            output(&d, &w, &[], levels(10, 10, 0.5)).out,
            Out::Mode(state(3, None, vec![]))
        );
        // Off: the "Off" mode, else direct black, else the lowest brightness.
        assert_eq!(
            output(&d, &want, &[], levels(100, 100, 0.0)).out,
            Out::Mode(state(9, None, vec![]))
        );
        let d = desc(vec![direct, breathing.clone()], Some(0));
        assert_eq!(
            output(&d, &want, &[], levels(100, 100, 0.0)).out,
            Out::Direct(vec![Rgb::BLACK; 2])
        );
        let d = desc(vec![breathing], None);
        assert_eq!(
            output(&d, &want, &[], levels(100, 100, 0.0)).out,
            Out::Mode(state(2, Some(1), vec![red]))
        );
    }
}
