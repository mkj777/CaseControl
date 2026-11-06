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
