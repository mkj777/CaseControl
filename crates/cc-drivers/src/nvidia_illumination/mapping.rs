// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! Pure colour-to-params mapping, ported from `NVIDIAIlluminationV1Controller::setZone`.

use cc_core::Rgb;
use cc_nvapi::{ZoneColor, ZoneControl, ZoneKind};

/// Brightness the reference's "Direct" mode starts with (`Static.brightness = 100`).
pub const DEFAULT_BRIGHTNESS: u8 = 100;

/// Highest brightness percentage (`Static.brightness_max = 100`).
pub const MAX_BRIGHTNESS: u8 = 100;

/// The manual params the reference writes for `color` at `brightness` in "Direct" mode
/// (`NVIDIA_ILLUMINATION_DIRECT`), or `None` for a zone type the reference has no branch for.
///
/// - RGB: `(r, g, b, brightness)`.
/// - RGBW: unless the card `treats_rgbw_as_rgb`, a colour whose highest and lowest channels differ by 10 or less is
///   sent as white only: `r = g = b = 0`, `w = (max + min) / 2`. Otherwise `(r, g, b, 0, brightness)`.
/// - SINGLE_COLOR and COLOR_FIXED: brightness only, `round(max(r, g, b) * brightness / 255)`, so black is 0, white is
///   the full mode brightness and a dim grey dims the zone.
///
/// Deviation from the reference: upstream sends the mode brightness unchanged for any non-black colour (and 0 for
/// black, `allZero`), so every lit colour looks the same on brightness-only zones. Scaling by the colour's value
/// (owner/orchestrator decision) lets direct-mode colours and profiles dim these zones.
#[must_use]
pub fn direct_color(
    kind: ZoneKind,
    color: Rgb,
    brightness: u8,
    treats_rgbw_as_rgb: bool,
) -> Option<ZoneColor> {
    let Rgb { r, g, b } = color;
    match kind {
        ZoneKind::Rgb => Some(ZoneColor::Rgb { r, g, b, brightness }),
        ZoneKind::Rgbw => {
            let min = r.min(g).min(b);
            let max = r.max(g).max(b);
            if !treats_rgbw_as_rgb && max - min <= 10 {
                // (max + min) / 2 computed in int upstream; u16 avoids the u8 overflow, the result fits a u8.
                let w = u8::try_from((u16::from(max) + u16::from(min)) / 2).unwrap_or(u8::MAX);
                Some(ZoneColor::Rgbw {
                    r: 0,
                    g: 0,
                    b: 0,
                    w,
                    brightness,
                })
            } else {
                Some(ZoneColor::Rgbw {
                    r,
                    g,
                    b,
                    w: 0,
                    brightness,
                })
            }
        }
        ZoneKind::SingleColor | ZoneKind::ColorFixed => Some(ZoneColor::Single {
            brightness: scaled_brightness(color, brightness),
        }),
        ZoneKind::Invalid | ZoneKind::Other(_) => None,
    }
}

/// `round(max(r, g, b) * brightness / 255)`, rounding halves up. At most `brightness`.
#[must_use]
pub fn scaled_brightness(color: Rgb, brightness: u8) -> u8 {
    let value = u32::from(color.r.max(color.g).max(color.b));
    let scaled = (value * u32::from(brightness) * 2 + 255) / 510;
    u8::try_from(scaled).unwrap_or(brightness)
}

/// The manual params the reference writes in "Off" mode (`NVIDIA_ILLUMINATION_OFF`): every channel and the
/// brightness zero. `None` for a zone type the reference has no branch for.
#[must_use]
pub fn off_color(kind: ZoneKind) -> Option<ZoneColor> {
    match kind {
        ZoneKind::Rgb => Some(ZoneColor::Rgb {
            r: 0,
            g: 0,
            b: 0,
            brightness: 0,
        }),
        ZoneKind::Rgbw => Some(ZoneColor::Rgbw {
            r: 0,
            g: 0,
            b: 0,
            w: 0,
            brightness: 0,
        }),
        ZoneKind::SingleColor | ZoneKind::ColorFixed => Some(ZoneColor::Single { brightness: 0 }),
        ZoneKind::Invalid | ZoneKind::Other(_) => None,
    }
}

/// Writes `color` into `zone` as a manual (static) colour, the way upstream sets
/// `ctrlMode = NV_GPU_CLIENT_ILLUM_CTRL_MODE_MANUAL_RGB` before the per-type params.
pub fn apply(zone: &mut ZoneControl, color: ZoneColor) {
    zone.mode = cc_nvapi::CtrlMode::Manual;
    zone.manual = color;
}

/// The colour and brightness a zone currently shows, for display (the inverse of [`direct_color`] where one exists).
///
/// RGB zones give their colour; RGBW zones give their colour, or grey `(w, w, w)` when only white is lit; brightness
/// only zones give white when lit and black when not. `None` when the zone is not in manual mode or of an unknown
/// type. (Upstream `getZoneColor` reads the RGB union member for every zone type, which is garbage for non-RGB
/// zones; this is the type-correct version.)
#[must_use]
pub fn current_color(zone: &ZoneControl) -> Option<(Rgb, u8)> {
    match zone.manual {
        ZoneColor::Rgb { r, g, b, brightness } => Some((Rgb::new(r, g, b), brightness)),
        ZoneColor::Rgbw {
            r,
            g,
            b,
            w,
            brightness,
        } => {
            if (r, g, b) == (0, 0, 0) && w > 0 {
                Some((Rgb::new(w, w, w), brightness))
            } else {
                Some((Rgb::new(r, g, b), brightness))
            }
        }
        ZoneColor::Single { brightness } => {
            let color = if brightness == 0 { Rgb::BLACK } else { Rgb::WHITE };
            Some((color, brightness))
        }
        ZoneColor::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgb = Rgb::new(255, 0, 0);

    #[test]
    fn rgb_zone_takes_colour_and_brightness() {
        assert_eq!(
            direct_color(ZoneKind::Rgb, Rgb::new(1, 2, 3), 77, false),
            Some(ZoneColor::Rgb {
                r: 1,
                g: 2,
                b: 3,
                brightness: 77
            })
        );
        // Black stays black with the brightness kept (upstream only zeroes brightness for brightness-only zones).
        assert_eq!(
            direct_color(ZoneKind::Rgb, Rgb::BLACK, 100, false),
            Some(ZoneColor::Rgb {
                r: 0,
                g: 0,
                b: 0,
                brightness: 100
            })
        );
    }

    #[test]
    fn rgbw_zone_saturated_colour_has_no_white() {
        assert_eq!(
            direct_color(ZoneKind::Rgbw, RED, 36, false),
            Some(ZoneColor::Rgbw {
                r: 255,
                g: 0,
                b: 0,
                w: 0,
                brightness: 36
            })
        );
        // max - min = 11 > 10: still RGB.
        assert_eq!(
            direct_color(ZoneKind::Rgbw, Rgb::new(100, 111, 105), 50, false),
            Some(ZoneColor::Rgbw {
                r: 100,
                g: 111,
                b: 105,
                w: 0,
                brightness: 50
            })
        );
    }

    #[test]
    fn rgbw_zone_near_grey_becomes_white_only() {
        // max 255, min 245, diff 10 <= 10: w = (255 + 245) / 2 = 250, which overflows u8 if added in u8.
        assert_eq!(
            direct_color(ZoneKind::Rgbw, Rgb::new(255, 245, 250), 100, false),
            Some(ZoneColor::Rgbw {
                r: 0,
                g: 0,
                b: 0,
                w: 250,
                brightness: 100
            })
        );
        // (101 + 100) / 2 = 100 (integer division).
        assert_eq!(
            direct_color(ZoneKind::Rgbw, Rgb::new(100, 101, 100), 9, false),
            Some(ZoneColor::Rgbw {
                r: 0,
                g: 0,
                b: 0,
                w: 100,
                brightness: 9
            })
        );
        // Black: diff 0 -> white 0, all zero.
        assert_eq!(
            direct_color(ZoneKind::Rgbw, Rgb::BLACK, 100, false),
            Some(ZoneColor::Rgbw {
                r: 0,
                g: 0,
                b: 0,
                w: 0,
                brightness: 100
            })
        );
    }

    #[test]
    fn rgbw_zone_on_card_that_ignores_white() {
        assert_eq!(
            direct_color(ZoneKind::Rgbw, Rgb::WHITE, 100, true),
            Some(ZoneColor::Rgbw {
                r: 255,
                g: 255,
                b: 255,
                w: 0,
                brightness: 100
            })
        );
    }

    #[test]
    fn brightness_only_zones() {
        for kind in [ZoneKind::SingleColor, ZoneKind::ColorFixed] {
            // 1 * 60 / 255 = 0.24 -> 0; 3 * 60 / 255 = 0.71 -> 1.
            assert_eq!(
                direct_color(kind, Rgb::new(0, 0, 1), 60, false),
                Some(ZoneColor::Single { brightness: 0 })
            );
            assert_eq!(
                direct_color(kind, Rgb::new(0, 3, 1), 60, false),
                Some(ZoneColor::Single { brightness: 1 })
            );
            // 50 % grey: 128 * 100 / 255 = 50.2 -> 50; 10 % grey: 26 * 100 / 255 = 10.2 -> 10.
            assert_eq!(
                direct_color(kind, Rgb::new(128, 128, 128), 100, false),
                Some(ZoneColor::Single { brightness: 50 })
            );
            assert_eq!(
                direct_color(kind, Rgb::new(26, 26, 26), 100, false),
                Some(ZoneColor::Single { brightness: 10 })
            );
            // Value is the max channel: 51 * 100 / 255 = 20 exactly.
            assert_eq!(
                direct_color(kind, Rgb::new(51, 0, 10), 100, false),
                Some(ZoneColor::Single { brightness: 20 })
            );
            assert_eq!(
                direct_color(kind, Rgb::WHITE, 100, false),
                Some(ZoneColor::Single { brightness: 100 })
            );
            assert_eq!(
                direct_color(kind, RED, 60, false),
                Some(ZoneColor::Single { brightness: 60 })
            );
            assert_eq!(
                direct_color(kind, Rgb::BLACK, 60, false),
                Some(ZoneColor::Single { brightness: 0 })
            );
        }
    }

    #[test]
    fn unknown_zone_types_are_left_alone() {
        for kind in [ZoneKind::Invalid, ZoneKind::Other(9)] {
            assert_eq!(direct_color(kind, RED, 100, false), None);
            assert_eq!(off_color(kind), None);
        }
    }

    #[test]
    fn off_zeroes_everything() {
        assert_eq!(
            off_color(ZoneKind::Rgb),
            Some(ZoneColor::Rgb {
                r: 0,
                g: 0,
                b: 0,
                brightness: 0
            })
        );
        assert_eq!(
            off_color(ZoneKind::Rgbw),
            Some(ZoneColor::Rgbw {
                r: 0,
                g: 0,
                b: 0,
                w: 0,
                brightness: 0
            })
        );
        assert_eq!(
            off_color(ZoneKind::SingleColor),
            Some(ZoneColor::Single { brightness: 0 })
        );
        assert_eq!(
            off_color(ZoneKind::ColorFixed),
            Some(ZoneColor::Single { brightness: 0 })
        );
    }

    /// Current params -> displayed colour -> params gives the same params for every value the mapping can produce.
    #[test]
    fn current_params_round_trip() {
        let cases = [
            (
                ZoneKind::Rgb,
                ZoneColor::Rgb {
                    r: 12,
                    g: 200,
                    b: 7,
                    brightness: 42,
                },
            ),
            (
                ZoneKind::Rgbw,
                ZoneColor::Rgbw {
                    r: 255,
                    g: 0,
                    b: 0,
                    w: 0,
                    brightness: 36,
                },
            ),
            (
                ZoneKind::Rgbw,
                ZoneColor::Rgbw {
                    r: 0,
                    g: 0,
                    b: 0,
                    w: 180,
                    brightness: 70,
                },
            ),
            (ZoneKind::SingleColor, ZoneColor::Single { brightness: 50 }),
            (ZoneKind::SingleColor, ZoneColor::Single { brightness: 0 }),
            (ZoneKind::ColorFixed, ZoneColor::Single { brightness: 60 }),
        ];
        for (kind, manual) in cases {
            let zone = ZoneControl::new(kind, manual);
            let (color, brightness) = current_color(&zone).expect("manual zone decodes");
            assert_eq!(
                direct_color(kind, color, brightness, false),
                Some(manual),
                "{kind:?} {manual:?}"
            );
            let mut rebuilt = ZoneControl::new(kind, ZoneColor::Other);
            apply(
                &mut rebuilt,
                direct_color(kind, color, brightness, false).expect("known kind"),
            );
            assert_eq!(rebuilt, zone);
        }
        let mut piecewise = ZoneControl::new(ZoneKind::Rgb, ZoneColor::Other);
        piecewise.mode = cc_nvapi::CtrlMode::PiecewiseLinear;
        assert_eq!(current_color(&piecewise), None);
    }
}
