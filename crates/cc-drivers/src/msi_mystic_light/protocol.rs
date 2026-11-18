// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! Byte layouts and pure packet builders of the 185-byte Mystic Light protocol.
//!
//! Ported from `reference/openrgb/MSIMotherboardController/MSIMotherboardCommon.h` (struct layouts) and
//! `MSIMotherboard185Controller.cpp` (`SetMode`, `SetZoneColor`, `SelectPerLedProtocol`, `ReadFwVersion`). Every
//! function here is pure; the controller owns all I/O.

use std::fmt;

use cc_core::Rgb;

/// MSI's USB vendor id.
pub const VID_MSI: u16 = 0x1462;
/// Vendor id of the "common PID" boards.
pub const VID_MSI_COMMON: u16 = 0x0DB0;
/// Product id shared by a set of 185-byte boards; the real board id is the first four hex digits of the serial.
pub const PID_COMMON: u16 = 0x0076;

/// Report id of the configuration feature report (`FeaturePacket_185`, `FeaturePacket_162`, `FeaturePacket_112`).
pub const CONFIG_REPORT_ID: u8 = 0x52;
/// Report id of the per-LED feature report (`FeaturePacket_PerLED_185`).
pub const PER_LED_REPORT_ID: u8 = 0x53;
/// `sizeof(FeaturePacket_185)`.
pub const CONFIG_LEN: usize = 185;
/// `sizeof(FeaturePacket_162)`.
pub const CONFIG_LEN_162: usize = 162;
/// `sizeof(FeaturePacket_112)`.
pub const CONFIG_LEN_112: usize = 112;
/// Index of `save_data` in `FeaturePacket_185`. Non-zero tells the firmware to write the packet to flash.
pub const SAVE_BYTE: usize = 184;
/// Buffer size the reference detector passes to `hid_get_feature_report(0x52)`.
pub const DETECT_BUFFER_LEN: usize = 200;
/// `NUMOF_PER_LED_MODE_LEDS`.
pub const PER_LED_LEDS: usize = 240;
/// Header bytes of `FeaturePacket_PerLED_185` (report id plus four header bytes).
pub const PER_LED_HEADER: usize = 5;
/// `sizeof(FeaturePacket_PerLED_185)`: 5 header bytes plus 240 RGB triples.
pub const PER_LED_LEN: usize = PER_LED_HEADER + 3 * PER_LED_LEDS;
/// Length of the firmware version request and response (`ReadFwVersion`).
pub const FW_REPORT_LEN: usize = 64;
/// Firmware request command for the APROM (application) version.
pub const FW_CMD_APROM: u8 = 0xB0;
/// Firmware request command for the LDROM (boot loader) version.
pub const FW_CMD_LDROM: u8 = 0xB6;

/// LED count of each JRAINBOW header in the synchronized per-LED mode (`SYNC_PER_LED_MODE_JRAINBOW_LED_COUNT`).
pub const SYNC_JRAINBOW_LEDS: usize = 40;
/// LED count of JCORSAIR/JRAINBOW3 in the synchronized per-LED mode (`SYNC_PER_LED_MODE_CORSAIR_LED_COUNT`).
pub const SYNC_CORSAIR_LEDS: usize = 120;
/// `JRAINBOW1_MAX_LED_COUNT`.
pub const JRAINBOW1_MAX_LEDS: usize = 200;
/// `JRAINBOW2_MAX_LED_COUNT` (also used for JRAINBOW3).
pub const JRAINBOW2_MAX_LEDS: usize = 240;
/// `JCORSAIR_MAX_LED_COUNT`.
pub const JCORSAIR_MAX_LEDS: usize = 240;

/// Mode (effect) values, `enum MSI_MODE` of the reference.
pub mod mode {
    /// `MSI_MODE_DISABLE`.
    pub const DISABLE: u8 = 0;
    /// `MSI_MODE_STATIC`.
    pub const STATIC: u8 = 1;
    /// `MSI_MODE_BREATHING`.
    pub const BREATHING: u8 = 2;
    /// `MSI_MODE_FLASHING`.
    pub const FLASHING: u8 = 3;
    /// `MSI_MODE_DOUBLE_FLASHING`.
    pub const DOUBLE_FLASHING: u8 = 4;
    /// `MSI_MODE_LIGHTNING`; modes above it make the on-board zone sync JRGB and JPIPE.
    pub const LIGHTNING: u8 = 5;
    /// `MSI_MODE_METEOR`.
    pub const METEOR: u8 = 7;
    /// `MSI_MODE_COLOR_RING`.
    pub const COLOR_RING: u8 = 15;
    /// `MSI_MODE_PLANETARY`.
    pub const PLANETARY: u8 = 16;
    /// `MSI_MODE_DOUBLE_METEOR`.
    pub const DOUBLE_METEOR: u8 = 17;
    /// `MSI_MODE_ENERGY`.
    pub const ENERGY: u8 = 18;
    /// `MSI_MODE_BLINK`.
    pub const BLINK: u8 = 19;
    /// `MSI_MODE_CLOCK`.
    pub const CLOCK: u8 = 20;
    /// `MSI_MODE_COLOR_PULSE`.
    pub const COLOR_PULSE: u8 = 21;
    /// `MSI_MODE_COLOR_SHIFT`.
    pub const COLOR_SHIFT: u8 = 22;
    /// `MSI_MODE_COLOR_WAVE`.
    pub const COLOR_WAVE: u8 = 23;
    /// `MSI_MODE_MARQUEE`.
    pub const MARQUEE: u8 = 24;
    /// `MSI_MODE_RAINBOW_WAVE`.
    pub const RAINBOW_WAVE: u8 = 26;
    /// `MSI_MODE_VISOR`.
    pub const VISOR: u8 = 27;
    /// `MSI_MODE_RAINBOW_FLASHING`.
    pub const RAINBOW_FLASHING: u8 = 29;
    /// `MSI_MODE_COLOR_RING_DOUBLE_FLASHING`.
    pub const COLOR_RING_DOUBLE_FLASHING: u8 = 35;
    /// `MSI_MODE_STACK`.
    pub const STACK: u8 = 36;
    /// `MSI_MODE_FIRE`.
    pub const FIRE: u8 = 38;
    /// Effect byte the firmware uses for per-LED (direct) control (`MSI_DIRECT_MODE`).
    pub const DIRECT_EFFECT: u8 = 0x25;
    /// Driver-side id of the direct mode (`MSI_MODE_DIRECT_DUMMY`); never sent to the device.
    pub const DIRECT_DUMMY: u8 = 100;
}

/// Speed values (`enum MSI_SPEED`).
pub mod speed {
    /// `MSI_SPEED_LOW`.
    pub const LOW: u8 = 0;
    /// `MSI_SPEED_MEDIUM`, the reference's default.
    pub const MEDIUM: u8 = 1;
    /// `MSI_SPEED_HIGH`.
    pub const HIGH: u8 = 2;
}

/// Brightness values (`enum MSI_BRIGHTNESS`): 0 = off, 1..=10 = 10 %..100 %.
pub mod brightness {
    /// `MSI_BRIGHTNESS_OFF`.
    pub const OFF: u8 = 0;
    /// `MSI_BRIGHTNESS_LEVEL_100`, the reference's default.
    pub const LEVEL_100: u8 = 10;
}

// Field offsets inside `ZoneData` / `RainbowZoneData` (10 / 11 bytes).
const EFFECT: usize = 0;
const COLOR: usize = 1;
const SPEED_BRIGHTNESS: usize = 4;
const COLOR2: usize = 5;
const COLOR_FLAGS: usize = 8;
const PADDING: usize = 9;
const LED_COUNT: usize = 10;

// `CorsairZoneData` offsets (11 bytes) used by the enable-direct message.
const CORSAIR_FAN_FLAGS: usize = 4;
const CORSAIR_QUANTITY: usize = 5;
const CORSAIR_PADDING2: usize = 8;
const CORSAIR_IS_INDIVIDUAL: usize = 10;

// Struct offsets inside `FeaturePacket_185` (the `// n` comments of the reference).
const OFF_J_RGB_1: usize = 1;
const OFF_J_PIPE_1: usize = 11;
const OFF_J_PIPE_2: usize = 21;
const OFF_J_RAINBOW_1: usize = 31;
const OFF_J_RAINBOW_2: usize = 42;
const OFF_J_CORSAIR: usize = 53;
const OFF_J_CORSAIR_OUTERLL120: usize = 64;
const OFF_ON_BOARD: [usize; 10] = [74, 84, 94, 104, 114, 124, 134, 144, 154, 164];
const OFF_J_RGB_2: usize = 174;
/// Start of every zone record (each begins with its effect byte).
const ZONE_OFFSETS: [usize; 18] = [
    OFF_J_RGB_1,
    OFF_J_PIPE_1,
    OFF_J_PIPE_2,
    OFF_J_RAINBOW_1,
    OFF_J_RAINBOW_2,
    OFF_J_CORSAIR,
    OFF_J_CORSAIR_OUTERLL120,
    OFF_ON_BOARD[0],
    OFF_ON_BOARD[1],
    OFF_ON_BOARD[2],
    OFF_ON_BOARD[3],
    OFF_ON_BOARD[4],
    OFF_ON_BOARD[5],
    OFF_ON_BOARD[6],
    OFF_ON_BOARD[7],
    OFF_ON_BOARD[8],
    OFF_ON_BOARD[9],
    OFF_J_RGB_2,
];

// Flag bits (`MSI_SPEED_BRIGHTNESS_FLAG_*`, `MSI_COLOR_FLAG_*`).
const SPEED_MASK: u8 = 0x03;
const BRIGHTNESS_SHIFT: u8 = 2;
const BRIGHTNESS_MASK: u8 = 0x1F << BRIGHTNESS_SHIFT;
const SB_SYNC_JRGB: u8 = 1 << 7;
const CF_SYNC_ONBOARD: u8 = 1 << 0;
const CF_SYNC_JRAINBOW1: u8 = 1 << 1;
const CF_SYNC_JRAINBOW2: u8 = 1 << 2;
const CF_SYNC_JCORSAIR: u8 = 1 << 3;
const CF_SYNC_JPIPE1: u8 = 1 << 4;
const CF_SYNC_JPIPE2: u8 = 1 << 5;
const CF_USE_CUSTOM_COLOR: u8 = 1 << 7;
const PER_LED_BASIC_SYNC_MODE: u8 = CF_USE_CUSTOM_COLOR | CF_SYNC_ONBOARD | CF_SYNC_JPIPE1 | CF_SYNC_JPIPE2;
const PER_LED_FULL_SYNC_MODE: u8 =
    PER_LED_BASIC_SYNC_MODE | CF_SYNC_JRAINBOW1 | CF_SYNC_JRAINBOW2 | CF_SYNC_JCORSAIR;

/// Which reference controller class a `get_feature_report(0x52)` answer selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PacketClass {
    /// `MSIMotherboard185Controller` (ported).
    Bytes185,
    /// `MSIMotherboard162Controller` (not ported).
    Bytes162,
    /// `MSIMotherboard112Controller` (not ported).
    Bytes112,
    /// Anything else; the reference would try the 761-byte path, which sends a 761-byte feature report during
    /// detection. Not ported.
    Other,
}

/// Classifies the length returned by `get_feature_report(0x52)` exactly as the reference detector does: a class
/// matches if the length equals the struct size or the struct size plus one.
pub fn classify(len: usize) -> PacketClass {
    let fits = |size: usize| len >= size && len <= size + 1;
    if fits(CONFIG_LEN) {
        PacketClass::Bytes185
    } else if fits(CONFIG_LEN_162) {
        PacketClass::Bytes162
    } else if fits(CONFIG_LEN_112) {
        PacketClass::Bytes112
    } else {
        PacketClass::Other
    }
}

/// A zone of the 185-byte protocol (`enum MSI_ZONE`, the subset the 185-byte class handles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MsiZone {
    /// `MSI_ZONE_J_RGB_1`.
    JRgb1,
    /// `MSI_ZONE_J_RGB_2`.
    JRgb2,
    /// `MSI_ZONE_J_PIPE_1`.
    JPipe1,
    /// `MSI_ZONE_J_PIPE_2`.
    JPipe2,
    /// `MSI_ZONE_J_RAINBOW_1`.
    JRainbow1,
    /// `MSI_ZONE_J_RAINBOW_2`.
    JRainbow2,
    /// `MSI_ZONE_J_RAINBOW_3`.
    JRainbow3,
    /// `MSI_ZONE_J_CORSAIR`.
    JCorsair,
    /// `MSI_ZONE_J_CORSAIR_OUTERLL120`.
    JCorsairOuterLl120,
    /// `MSI_ZONE_ON_BOARD_LED_<n>`, n in 0..=10.
    OnBoard(u8),
}

impl MsiZone {
    /// Display name used by the reference's `led_zones` table, or `None` for zones it never exposes.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::JRgb1 => "JRGB1",
            Self::JRgb2 => "JRGB2",
            Self::JRainbow1 => "JRAINBOW1",
            Self::JRainbow2 => "JRAINBOW2",
            Self::JRainbow3 => "JRAINBOW3",
            Self::JCorsair => "JCORSAIR",
            Self::JPipe1 => "PIPE1",
            Self::JPipe2 => "PIPE2",
            Self::OnBoard(0) => "ONBOARD",
            _ => return None,
        })
    }

    /// Whether the reference treats the zone as an addressable (resizable) header.
    pub fn is_addressable(self) -> bool {
        matches!(
            self,
            Self::JRainbow1 | Self::JRainbow2 | Self::JRainbow3 | Self::JCorsair
        )
    }

    fn is_jrgb_or_jpipe(self) -> bool {
        matches!(self, Self::JRgb1 | Self::JRgb2 | Self::JPipe1 | Self::JPipe2)
    }
}

/// Offset of a zone's `ZoneData` inside `FeaturePacket_185` (`GetZoneData`).
///
/// Boards with "mixed mode" support (board id `>= 0x7D03`) keep JRGB2 in the `on_board_led_6` slot and on-board
/// LED 6 in the `j_corsair_outerll120` slot.
pub fn zone_offset(zone: MsiZone, mixed_mode: bool) -> Option<usize> {
    Some(match zone {
        MsiZone::JRgb1 => OFF_J_RGB_1,
        MsiZone::JRgb2 if mixed_mode => OFF_ON_BOARD[6],
        MsiZone::JRgb2 => OFF_J_RGB_2,
        MsiZone::JRainbow1 => OFF_J_RAINBOW_1,
        MsiZone::JRainbow2 => OFF_J_RAINBOW_2,
        MsiZone::JRainbow3 | MsiZone::JCorsair => OFF_J_CORSAIR,
        MsiZone::JPipe1 => OFF_J_PIPE_1,
        MsiZone::JPipe2 => OFF_J_PIPE_2,
        MsiZone::OnBoard(6) if mixed_mode => OFF_J_CORSAIR_OUTERLL120,
        MsiZone::OnBoard(n) if n <= 9 => OFF_ON_BOARD[usize::from(n)],
        MsiZone::OnBoard(_) => return None,
        MsiZone::JCorsairOuterLl120 => OFF_J_CORSAIR_OUTERLL120,
    })
}

/// Offset of the `cycle_or_led_num` byte of a JRAINBOW zone (`GetRainbowZoneData`). JRAINBOW3 reads the
/// `j_corsair` slot as `RainbowZoneData`, so its count is the `is_individual` byte.
pub fn led_count_offset(zone: MsiZone) -> Option<usize> {
    match zone {
        MsiZone::JRainbow1 => Some(OFF_J_RAINBOW_1 + LED_COUNT),
        MsiZone::JRainbow2 => Some(OFF_J_RAINBOW_2 + LED_COUNT),
        MsiZone::JRainbow3 => Some(OFF_J_CORSAIR + LED_COUNT),
        _ => None,
    }
}

/// `speedAndBrightnessFlags` for a speed and brightness, masked as in `SetMode`.
pub fn speed_brightness_flags(speed: u8, brightness: u8) -> u8 {
    ((brightness << BRIGHTNESS_SHIFT) & BRIGHTNESS_MASK) | (speed & SPEED_MASK)
}

fn custom_color_flags(flags: u8, random: bool) -> u8 {
    if random {
        flags & !CF_USE_CUSTOM_COLOR
    } else {
        flags | CF_USE_CUSTOM_COLOR
    }
}

/// The board facts `SetMode` and `SetZoneColor` depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Board id `>= 0x7D03` (`mixed_mode_support`).
    pub mixed_mode: bool,
    /// The board exposes no ONBOARD zone (`no_onboards`).
    pub no_onboards: bool,
    /// `numof_onboard_leds`.
    pub onboard_leds: u8,
}

/// The decoded state of one zone in a configuration packet (`GetMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneState {
    /// Effect byte (`MSI_MODE`).
    pub effect: u8,
    /// Speed bits.
    pub speed: u8,
    /// Brightness bits.
    pub brightness: u8,
    /// `MSI_COLOR_FLAG_USE_CUSTOM_COLOR` is set.
    pub custom_color: bool,
    /// First color.
    pub color: Rgb,
    /// Second color.
    pub color2: Rgb,
}

/// One `FeaturePacket_185` (report id 0x52), byte 0 is the report id.
#[derive(Clone, PartialEq, Eq)]
pub struct ConfigPacket(pub [u8; CONFIG_LEN]);

impl fmt::Debug for ConfigPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ConfigPacket[{}]", cc_hid::hex(&self.0))
    }
}

impl ConfigPacket {
    /// Wraps a report read from the device. `None` unless it is exactly [`CONFIG_LEN`] bytes with report id 0x52.
    pub fn from_report(bytes: &[u8]) -> Option<Self> {
        let arr: [u8; CONFIG_LEN] = bytes.try_into().ok()?;
        (arr[0] == CONFIG_REPORT_ID).then_some(Self(arr))
    }

    /// The bytes to send, with `save_data` set to `save` (1 or 0). Nothing else changes.
    pub fn with_save(&self, save: bool) -> [u8; CONFIG_LEN] {
        let mut out = self.0;
        out[SAVE_BYTE] = u8::from(save);
        out
    }

    /// Whether any zone record of the packet carries the per-LED effect (0x25), i.e. the packet would put (part of)
    /// the board into per-LED mode, or the board was left in it. The enable message sets it on the on-board zone
    /// and, in the unsynchronized protocol, on the ARGB headers. This is the one "per-LED" check of the driver (the
    /// reference's `GetMode` looks at the on-board zone only).
    pub fn has_direct_effect(&self) -> bool {
        ZONE_OFFSETS
            .iter()
            .any(|&o| self.0[o + EFFECT] == mode::DIRECT_EFFECT)
    }

    /// A remembered "last good own lighting" (ADR 0001 #38) as a packet that is safe to send, or `None`: exactly
    /// [`CONFIG_LEN`] bytes (the 185-byte class), report id 0x52, save byte 0 and no zone in the per-LED effect.
    pub fn from_own_lighting(bytes: &[u8]) -> Option<Self> {
        let packet = Self::from_report(bytes)?;
        (packet.0[SAVE_BYTE] == 0 && !packet.has_direct_effect()).then_some(packet)
    }

    /// The bytes to remember as the board's last good own lighting: the packet with the save byte forced to 0, or
    /// `None` when [`from_own_lighting`](Self::from_own_lighting) would not accept it.
    pub fn own_lighting(&self) -> Option<Vec<u8>> {
        let bytes = self.with_save(false);
        Self::from_own_lighting(&bytes).map(|_| bytes.to_vec())
    }

    /// `cycle_or_led_num` of a JRAINBOW zone.
    pub fn led_count(&self, zone: MsiZone) -> Option<u8> {
        led_count_offset(zone).map(|o| self.0[o])
    }

    /// Sets `cycle_or_led_num` of a JRAINBOW zone (`SetZoneLEDCount`); other zones are ignored.
    pub fn set_led_count(&mut self, zone: MsiZone, count: u8) {
        if let Some(o) = led_count_offset(zone) {
            self.0[o] = count;
        }
    }

    /// Decodes a zone (`GetMode`).
    pub fn zone_state(&self, zone: MsiZone, mixed_mode: bool) -> Option<ZoneState> {
        let o = zone_offset(zone, mixed_mode)?;
        let b = &self.0;
        Some(ZoneState {
            effect: b[o + EFFECT],
            speed: b[o + SPEED_BRIGHTNESS] & SPEED_MASK,
            brightness: (b[o + SPEED_BRIGHTNESS] & BRIGHTNESS_MASK) >> BRIGHTNESS_SHIFT,
            custom_color: b[o + COLOR_FLAGS] & CF_USE_CUSTOM_COLOR != 0,
            color: Rgb::new(b[o + COLOR], b[o + COLOR + 1], b[o + COLOR + 2]),
            color2: Rgb::new(b[o + COLOR2], b[o + COLOR2 + 1], b[o + COLOR2 + 2]),
        })
    }

    /// `MSIMotherboard185Controller::SetMode` for one zone (the zone-based direct-mode guard is not ported because
    /// zone-based boards are not supported).
    pub fn apply_mode(
        &mut self,
        layout: &Layout,
        zone: MsiZone,
        effect: u8,
        speed: u8,
        brightness: u8,
        random: bool,
    ) {
        let Some(off) = zone_offset(zone, layout.mixed_mode) else {
            return;
        };
        let flags = speed_brightness_flags(speed, brightness);
        let ob = OFF_ON_BOARD[0];
        let b = &mut self.0;

        b[off + EFFECT] = effect;
        b[off + SPEED_BRIGHTNESS] = flags;
        b[off + COLOR_FLAGS] = custom_color_flags(b[off + COLOR_FLAGS], random);
        // "J_RAINBOW_3 zone needs padding of 4".
        b[off + PADDING] = if zone == MsiZone::JRainbow3 { 4 } else { 0 };

        // Boards without on-board LEDs mirror JRGB/JPIPE into the on-board zone. The reference also re-applies the
        // zone's flags here; both assignments are idempotent, so they are not repeated.
        if layout.no_onboards && zone.is_jrgb_or_jpipe() {
            b[ob + EFFECT] = b[off + EFFECT];
            b[ob + COLOR_FLAGS] |= CF_SYNC_ONBOARD;
            b[ob + PADDING] = 0;
        }

        if effect > mode::LIGHTNING {
            b[ob + SPEED_BRIGHTNESS] |= SB_SYNC_JRGB;
            b[ob + COLOR_FLAGS] |= CF_SYNC_JPIPE1 | CF_SYNC_JPIPE2;
        } else {
            b[ob + SPEED_BRIGHTNESS] &= !SB_SYNC_JRGB;
            b[ob + COLOR_FLAGS] &= !(CF_SYNC_JPIPE1 | CF_SYNC_JPIPE2);
        }

        if zone == MsiZone::OnBoard(0) && effect <= mode::LIGHTNING {
            for i in 0..layout.onboard_leds {
                if let Some(o) = zone_offset(MsiZone::OnBoard(1 + i), layout.mixed_mode) {
                    b[o + EFFECT] = effect;
                    b[o + SPEED_BRIGHTNESS] = flags;
                    b[o + COLOR_FLAGS] = custom_color_flags(b[o + COLOR_FLAGS], random);
                    b[o + PADDING] = 0;
                }
            }
        }
    }

    /// `MSIMotherboard185Controller::SetZoneColor` for one zone.
    pub fn apply_zone_color(&mut self, layout: &Layout, zone: MsiZone, color: Rgb, color2: Rgb) {
        let Some(off) = zone_offset(zone, layout.mixed_mode) else {
            return;
        };
        self.put_colors(off, color, color2);
        if layout.no_onboards && zone.is_jrgb_or_jpipe() {
            self.put_colors(OFF_ON_BOARD[0], color, color2);
        }
        if zone == MsiZone::OnBoard(0) {
            for i in 0..layout.onboard_leds {
                if let Some(o) = zone_offset(MsiZone::OnBoard(1 + i), layout.mixed_mode) {
                    self.put_colors(o, color, color2);
                }
            }
        }
    }

    fn put_colors(&mut self, off: usize, c1: Rgb, c2: Rgb) {
        self.0[off + COLOR..off + COLOR + 3].copy_from_slice(&[c1.r, c1.g, c1.b]);
        self.0[off + COLOR2..off + COLOR2 + 3].copy_from_slice(&[c2.r, c2.g, c2.b]);
    }
}

/// Which addressable headers a board exposes (`no_jrainbow1`, `no_jrainbow2`, `no_jcorsair` inverted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Headers {
    /// JRAINBOW1 is a supported zone.
    pub jrainbow1: bool,
    /// JRAINBOW2 is a supported zone.
    pub jrainbow2: bool,
    /// JCORSAIR or JRAINBOW3 is a supported zone.
    pub jcorsair: bool,
}

/// Whether the synchronized per-LED protocol applies, from the current header LED counts
/// (`SelectPerLedProtocol`): it does unless JRAINBOW1/2 exceed 40 LEDs or JRAINBOW3 exceeds 120.
pub fn sync_direct_mode(config: &ConfigPacket) -> bool {
    let n = |z| usize::from(config.led_count(z).unwrap_or(0));
    !(n(MsiZone::JRainbow1) > SYNC_JRAINBOW_LEDS
        || n(MsiZone::JRainbow2) > SYNC_JRAINBOW_LEDS
        || n(MsiZone::JRainbow3) > SYNC_CORSAIR_LEDS)
}

/// The message that switches the board into per-LED mode (`enable_per_led_msg` after the controller's
/// constructor and `SelectPerLedProtocol`).
pub fn enable_direct_packet(sync: bool, headers: Headers) -> [u8; CONFIG_LEN] {
    // Default member initialisers of FeaturePacket_185: report id 0x52, every zone effect MSI_MODE_STATIC,
    // RainbowZoneData::cycle_or_led_num = 100, CorsairZoneData::fan_flags = 40; everything else zero.
    let mut b = [0u8; CONFIG_LEN];
    b[0] = CONFIG_REPORT_ID;
    for off in [
        OFF_J_RGB_1,
        OFF_J_PIPE_1,
        OFF_J_PIPE_2,
        OFF_J_RAINBOW_1,
        OFF_J_RAINBOW_2,
        OFF_J_CORSAIR,
    ]
    .into_iter()
    .chain([OFF_J_CORSAIR_OUTERLL120])
    .chain(OFF_ON_BOARD)
    .chain([OFF_J_RGB_2])
    {
        b[off + EFFECT] = mode::STATIC;
    }
    b[OFF_J_RAINBOW_1 + LED_COUNT] = 100;
    b[OFF_J_RAINBOW_2 + LED_COUNT] = 100;
    b[OFF_J_CORSAIR + CORSAIR_FAN_FLAGS] = 40;

    // Constructor ("Set up per-LED switching message for synchronized mode").
    let set = |b: &mut [u8; CONFIG_LEN], off: usize, sb: u8, cf: u8| {
        b[off + SPEED_BRIGHTNESS] = sb;
        b[off + COLOR_FLAGS] = cf;
    };
    set(&mut b, OFF_J_RGB_1, 0x08, 0x80);
    set(&mut b, OFF_J_PIPE_1, 0x2A, 0x80);
    set(&mut b, OFF_J_PIPE_2, 0x2A, 0x80);
    set(&mut b, OFF_J_RAINBOW_1, 0x29, 0x80);
    b[OFF_J_RAINBOW_1 + LED_COUNT] = 0x28;
    set(&mut b, OFF_J_RAINBOW_2, 0x29, 0x80);
    b[OFF_J_RAINBOW_2 + LED_COUNT] = 0x28;
    b[OFF_J_CORSAIR + CORSAIR_FAN_FLAGS] = 0x29;
    b[OFF_J_CORSAIR + CORSAIR_QUANTITY] = 0x00;
    b[OFF_J_CORSAIR + CORSAIR_PADDING2] = 0x82;
    b[OFF_J_CORSAIR + CORSAIR_IS_INDIVIDUAL] = 0x78;
    set(&mut b, OFF_J_CORSAIR_OUTERLL120, 0x28, 0x80);
    b[OFF_ON_BOARD[0] + EFFECT] = mode::DIRECT_EFFECT;
    set(
        &mut b,
        OFF_ON_BOARD[0],
        0x29 | SB_SYNC_JRGB,
        PER_LED_FULL_SYNC_MODE,
    );
    for off in &OFF_ON_BOARD[1..] {
        set(&mut b, *off, 0x28, 0x80);
    }
    set(&mut b, OFF_J_RGB_2, 0x2A, 0x80);

    // SelectPerLedProtocol.
    let count = |n: usize| u8::try_from(n).unwrap_or(u8::MAX);
    if sync {
        b[OFF_J_RAINBOW_1 + EFFECT] = mode::STATIC;
        b[OFF_J_RAINBOW_1 + LED_COUNT] = count(SYNC_JRAINBOW_LEDS);
        b[OFF_J_RAINBOW_2 + EFFECT] = mode::STATIC;
        b[OFF_J_RAINBOW_2 + LED_COUNT] = count(SYNC_JRAINBOW_LEDS);
        b[OFF_J_CORSAIR + EFFECT] = mode::STATIC;
        b[OFF_J_CORSAIR + CORSAIR_IS_INDIVIDUAL] = count(SYNC_CORSAIR_LEDS);
        b[OFF_ON_BOARD[0] + COLOR_FLAGS] = PER_LED_FULL_SYNC_MODE;
    } else {
        if headers.jrainbow1 {
            b[OFF_J_RAINBOW_1 + EFFECT] = mode::DIRECT_EFFECT;
            b[OFF_J_RAINBOW_1 + LED_COUNT] = count(JRAINBOW1_MAX_LEDS);
        }
        if headers.jrainbow2 {
            b[OFF_J_RAINBOW_2 + EFFECT] = mode::DIRECT_EFFECT;
            b[OFF_J_RAINBOW_2 + LED_COUNT] = count(JRAINBOW2_MAX_LEDS);
        }
        if headers.jcorsair {
            b[OFF_J_CORSAIR + EFFECT] = mode::DIRECT_EFFECT;
            b[OFF_J_CORSAIR + CORSAIR_IS_INDIVIDUAL] = count(JCORSAIR_MAX_LEDS);
        }
        b[OFF_ON_BOARD[0] + COLOR_FLAGS] = PER_LED_BASIC_SYNC_MODE;
    }
    b
}

/// Which `FeaturePacket_PerLED_185` a buffer is; selects the header bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PerLedTarget {
    /// On-board LEDs plus every header in synchronized mode (`per_led_data_onboard_and_sync`, hdr1 = 6).
    OnboardAndSync,
    /// JRAINBOW1 alone (hdr1 = 4, hdr2 = 0).
    JRainbow1,
    /// JRAINBOW2 alone (hdr1 = 4, hdr2 = 1).
    JRainbow2,
    /// JCORSAIR / JRAINBOW3 alone (hdr1 = 5).
    JCorsair,
}

/// One `FeaturePacket_PerLED_185` (report id 0x53): 5 header bytes and 240 RGB triples.
#[derive(Clone, PartialEq, Eq)]
pub struct PerLedPacket(pub [u8; PER_LED_LEN]);

impl fmt::Debug for PerLedPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PerLedPacket[{}]", cc_hid::hex(&self.0[..PER_LED_HEADER]))
    }
}

impl PerLedPacket {
    /// An all-black packet for `target`.
    pub fn new(target: PerLedTarget) -> Self {
        let (hdr1, hdr2) = match target {
            PerLedTarget::OnboardAndSync => (0x06, 0x00),
            PerLedTarget::JRainbow1 => (0x04, 0x00),
            PerLedTarget::JRainbow2 => (0x04, 0x01),
            PerLedTarget::JCorsair => (0x05, 0x00),
        };
        let mut b = [0u8; PER_LED_LEN];
        b[..PER_LED_HEADER].copy_from_slice(&[PER_LED_REPORT_ID, mode::DIRECT_EFFECT, hdr1, hdr2, 0x00]);
        Self(b)
    }

    /// Sets LED `index` (0..240); out-of-range indices are ignored.
    pub fn set(&mut self, index: usize, color: Rgb) {
        if index < PER_LED_LEDS {
            let o = PER_LED_HEADER + 3 * index;
            self.0[o..o + 3].copy_from_slice(&[color.r, color.g, color.b]);
        }
    }

    /// LED `index`, if in range.
    pub fn get(&self, index: usize) -> Option<Rgb> {
        (index < PER_LED_LEDS).then(|| {
            let o = PER_LED_HEADER + 3 * index;
            Rgb::new(self.0[o], self.0[o + 1], self.0[o + 2])
        })
    }
}

/// The firmware version request (`ReadFwVersion`): output report 0x01, the command byte, then 0xCC padding.
pub fn firmware_request(command: u8) -> [u8; FW_REPORT_LEN] {
    let mut b = [0xCC; FW_REPORT_LEN];
    b[0] = 0x01;
    b[1] = command;
    b
}

/// Formats a version byte as the reference does: high nibble, '.', low nibble.
pub fn format_version(v: u8) -> String {
    format!("{}.{}", v >> 4, v & 0x0F)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn classify_matches_reference_ranges() {
        // `packet_length >= sizeof(X) && packet_length <= sizeof(X) + 1`, tried 185, 162, 112 in that order.
        assert_eq!(classify(185), PacketClass::Bytes185);
        assert_eq!(classify(186), PacketClass::Bytes185);
        assert_eq!(classify(184), PacketClass::Other);
        assert_eq!(classify(187), PacketClass::Other);
        assert_eq!(classify(162), PacketClass::Bytes162);
        assert_eq!(classify(163), PacketClass::Bytes162);
        assert_eq!(classify(112), PacketClass::Bytes112);
        assert_eq!(classify(113), PacketClass::Bytes112);
        assert_eq!(classify(0), PacketClass::Other);
        assert_eq!(classify(761), PacketClass::Other);
        assert_eq!(classify(200), PacketClass::Other);
    }

    #[test]
    fn own_lighting_validation() {
        // A Rainbow wave on JRAINBOW1 with every other zone Static: acceptable once the save byte is 0.
        let mut b = [0u8; CONFIG_LEN];
        b[0] = CONFIG_REPORT_ID;
        for o in ZONE_OFFSETS {
            b[o] = mode::STATIC;
        }
        b[OFF_J_RAINBOW_1] = mode::RAINBOW_WAVE;
        b[SAVE_BYTE] = 1;
        let read = ConfigPacket(b);
        assert!(!read.has_direct_effect());
        assert_eq!(ConfigPacket::from_own_lighting(&b), None, "save byte set");
        let stored = read.own_lighting().expect("valid");
        assert_eq!(stored.len(), CONFIG_LEN);
        assert_eq!(stored[SAVE_BYTE], 0);
        assert_eq!(stored[..SAVE_BYTE], b[..SAVE_BYTE]);
        assert_eq!(
            ConfigPacket::from_own_lighting(&stored).map(|p| p.0.to_vec()),
            Some(stored.clone())
        );

        // Wrong length (162-byte class or truncated), wrong report id: rejected.
        assert_eq!(ConfigPacket::from_own_lighting(&stored[..CONFIG_LEN_162]), None);
        assert_eq!(
            ConfigPacket::from_own_lighting(&[stored.as_slice(), &[0]].concat()),
            None
        );
        assert_eq!(ConfigPacket::from_own_lighting(&[]), None);
        let mut wrong_id = stored.clone();
        wrong_id[0] = 0x53;
        assert_eq!(ConfigPacket::from_own_lighting(&wrong_id), None);

        // The per-LED effect in any zone record: rejected, and never offered for storing.
        for o in ZONE_OFFSETS {
            let mut direct = stored.clone();
            direct[o] = mode::DIRECT_EFFECT;
            assert_eq!(ConfigPacket::from_own_lighting(&direct), None, "offset {o}");
            let packet = ConfigPacket::from_report(&direct).expect("report");
            assert!(packet.has_direct_effect());
            assert_eq!(packet.own_lighting(), None);
        }
        // The enable message itself (sync and unsynchronized) is per-LED.
        let all = Headers {
            jrainbow1: true,
            jrainbow2: true,
            jcorsair: true,
        };
        for sync in [true, false] {
            let enable = enable_direct_packet(sync, all);
            assert_eq!(ConfigPacket::from_own_lighting(&enable), None);
        }
    }

    #[test]
    fn struct_sizes() {
        // ZoneData = effect(1) + color(3) + flags(1) + color2(3) + colorFlags(1) + padding(1) = 10,
        // RainbowZoneData = 11, CorsairZoneData = 1 + 3 + 1 + 1 + 4 + 1 = 11.
        // FeaturePacket_185 = id + 3 ZoneData + 2 Rainbow + Corsair + outerll120 + 10 on-board + j_rgb_2 + save.
        assert_eq!(1 + 3 * 10 + 2 * 11 + 11 + 10 + 10 * 10 + 10 + 1, CONFIG_LEN);
        assert_eq!(SAVE_BYTE, CONFIG_LEN - 1);
        assert_eq!(PER_LED_LEN, 725);
    }

    #[test]
    fn zone_offsets_follow_get_zone_data() {
        // From the `// n` comments of FeaturePacket_185 and the mixed-mode swaps in GetZoneData.
        assert_eq!(zone_offset(MsiZone::JRgb1, true), Some(1));
        assert_eq!(zone_offset(MsiZone::JRgb2, true), Some(134));
        assert_eq!(zone_offset(MsiZone::JRgb2, false), Some(174));
        assert_eq!(zone_offset(MsiZone::JRainbow1, true), Some(31));
        assert_eq!(zone_offset(MsiZone::JRainbow2, true), Some(42));
        assert_eq!(zone_offset(MsiZone::JRainbow3, true), Some(53));
        assert_eq!(zone_offset(MsiZone::JCorsair, false), Some(53));
        assert_eq!(zone_offset(MsiZone::JPipe1, true), Some(11));
        assert_eq!(zone_offset(MsiZone::JPipe2, true), Some(21));
        assert_eq!(zone_offset(MsiZone::OnBoard(0), true), Some(74));
        assert_eq!(zone_offset(MsiZone::OnBoard(6), true), Some(64));
        assert_eq!(zone_offset(MsiZone::OnBoard(6), false), Some(134));
        assert_eq!(zone_offset(MsiZone::OnBoard(9), false), Some(164));
        assert_eq!(zone_offset(MsiZone::OnBoard(10), false), None);
        assert_eq!(led_count_offset(MsiZone::JRainbow1), Some(41));
        assert_eq!(led_count_offset(MsiZone::JRainbow2), Some(52));
        assert_eq!(led_count_offset(MsiZone::JRainbow3), Some(63));
        assert_eq!(led_count_offset(MsiZone::JCorsair), None);
    }

    #[test]
    fn speed_brightness_packing() {
        // ((brightness << 2) & 0x7C) | (speed & 0x03)
        assert_eq!(speed_brightness_flags(speed::MEDIUM, brightness::LEVEL_100), 0x29);
        assert_eq!(speed_brightness_flags(speed::HIGH, 5), 0x16);
        assert_eq!(speed_brightness_flags(0, 0), 0x00);
    }

    /// Expected `enable_per_led_msg` in synchronized mode, written out from the reference:
    /// default initialisers (effect 1, rainbow count 100, corsair fan_flags 40), then the constructor's assignments,
    /// then SelectPerLedProtocol's sync branch (rainbow effect 1 / count 40, corsair is_individual 120,
    /// on-board colorFlags PER_LED_FULL_SYNC_MODE = 0x80|0x01|0x10|0x20|0x02|0x04|0x08 = 0xBF).
    pub(in crate::msi_mystic_light) fn expected_enable_sync() -> Vec<u8> {
        let zone = |effect: u8, sb: u8, cf: u8| vec![effect, 0, 0, 0, sb, 0, 0, 0, cf, 0];
        let mut v = vec![0x52];
        v.extend(zone(0x01, 0x08, 0x80)); // j_rgb_1 @1
        v.extend(zone(0x01, 0x2A, 0x80)); // j_pipe_1 @11
        v.extend(zone(0x01, 0x2A, 0x80)); // j_pipe_2 @21
        v.extend(zone(0x01, 0x29, 0x80)); // j_rainbow_1 @31
        v.push(0x28); //                     cycle_or_led_num = 40
        v.extend(zone(0x01, 0x29, 0x80)); // j_rainbow_2 @42
        v.push(0x28);
        // j_corsair @53: effect 1, color 0, fan_flags 0x29, quantity 0, padding {0, 0, 0x82, 0}, is_individual 0x78
        v.extend([0x01, 0, 0, 0, 0x29, 0x00, 0, 0, 0x82, 0, 0x78]);
        v.extend(zone(0x01, 0x28, 0x80)); // j_corsair_outerll120 @64
        v.extend(zone(0x25, 0xA9, 0xBF)); // on_board_led @74: MSI_DIRECT_MODE, 0x29 | SYNC_JRGB, FULL_SYNC
        for _ in 1..=9 {
            v.extend(zone(0x01, 0x28, 0x80)); // on_board_led_1..9 @84..164
        }
        v.extend(zone(0x01, 0x2A, 0x80)); // j_rgb_2 @174
        v.push(0x00); // save_data @184
        v
    }

    #[test]
    fn enable_direct_sync_is_byte_exact() {
        let expected = expected_enable_sync();
        assert_eq!(expected.len(), CONFIG_LEN);
        let headers = Headers {
            jrainbow1: true,
            jrainbow2: true,
            jcorsair: false,
        };
        assert_eq!(enable_direct_packet(true, headers).to_vec(), expected);
        // In sync mode the header presence does not matter.
        assert_eq!(enable_direct_packet(true, Headers::default()).to_vec(), expected);
    }

    #[test]
    fn enable_direct_non_sync_is_byte_exact() {
        // SelectPerLedProtocol's else branch for MS-7D75 (JRAINBOW1 and 2, no JCORSAIR): rainbow effects become
        // MSI_DIRECT_MODE with counts 200 (0xC8) and 240 (0xF0), j_corsair keeps the constructor values,
        // on-board colorFlags = PER_LED_BASIC_SYNC_MODE = 0x80|0x01|0x10|0x20 = 0xB1.
        let mut expected = expected_enable_sync();
        expected[31] = 0x25;
        expected[41] = 0xC8;
        expected[42] = 0x25;
        expected[52] = 0xF0;
        expected[82] = 0xB1;
        let headers = Headers {
            jrainbow1: true,
            jrainbow2: true,
            jcorsair: false,
        };
        assert_eq!(enable_direct_packet(false, headers).to_vec(), expected);

        // With JCORSAIR: j_corsair effect 0x25, is_individual 240.
        expected[53] = 0x25;
        expected[63] = 0xF0;
        let all = Headers {
            jrainbow1: true,
            jrainbow2: true,
            jcorsair: true,
        };
        assert_eq!(enable_direct_packet(false, all).to_vec(), expected);
    }

    #[test]
    fn sync_decision() {
        let mut c = ConfigPacket([0; CONFIG_LEN]);
        assert!(sync_direct_mode(&c));
        c.set_led_count(MsiZone::JRainbow1, 40);
        c.set_led_count(MsiZone::JRainbow2, 40);
        c.0[63] = 120;
        assert!(sync_direct_mode(&c));
        c.set_led_count(MsiZone::JRainbow1, 41);
        assert!(!sync_direct_mode(&c));
        c.set_led_count(MsiZone::JRainbow1, 0);
        c.0[63] = 121;
        assert!(!sync_direct_mode(&c));
    }

    #[test]
    fn per_led_headers() {
        // FeaturePacket_PerLED_185 { 0x53, 0x25, 0x06, 0x00, 0x00 }, with hdr1/hdr2 overridden in the constructor.
        assert_eq!(
            &PerLedPacket::new(PerLedTarget::OnboardAndSync).0[..5],
            &[0x53, 0x25, 0x06, 0x00, 0x00]
        );
        assert_eq!(
            &PerLedPacket::new(PerLedTarget::JRainbow1).0[..5],
            &[0x53, 0x25, 0x04, 0x00, 0x00]
        );
        assert_eq!(
            &PerLedPacket::new(PerLedTarget::JRainbow2).0[..5],
            &[0x53, 0x25, 0x04, 0x01, 0x00]
        );
        assert_eq!(
            &PerLedPacket::new(PerLedTarget::JCorsair).0[..5],
            &[0x53, 0x25, 0x05, 0x00, 0x00]
        );
        let mut p = PerLedPacket::new(PerLedTarget::OnboardAndSync);
        p.set(1, Rgb::new(1, 2, 3));
        p.set(240, Rgb::new(9, 9, 9));
        assert_eq!(&p.0[5..11], &[0, 0, 0, 1, 2, 3]);
        assert_eq!(p.get(1), Some(Rgb::new(1, 2, 3)));
        assert_eq!(p.get(240), None);
        assert!(p.0[11..].iter().all(|b| *b == 0));
    }

    #[test]
    fn firmware_request_bytes() {
        // request[0] = 0x01, request[1] = 0xB0 / 0xB6, memset(&request[2], 0xCC, 62).
        let r = firmware_request(FW_CMD_APROM);
        assert_eq!(&r[..2], &[0x01, 0xB0]);
        assert!(r[2..].iter().all(|b| *b == 0xCC));
        assert_eq!(firmware_request(FW_CMD_LDROM)[1], 0xB6);
        assert_eq!(format_version(0x12), "1.2");
    }

    #[test]
    fn config_packet_save_and_state() {
        let mut raw = [0u8; CONFIG_LEN];
        raw[0] = 0x52;
        raw[SAVE_BYTE] = 1;
        raw[74] = 0x25;
        raw[1..10].copy_from_slice(&[0x02, 1, 2, 3, 0x29, 4, 5, 6, 0x80]);
        let c = ConfigPacket::from_report(&raw).expect("valid");
        assert!(c.has_direct_effect());
        assert_eq!(c.with_save(false)[SAVE_BYTE], 0);
        assert_eq!(c.with_save(true)[SAVE_BYTE], 1);
        assert_eq!(&c.with_save(false)[..SAVE_BYTE], &raw[..SAVE_BYTE]);
        let s = c.zone_state(MsiZone::JRgb1, true).expect("zone");
        assert_eq!(
            s,
            ZoneState {
                effect: 2,
                speed: 1,
                brightness: 10,
                custom_color: true,
                color: Rgb::new(1, 2, 3),
                color2: Rgb::new(4, 5, 6)
            }
        );
        assert!(ConfigPacket::from_report(&raw[..184]).is_none());
        raw[0] = 0x53;
        assert!(ConfigPacket::from_report(&raw).is_none());
    }

    #[test]
    fn apply_mode_onboard_zone_with_onboard_leds() {
        // SetMode(ON_BOARD_LED_0, BREATHING, MEDIUM, LEVEL_100, false) on a board with 2 on-board LEDs, not mixed:
        // on_board_led @74 and on_board_led_1/2 @84/@94 get effect 2, flags 0x29, colorFlags |= 0x80, padding 0;
        // mode <= LIGHTNING clears SYNC_JRGB (bit 7 of @78) and SYNC_JPIPE1/2 (0x30 of @82).
        let layout = Layout {
            mixed_mode: false,
            no_onboards: false,
            onboard_leds: 2,
        };
        let mut c = ConfigPacket([0; CONFIG_LEN]);
        c.0[82] = 0x31;
        c.0[83] = 7;
        c.apply_mode(
            &layout,
            MsiZone::OnBoard(0),
            mode::BREATHING,
            speed::MEDIUM,
            brightness::LEVEL_100,
            false,
        );
        let mut e = [0u8; CONFIG_LEN];
        for off in [74, 84, 94] {
            e[off] = 2;
            e[off + 4] = 0x29;
            e[off + 8] = 0x80;
        }
        e[82] = 0x81; // 0x31 | 0x80, then & !0x30
        assert_eq!(c.0, e);

        // A mode above LIGHTNING sets the sync bits on the on-board zone instead and leaves LEDs 1..n alone.
        c.apply_mode(&layout, MsiZone::OnBoard(0), mode::METEOR, speed::LOW, 3, true);
        e[74] = 7;
        e[78] = 0x0C | 0x80;
        e[82] = (0x81 & !0x80) | 0x30;
        assert_eq!(c.0, e);
    }
}
