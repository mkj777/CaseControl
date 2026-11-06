//! Lighting modes: capability flags, mode descriptions and the state a mode is set to.

use std::ops::RangeInclusive;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::color::Rgb;
use crate::error::ModelError;

bitflags! {
    /// What a [`Mode`] lets the user adjust. Mirrors OpenRGB's `MODE_FLAG_*`.
    ///
    /// Serializes as text in human-readable formats (`"SPEED | BRIGHTNESS"`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct ModeCaps: u32 {
        /// The mode has a speed parameter.
        const SPEED = 1 << 0;
        /// The mode runs left or right.
        const DIRECTION_LR = 1 << 1;
        /// The mode runs up or down.
        const DIRECTION_UD = 1 << 2;
        /// The mode runs horizontally or vertically.
        const DIRECTION_HV = 1 << 3;
        /// The mode has a brightness parameter.
        const BRIGHTNESS = 1 << 4;
        /// The mode takes one color per LED.
        const PER_LED_COLOR = 1 << 5;
        /// The mode takes a small set of mode-wide colors.
        const MODE_SPECIFIC_COLOR = 1 << 6;
        /// The mode can pick random colors.
        const RANDOM_COLOR = 1 << 7;
        /// The mode can be saved to the device on request.
        const SAVE_MANUAL = 1 << 8;
        /// The device saves the mode automatically.
        const SAVE_AUTOMATIC = 1 << 9;
    }
}

/// Where a mode's colors come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorMode {
    /// The mode takes no colors.
    None,
    /// One color per LED; `ModeState::colors` holds the whole LED buffer.
    PerLed,
    /// A small set of mode-wide colors, counted by [`Mode::colors`].
    ModeSpecific,
    /// The device picks random colors; no colors are passed.
    Random,
}

/// Direction of an animated effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    /// Needs [`ModeCaps::DIRECTION_LR`].
    Left,
    /// Needs [`ModeCaps::DIRECTION_LR`].
    Right,
    /// Needs [`ModeCaps::DIRECTION_UD`].
    Up,
    /// Needs [`ModeCaps::DIRECTION_UD`].
    Down,
    /// Needs [`ModeCaps::DIRECTION_HV`].
    Horizontal,
    /// Needs [`ModeCaps::DIRECTION_HV`].
    Vertical,
}

impl Direction {
    /// The capability flag a mode must have to accept this direction.
    pub fn required_cap(self) -> ModeCaps {
        match self {
            Self::Left | Self::Right => ModeCaps::DIRECTION_LR,
            Self::Up | Self::Down => ModeCaps::DIRECTION_UD,
            Self::Horizontal | Self::Vertical => ModeCaps::DIRECTION_HV,
        }
    }
}

/// A lighting mode a device offers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mode {
    /// Driver-defined id, unique within a device.
    pub id: u32,
    /// Display name.
    pub name: String,
    /// What can be adjusted.
    pub caps: ModeCaps,
    /// Where the colors come from.
    pub color_mode: ColorMode,
    /// Allowed speed values, if the device constrains them.
    pub speed: Option<RangeInclusive<u32>>,
    /// Allowed brightness values, if the device constrains them.
    pub brightness: Option<RangeInclusive<u32>>,
    /// Allowed number of mode-wide colors; required for [`ColorMode::ModeSpecific`].
    pub colors: Option<RangeInclusive<usize>>,
}

/// The state a mode is set to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModeState {
    /// Id of the [`Mode`].
    pub mode: u32,
    /// Speed, only if the mode has [`ModeCaps::SPEED`].
    pub speed: Option<u32>,
    /// Brightness, only if the mode has [`ModeCaps::BRIGHTNESS`].
    pub brightness: Option<u32>,
    /// Direction, only if the mode has the matching direction capability.
    pub direction: Option<Direction>,
    /// Colors as required by the mode's [`ColorMode`].
    pub colors: Vec<Rgb>,
}

impl Mode {
    /// Checks that `state` is a valid state of this mode on a device with `led_count` LEDs.
    ///
    /// Speed and brightness may only be present if the matching capability is set, and then must lie inside the
    /// declared range (if any). A direction needs its capability. The color count must be inside [`Mode::colors`]
    /// for [`ColorMode::ModeSpecific`], equal to `led_count` for [`ColorMode::PerLed`] and zero for
    /// [`ColorMode::None`] and [`ColorMode::Random`].
    pub fn validate_state(&self, state: &ModeState, led_count: usize) -> Result<(), ModelError> {
        if state.mode != self.id {
            return Err(ModelError::ModeMismatch {
                mode: self.id,
                state_mode: state.mode,
            });
        }
        self.check_param("speed", ModeCaps::SPEED, self.speed.as_ref(), state.speed)?;
        self.check_param(
            "brightness",
            ModeCaps::BRIGHTNESS,
            self.brightness.as_ref(),
            state.brightness,
        )?;
        if let Some(direction) = state.direction
            && !self.caps.contains(direction.required_cap())
        {
            return Err(ModelError::DirectionUnsupported {
                mode: self.id,
                direction,
            });
        }
        let (min, max) = match self.color_mode {
            ColorMode::None | ColorMode::Random => (0, 0),
            ColorMode::PerLed => (led_count, led_count),
            ColorMode::ModeSpecific => {
                let range = self
                    .colors
                    .as_ref()
                    .ok_or(ModelError::MissingColorRange { mode: self.id })?;
                (*range.start(), *range.end())
            }
        };
        let actual = state.colors.len();
        if !(min..=max).contains(&actual) {
            return Err(ModelError::ColorCount {
                mode: self.id,
                actual,
                min,
                max,
            });
        }
        Ok(())
    }

    fn check_param(
        &self,
        param: &'static str,
        cap: ModeCaps,
        range: Option<&RangeInclusive<u32>>,
        value: Option<u32>,
    ) -> Result<(), ModelError> {
        let Some(value) = value else { return Ok(()) };
        if !self.caps.contains(cap) {
            return Err(ModelError::ParamUnsupported { mode: self.id, param });
        }
        match range {
            Some(range) if !range.contains(&value) => Err(ModelError::ParamOutOfRange {
                mode: self.id,
                param,
                value,
                min: *range.start(),
                max: *range.end(),
            }),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(caps: ModeCaps, color_mode: ColorMode) -> Mode {
        Mode {
            id: 7,
            name: "Test".into(),
            caps,
            color_mode,
            speed: Some(1..=5),
            brightness: Some(0..=100),
            colors: Some(1..=2),
        }
    }

    fn state() -> ModeState {
        ModeState {
            mode: 7,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![],
        }
    }

    #[test]
    fn empty_state_of_colorless_mode_is_valid() {
        let m = mode(ModeCaps::empty(), ColorMode::None);
        assert_eq!(m.validate_state(&state(), 10), Ok(()));
    }

    #[test]
    fn rejects_state_for_other_mode() {
        let m = mode(ModeCaps::empty(), ColorMode::None);
        let s = ModeState { mode: 8, ..state() };
        assert_eq!(
            m.validate_state(&s, 10),
            Err(ModelError::ModeMismatch {
                mode: 7,
                state_mode: 8
            })
        );
    }

    #[test]
    fn speed_needs_cap_and_range() {
        let s = ModeState {
            speed: Some(3),
            ..state()
        };
        let without = mode(ModeCaps::empty(), ColorMode::None);
        assert_eq!(
            without.validate_state(&s, 0),
            Err(ModelError::ParamUnsupported {
                mode: 7,
                param: "speed"
            })
        );
        let with = mode(ModeCaps::SPEED, ColorMode::None);
        assert_eq!(with.validate_state(&s, 0), Ok(()));
        let too_fast = ModeState {
            speed: Some(6),
            ..state()
        };
        assert_eq!(
            with.validate_state(&too_fast, 0),
            Err(ModelError::ParamOutOfRange {
                mode: 7,
                param: "speed",
                value: 6,
                min: 1,
                max: 5
            })
        );
        let unbounded = Mode { speed: None, ..with };
        assert_eq!(unbounded.validate_state(&too_fast, 0), Ok(()));
    }

    #[test]
    fn brightness_needs_cap_and_range() {
        let s = ModeState {
            brightness: Some(101),
            ..state()
        };
        assert_eq!(
            mode(ModeCaps::SPEED, ColorMode::None).validate_state(&s, 0),
            Err(ModelError::ParamUnsupported {
                mode: 7,
                param: "brightness"
            })
        );
        assert!(matches!(
            mode(ModeCaps::BRIGHTNESS, ColorMode::None).validate_state(&s, 0),
            Err(ModelError::ParamOutOfRange {
                param: "brightness",
                value: 101,
                ..
            })
        ));
        let ok = ModeState {
            brightness: Some(100),
            ..state()
        };
        assert_eq!(
            mode(ModeCaps::BRIGHTNESS, ColorMode::None).validate_state(&ok, 0),
            Ok(())
        );
    }

    #[test]
    fn direction_must_match_cap() {
        let m = mode(ModeCaps::DIRECTION_LR, ColorMode::None);
        for (d, ok) in [
            (Direction::Left, true),
            (Direction::Right, true),
            (Direction::Up, false),
            (Direction::Down, false),
            (Direction::Horizontal, false),
            (Direction::Vertical, false),
        ] {
            let s = ModeState {
                direction: Some(d),
                ..state()
            };
            assert_eq!(m.validate_state(&s, 0).is_ok(), ok, "{d:?}");
        }
        let hv = mode(ModeCaps::DIRECTION_HV, ColorMode::None);
        let s = ModeState {
            direction: Some(Direction::Vertical),
            ..state()
        };
        assert_eq!(hv.validate_state(&s, 0), Ok(()));
        let s = ModeState {
            direction: Some(Direction::Up),
            ..state()
        };
        assert_eq!(
            hv.validate_state(&s, 0),
            Err(ModelError::DirectionUnsupported {
                mode: 7,
                direction: Direction::Up
            })
        );
        let ud = mode(ModeCaps::DIRECTION_UD, ColorMode::None);
        assert_eq!(ud.validate_state(&s, 0), Ok(()));
    }

    #[test]
    fn per_led_needs_full_buffer() {
        let m = mode(ModeCaps::PER_LED_COLOR, ColorMode::PerLed);
        let s = ModeState {
            colors: vec![Rgb::WHITE; 4],
            ..state()
        };
        assert_eq!(m.validate_state(&s, 4), Ok(()));
        assert_eq!(
            m.validate_state(&s, 5),
            Err(ModelError::ColorCount {
                mode: 7,
                actual: 4,
                min: 5,
                max: 5
            })
        );
    }

    #[test]
    fn mode_specific_count_in_range() {
        let m = mode(ModeCaps::MODE_SPECIFIC_COLOR, ColorMode::ModeSpecific);
        for (n, ok) in [(0, false), (1, true), (2, true), (3, false)] {
            let s = ModeState {
                colors: vec![Rgb::BLACK; n],
                ..state()
            };
            assert_eq!(m.validate_state(&s, 100).is_ok(), ok, "{n} colors");
        }
        let no_range = Mode { colors: None, ..m };
        assert_eq!(
            no_range.validate_state(&state(), 100),
            Err(ModelError::MissingColorRange { mode: 7 })
        );
    }

    #[test]
    fn none_and_random_take_no_colors() {
        let s = ModeState {
            colors: vec![Rgb::BLACK],
            ..state()
        };
        for cm in [ColorMode::None, ColorMode::Random] {
            let m = mode(ModeCaps::RANDOM_COLOR, cm);
            assert_eq!(m.validate_state(&state(), 3), Ok(()));
            assert_eq!(
                m.validate_state(&s, 3),
                Err(ModelError::ColorCount {
                    mode: 7,
                    actual: 1,
                    min: 0,
                    max: 0
                }),
                "{cm:?}"
            );
        }
    }

    #[test]
    fn caps_serialize_as_text() {
        let caps = ModeCaps::SPEED | ModeCaps::BRIGHTNESS;
        let json = serde_json::to_string(&caps).expect("serialize");
        assert_eq!(json, "\"SPEED | BRIGHTNESS\"");
        let back: ModeCaps = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, caps);
    }
}
