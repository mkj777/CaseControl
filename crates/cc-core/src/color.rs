//! 24-bit RGB colors with a `"#RRGGBB"` string form.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A 24-bit RGB color.
///
/// Serializes as the upper-case string `"#RRGGBB"`. Parses (and deserializes) from `#RRGGBB`, the short form `#RGB`
/// (each nibble doubled, so `#F80` is `#FF8800`) and `RRGGBB` without the hash; hex digits are case-insensitive.
/// Drivers do their own wire byte order; this type carries no packing helpers beyond [`From<u32>`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb {
    /// Black, `#000000`.
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);
    /// White, `#FFFFFF`.
    pub const WHITE: Rgb = Rgb::new(0xFF, 0xFF, 0xFF);

    /// Creates a color from its three channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// The colour's intensity on a brightness-only LED: its highest channel, so white and every saturated colour are
    /// full intensity and black is off.
    pub const fn intensity(self) -> u8 {
        let rg = if self.r > self.g { self.r } else { self.g };
        if rg > self.b { rg } else { self.b }
    }

    /// The grey that stores a brightness-only LED's level of `percent` (0-100, higher counts as 100) as a colour:
    /// `round(percent * 255 / 100)` on every channel (ADR 0001 #41). Its [`intensity`](Self::intensity) maps back to
    /// the same percent ([`level_percent`](Self::level_percent)), and a driver that scales its brightness by the
    /// intensity (NVIDIA Illumination: `round(intensity * brightness / 255)`) writes exactly `percent` at full
    /// brightness.
    pub const fn white_level(percent: u8) -> Self {
        let p = if percent > 100 { 100 } else { percent as u16 };
        // Fits a u8: at most (100 * 255 + 50) / 100 = 255.
        let v = ((p * 255 + 50) / 100) as u8;
        Self::new(v, v, v)
    }

    /// The level in percent (0-100) a brightness-only LED shows for this colour: its
    /// [`intensity`](Self::intensity) as `round(intensity * 100 / 255)`. Inverse of [`white_level`](Self::white_level).
    pub const fn level_percent(self) -> u8 {
        // Fits a u8: at most (255 * 100 + 127) / 255 = 100.
        ((self.intensity() as u16 * 100 + 127) / 255) as u8
    }
}

/// An sRGB channel (0-255) as linear light (0.0-1.0), with the sRGB transfer function (IEC 61966-2-1).
pub fn srgb_to_linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light (clamped to 0.0-1.0) as an sRGB channel, rounded; the inverse of [`srgb_to_linear`].
pub fn linear_to_srgb(linear: f64) -> u8 {
    let l = if linear.is_nan() {
        0.0
    } else {
        linear.clamp(0.0, 1.0)
    };
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    // In 0..=255 by construction (c in 0..=1); the cast only drops the fraction rounding removed.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = (c * 255.0).round().clamp(0.0, 255.0) as u8;
    v
}

/// The average of `colors` in linear light, converted back to sRGB (ADR 0001 #42: the one colour a zone of mixed
/// colours is saved as). Averaging in linear light keeps the perceived brightness of a mix: red and green average to
/// a bright yellow (`#BCBC00`), not the muddy `#808000` a plain sRGB mean gives. `None` for no colours.
pub fn average_linear(colors: &[Rgb]) -> Option<Rgb> {
    if colors.is_empty() {
        return None;
    }
    let mut sum = [0.0_f64; 3];
    for c in colors {
        sum[0] += srgb_to_linear(c.r);
        sum[1] += srgb_to_linear(c.g);
        sum[2] += srgb_to_linear(c.b);
    }
    // At most a few hundred LEDs per zone: exact in f64.
    #[allow(clippy::cast_precision_loss)]
    let n = colors.len() as f64;
    Some(Rgb::new(
        linear_to_srgb(sum[0] / n),
        linear_to_srgb(sum[1] / n),
        linear_to_srgb(sum[2] / n),
    ))
}

impl From<[u8; 3]> for Rgb {
    fn from([r, g, b]: [u8; 3]) -> Self {
        Self::new(r, g, b)
    }
}

impl From<u32> for Rgb {
    /// Interprets the value as `0x00RRGGBB`; the top byte is ignored.
    fn from(v: u32) -> Self {
        let [_, r, g, b] = v.to_be_bytes();
        Self::new(r, g, b)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// Error returned when a string is not a valid color.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid color {input:?}: expected #RRGGBB, #RGB or RRGGBB")]
pub struct ParseRgbError {
    /// The rejected input.
    pub input: String,
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn parse_digits(digits: &[u8]) -> Option<Rgb> {
    let nibbles: Vec<u8> = digits.iter().map(|&c| hex_nibble(c)).collect::<Option<_>>()?;
    match nibbles.as_slice() {
        [r, g, b] => Some(Rgb::new(r * 0x11, g * 0x11, b * 0x11)),
        [r1, r2, g1, g2, b1, b2] => Some(Rgb::new(r1 << 4 | r2, g1 << 4 | g2, b1 << 4 | b2)),
        _ => None,
    }
}

impl FromStr for Rgb {
    type Err = ParseRgbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        let parsed = match bytes {
            [b'#', rest @ ..] => parse_digits(rest),
            // Without the hash only the long form is accepted, so "abc" is not silently a color.
            _ if bytes.len() == 6 => parse_digits(bytes),
            _ => None,
        };
        parsed.ok_or_else(|| ParseRgbError { input: s.to_owned() })
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_is_the_highest_channel() {
        assert_eq!(Rgb::WHITE.intensity(), 255);
        assert_eq!(Rgb::BLACK.intensity(), 0);
        assert_eq!(Rgb::new(255, 0, 0).intensity(), 255);
        assert_eq!(Rgb::new(1, 9, 3).intensity(), 9);
        assert_eq!(Rgb::new(1, 2, 30).intensity(), 30);
    }

    #[test]
    fn white_level_round_trips_every_percent() {
        assert_eq!(Rgb::white_level(0), Rgb::BLACK);
        assert_eq!(Rgb::white_level(100), Rgb::WHITE);
        // 50 * 2.55 = 127.5 -> 128; 1 * 2.55 = 2.55 -> 3; 33 * 2.55 = 84.15 -> 84.
        assert_eq!(Rgb::white_level(50), Rgb::new(128, 128, 128));
        assert_eq!(Rgb::white_level(1), Rgb::new(3, 3, 3));
        assert_eq!(Rgb::white_level(33), Rgb::new(84, 84, 84));
        assert_eq!(Rgb::white_level(255), Rgb::WHITE, "above 100 counts as 100");
        for p in 0..=100 {
            assert_eq!(Rgb::white_level(p).level_percent(), p, "{p} %");
        }
        // A colour counts as its highest channel.
        assert_eq!(Rgb::new(255, 0, 0).level_percent(), 100);
        assert_eq!(Rgb::new(0, 0, 128).level_percent(), 50);
    }

    #[test]
    fn srgb_linear_round_trips_every_channel() {
        assert_eq!(srgb_to_linear(0), 0.0);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-12);
        // 128 / 255 = 0.50196; ((0.50196 + 0.055) / 1.055)^2.4 = 0.21586.
        assert!((srgb_to_linear(128) - 0.215_86).abs() < 1e-4);
        // The linear segment: 10 / 255 / 12.92 = 0.0030353.
        assert!((srgb_to_linear(10) - 0.003_035_3).abs() < 1e-6);
        for v in 0..=255u8 {
            assert_eq!(linear_to_srgb(srgb_to_linear(v)), v, "{v}");
        }
        assert_eq!(linear_to_srgb(-1.0), 0);
        assert_eq!(linear_to_srgb(2.0), 255);
        assert_eq!(linear_to_srgb(f64::NAN), 0);
    }

    #[test]
    fn average_is_taken_in_linear_light() {
        assert_eq!(average_linear(&[]), None);
        assert_eq!(average_linear(&[Rgb::new(1, 2, 3)]), Some(Rgb::new(1, 2, 3)));
        let same = [Rgb::new(200, 10, 99); 20];
        assert_eq!(average_linear(&same), Some(Rgb::new(200, 10, 99)));
        // Black and white: linear 0.5 -> 1.055 * 0.5^(1/2.4) - 0.055 = 0.73536 -> 187.5 -> 188 (sRGB mean: 128).
        assert_eq!(
            average_linear(&[Rgb::BLACK, Rgb::WHITE]),
            Some(Rgb::new(188, 188, 188))
        );
        // Red and green: each channel averages 1.0 and 0.0 -> 188, blue stays 0.
        assert_eq!(
            average_linear(&[Rgb::new(255, 0, 0), Rgb::new(0, 255, 0)]),
            Some(Rgb::new(188, 188, 0))
        );
        // Three primaries: linear 1/3 -> 0.61306 -> 156.3 -> 156.
        assert_eq!(
            average_linear(&[Rgb::new(255, 0, 0), Rgb::new(0, 255, 0), Rgb::new(0, 0, 255)]),
            Some(Rgb::new(156, 156, 156))
        );
    }

    fn parse(s: &str) -> Rgb {
        s.parse().expect("valid color")
    }

    #[test]
    fn formats_upper_case_with_hash() {
        assert_eq!(Rgb::new(0x0a, 0xbc, 0xff).to_string(), "#0ABCFF");
        assert_eq!(Rgb::BLACK.to_string(), "#000000");
        assert_eq!(Rgb::WHITE.to_string(), "#FFFFFF");
    }

    #[test]
    fn parses_all_accepted_forms() {
        assert_eq!(parse("#0ABCFF"), Rgb::new(0x0a, 0xbc, 0xff));
        assert_eq!(parse("#0abcff"), Rgb::new(0x0a, 0xbc, 0xff));
        assert_eq!(parse("0ABCFF"), Rgb::new(0x0a, 0xbc, 0xff));
        assert_eq!(parse("#F80"), Rgb::new(0xff, 0x88, 0x00));
        assert_eq!(parse("#fff"), Rgb::WHITE);
    }

    #[test]
    fn round_trips_through_string() {
        for c in [
            Rgb::BLACK,
            Rgb::WHITE,
            Rgb::new(1, 2, 3),
            Rgb::new(0xde, 0xad, 0xbe),
        ] {
            assert_eq!(parse(&c.to_string()), c);
        }
        assert_eq!(parse("#abc").to_string(), "#AABBCC");
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "#",
            "abc",
            "#ABCD",
            "#ABCDE",
            "#ABCDEFF",
            "ABCDEFF",
            "#GGGGGG",
            "#+f0000",
            "+f0000",
            "#-10000",
            " #FFFFFF",
            "#FFFFFF ",
            "##FFFFFF",
            "#\u{c4}\u{c4}\u{c4}",
            "\u{c4}\u{c4}\u{c4}",
            "0xFFFFFF",
        ] {
            assert!(bad.parse::<Rgb>().is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn conversions() {
        assert_eq!(Rgb::from([1, 2, 3]), Rgb::new(1, 2, 3));
        assert_eq!(Rgb::from(0x0012_3456_u32), Rgb::new(0x12, 0x34, 0x56));
        assert_eq!(Rgb::from(0xFF12_3456_u32), Rgb::new(0x12, 0x34, 0x56));
    }

    #[test]
    fn serde_uses_string_form() {
        let json = serde_json::to_string(&Rgb::new(0x12, 0xab, 0x00)).expect("serialize");
        assert_eq!(json, "\"#12AB00\"");
        let back: Rgb = serde_json::from_str("\"#1a0\"").expect("deserialize short form");
        assert_eq!(back, Rgb::new(0x11, 0xaa, 0x00));
        assert!(serde_json::from_str::<Rgb>("\"red\"").is_err());
        assert!(serde_json::from_str::<Rgb>("16711680").is_err());
    }
}
