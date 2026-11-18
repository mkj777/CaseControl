// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! The 185-byte Mystic Light controller (`MSIMotherboard185Controller` plus `RGBController_MSIMotherboard185`).

use std::thread;
use std::time::Duration;

use cc_core::{
    Bus, ColorMode, Controller, DeviceDescription, DeviceError, DeviceId, DeviceInfo, DeviceKind, Led, Mode,
    ModeCaps, ModeState, Rgb, Zone, ZoneKind,
};
use cc_hid::{HidError, HidTransport};

use super::boards::{self, BoardConfig, DirectSupport};
use super::protocol::{
    self, CONFIG_LEN, CONFIG_REPORT_ID, ConfigPacket, DETECT_BUFFER_LEN, FW_CMD_APROM, FW_CMD_LDROM,
    FW_REPORT_LEN, Headers, JCORSAIR_MAX_LEDS, JRAINBOW1_MAX_LEDS, JRAINBOW2_MAX_LEDS, Layout, MsiZone,
    PID_COMMON, PacketClass, PerLedPacket, PerLedTarget, SYNC_CORSAIR_LEDS, SYNC_JRAINBOW_LEDS, mode,
};

/// Delay after each per-header packet in the unsynchronized per-LED protocol (reference: `sleep_for(13ms)`).
pub const NON_SYNC_PACKET_DELAY: Duration = Duration::from_millis(13);

/// How long to wait for each firmware version response. The reference uses a blocking `hid_read`.
pub const FW_READ_TIMEOUT: Duration = Duration::from_millis(1000);

/// A Mystic Light board found during enumeration, with its id computed from enumeration data only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// HID path of the matched interface.
    pub path: String,
    /// USB vendor id.
    pub vid: u16,
    /// USB product id.
    pub pid: u16,
    /// Serial number from enumeration, if any.
    pub serial: Option<String>,
    /// Stable id.
    pub id: DeviceId,
}

/// Firmware versions read at open (`ReadFwVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Firmware {
    /// APROM (application) version byte; the firmware gate compares it.
    pub aprom: u8,
    /// LDROM (boot loader) version byte.
    pub ldrom: u8,
}

impl Firmware {
    /// The reference's version string, `"AP/LD <aprom> / <ldrom>"`.
    pub fn display(&self) -> String {
        format!(
            "AP/LD {} / {}",
            protocol::format_version(self.aprom),
            protocol::format_version(self.ldrom)
        )
    }
}

/// The board facts resolved at open.
#[derive(Debug, Clone, Copy)]
struct Board {
    config: BoardConfig,
    layout: Layout,
    headers: Headers,
}

impl Board {
    fn new(board_id: u16, config: BoardConfig) -> Self {
        let has = |z: MsiZone| config.zones.contains(&z);
        Self {
            layout: Layout {
                mixed_mode: board_id >= 0x7D03,
                no_onboards: !has(MsiZone::OnBoard(0)),
                onboard_leds: config.onboard_leds,
            },
            headers: Headers {
                jrainbow1: has(MsiZone::JRainbow1),
                jrainbow2: has(MsiZone::JRainbow2),
                jcorsair: has(MsiZone::JCorsair) || has(MsiZone::JRainbow3),
            },
            config,
        }
    }

    fn per_led(&self) -> bool {
        self.config.direct == DirectSupport::PerLed
    }

    /// `GetMaxDirectLeds`.
    fn max_direct_leds(&self, zone: MsiZone) -> usize {
        let per_led = self.per_led();
        match zone {
            MsiZone::JRgb1 | MsiZone::JRgb2 => 1,
            MsiZone::JPipe1 => usize::from(self.config.pipe1_leds),
            MsiZone::JPipe2 => usize::from(self.config.pipe2_leds),
            MsiZone::JRainbow1 if per_led => JRAINBOW1_MAX_LEDS,
            MsiZone::JRainbow2 | MsiZone::JRainbow3 if per_led => JRAINBOW2_MAX_LEDS,
            MsiZone::JCorsair if per_led => JCORSAIR_MAX_LEDS,
            MsiZone::OnBoard(0) => usize::from(self.config.onboard_leds),
            _ => 1,
        }
    }

    /// Start of each zone in the synchronized per-LED buffer (`per_led_*` pointers set up in the constructor).
    fn sync_offset(&self, zone: MsiZone) -> Option<usize> {
        let onboard = 0;
        let pipe1 = onboard + usize::from(self.config.onboard_leds);
        let pipe2 = pipe1 + usize::from(self.config.pipe1_leds);
        let jrgb = pipe2 + usize::from(self.config.pipe2_leds);
        let jrainbow1 = jrgb + usize::from(self.config.jrgbs);
        let jrainbow2 = jrainbow1 + SYNC_JRAINBOW_LEDS;
        let jcorsair = jrainbow2 + SYNC_JRAINBOW_LEDS;
        Some(match zone {
            MsiZone::OnBoard(0) => onboard,
            MsiZone::JPipe1 => pipe1,
            MsiZone::JPipe2 => pipe2,
            MsiZone::JRgb1 => jrgb,
            MsiZone::JRgb2 => jrgb + 1,
            MsiZone::JRainbow1 => jrainbow1,
            MsiZone::JRainbow2 => jrainbow2,
            MsiZone::JRainbow3 | MsiZone::JCorsair => jcorsair,
            _ => return None,
        })
    }

    fn has_per_led_data(&self) -> bool {
        !self.layout.no_onboards || self.headers.jrainbow1 || self.headers.jrainbow2 || self.headers.jcorsair
    }
}

/// The four per-LED report buffers of the reference controller.
#[derive(Debug, Clone)]
struct PerLedBuffers {
    onboard_and_sync: PerLedPacket,
    jrainbow1: PerLedPacket,
    jrainbow2: PerLedPacket,
    jcorsair: PerLedPacket,
}

impl PerLedBuffers {
    fn new() -> Self {
        Self {
            onboard_and_sync: PerLedPacket::new(PerLedTarget::OnboardAndSync),
            jrainbow1: PerLedPacket::new(PerLedTarget::JRainbow1),
            jrainbow2: PerLedPacket::new(PerLedTarget::JRainbow2),
            jcorsair: PerLedPacket::new(PerLedTarget::JCorsair),
        }
    }
}

/// Mode table of `RGBController_MSIMotherboard185::SetupModes` (commented-out reference modes are left out).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModeKind {
    /// `MODE_FLAG_HAS_BRIGHTNESS | MODE_FLAG_HAS_PER_LED_COLOR | MODE_FLAG_MANUAL_SAVE`.
    Static,
    /// `PER_LED_ONLY`.
    PerLedOnly,
    /// `RANDOM_ONLY`.
    RandomOnly,
    /// `COMMON` = `RANDOM_ONLY | MODE_FLAG_HAS_PER_LED_COLOR`.
    Common,
}

const HARDWARE_MODES: &[(&str, u8, ModeKind)] = &[
    ("Static", mode::STATIC, ModeKind::Static),
    ("Breathing", mode::BREATHING, ModeKind::PerLedOnly),
    ("Flashing", mode::FLASHING, ModeKind::Common),
    ("Double flashing", mode::DOUBLE_FLASHING, ModeKind::Common),
    ("Lightning", mode::LIGHTNING, ModeKind::PerLedOnly),
    ("Meteor", mode::METEOR, ModeKind::Common),
    ("Color ring", mode::COLOR_RING, ModeKind::RandomOnly),
    ("Planetary", mode::PLANETARY, ModeKind::RandomOnly),
    ("Double meteor", mode::DOUBLE_METEOR, ModeKind::RandomOnly),
    ("Energy", mode::ENERGY, ModeKind::RandomOnly),
    ("Blink", mode::BLINK, ModeKind::Common),
    ("Clock", mode::CLOCK, ModeKind::RandomOnly),
    ("Color pulse", mode::COLOR_PULSE, ModeKind::Common),
    ("Color shift", mode::COLOR_SHIFT, ModeKind::RandomOnly),
    ("Color wave", mode::COLOR_WAVE, ModeKind::Common),
    ("Marquee", mode::MARQUEE, ModeKind::PerLedOnly),
    ("Rainbow wave", mode::RAINBOW_WAVE, ModeKind::RandomOnly),
    ("Visor", mode::VISOR, ModeKind::Common),
    ("Rainbow flashing", mode::RAINBOW_FLASHING, ModeKind::RandomOnly),
    (
        "Color ring double flashing",
        mode::COLOR_RING_DOUBLE_FLASHING,
        ModeKind::RandomOnly,
    ),
    ("Stack", mode::STACK, ModeKind::Common),
    ("Fire", mode::FIRE, ModeKind::RandomOnly),
];

fn hardware_mode(id: u8, name: &str, kind: ModeKind) -> Mode {
    let (caps, color_mode) = match kind {
        ModeKind::Static => (
            ModeCaps::BRIGHTNESS | ModeCaps::PER_LED_COLOR | ModeCaps::SAVE_MANUAL,
            ColorMode::PerLed,
        ),
        ModeKind::PerLedOnly => (
            ModeCaps::BRIGHTNESS | ModeCaps::SPEED | ModeCaps::PER_LED_COLOR | ModeCaps::SAVE_MANUAL,
            ColorMode::PerLed,
        ),
        ModeKind::RandomOnly => (
            ModeCaps::BRIGHTNESS | ModeCaps::SPEED | ModeCaps::RANDOM_COLOR | ModeCaps::SAVE_MANUAL,
            ColorMode::Random,
        ),
        ModeKind::Common => (
            ModeCaps::BRIGHTNESS
                | ModeCaps::SPEED
                | ModeCaps::RANDOM_COLOR
                | ModeCaps::PER_LED_COLOR
                | ModeCaps::SAVE_MANUAL,
            ColorMode::PerLed,
        ),
    };
    Mode {
        id: u32::from(id),
        name: name.to_owned(),
        caps,
        color_mode,
        speed: caps
            .contains(ModeCaps::SPEED)
            .then_some(u32::from(protocol::speed::LOW)..=u32::from(protocol::speed::HIGH)),
        brightness: caps
            .contains(ModeCaps::BRIGHTNESS)
            .then_some(u32::from(protocol::brightness::OFF)..=u32::from(protocol::brightness::LEVEL_100)),
        colors: None,
    }
}

/// The modes this board exposes; the direct mode (id 100) only on per-LED boards.
fn modes(board: &Board) -> Vec<Mode> {
    let mut out = Vec::with_capacity(HARDWARE_MODES.len() + 1);
    if board.per_led() {
        out.push(Mode {
            id: u32::from(mode::DIRECT_DUMMY),
            name: "Direct".to_owned(),
            caps: ModeCaps::PER_LED_COLOR,
            color_mode: ColorMode::PerLed,
            speed: None,
            brightness: None,
            colors: None,
        });
    }
    out.extend(
        HARDWARE_MODES
            .iter()
            .map(|(name, id, kind)| hardware_mode(*id, name, *kind)),
    );
    out
}

/// Zones and LEDs as `RGBController_MSIMotherboard185::SetupZones` builds them, with `lens[i]` as the size of each
/// addressable header (ignored for fixed zones).
fn layout_zones(board: &Board, lens: &[usize]) -> (Vec<Zone>, Vec<Led>) {
    let mut zones = Vec::with_capacity(board.config.zones.len());
    let mut leds = Vec::new();
    for (i, z) in board.config.zones.iter().enumerate() {
        let name = z.name().unwrap_or("ZONE").to_owned();
        let max = board.max_direct_leds(*z);
        let (kind, len, resizable) = if z.is_addressable() {
            let len = lens.get(i).copied().unwrap_or(0).min(max);
            (ZoneKind::Linear, len, Some(0..=max))
        } else {
            (ZoneKind::Single, max, None)
        };
        leds.extend((0..len).map(|n| Led {
            name: format!("{name}, LED {}", n + 1),
        }));
        zones.push(Zone {
            name,
            kind,
            start: leds.len() - len,
            len,
            resizable,
            brightness_only: false,
        });
    }
    (zones, leds)
}

/// The mode the board shows on its own, decoded from configuration `config` (`GetMode` of the first zone with LEDs,
/// as `RGBController_MSIMotherboard185` reads the active mode): its effect as one of `desc`'s hardware modes, speed and
/// brightness clamped to the mode's ranges, and for per-LED colour modes each zone's first colour on all its LEDs.
/// `None` for the per-LED (direct) effect, "disabled" and effects without a mode, since their colours are unknown;
/// also `None` for a configuration with any zone in per-LED mode (effect 0x25), whose other zone records (Static, as
/// the enable message sets them) do not describe what the board shows.
fn hardware_state(config: &ConfigPacket, board: &Board, desc: &DeviceDescription) -> Option<ModeState> {
    if config.has_direct_effect() {
        return None;
    }
    let mixed = board.layout.mixed_mode;
    let (first, _) = board
        .config
        .zones
        .iter()
        .zip(&desc.zones)
        .find(|(_, z)| z.len > 0)?;
    let zs = config.zone_state(*first, mixed)?;
    let mode = desc
        .modes
        .iter()
        .find(|m| m.id == u32::from(zs.effect) && desc.direct_mode != Some(m.id))?;
    let clamp = |v: u8, r: &Option<std::ops::RangeInclusive<u32>>| {
        r.as_ref().map(|r| u32::from(v).clamp(*r.start(), *r.end()))
    };
    let colors = if mode.color_mode == ColorMode::PerLed {
        let mut out = Vec::with_capacity(desc.led_count());
        for (msi, zone) in board.config.zones.iter().zip(&desc.zones) {
            let color = config.zone_state(*msi, mixed).map_or(Rgb::BLACK, |z| z.color);
            out.extend(std::iter::repeat_n(color, zone.len));
        }
        out
    } else {
        Vec::new()
    };
    let state = ModeState {
        mode: mode.id,
        speed: clamp(zs.speed, &mode.speed),
        brightness: clamp(zs.brightness, &mode.brightness),
        direction: None,
        colors,
    };
    desc.validate_state(&state).is_ok().then_some(state)
}

fn hid_err(e: HidError) -> DeviceError {
    DeviceError::from(e)
}

/// Driver for the 185-byte Mystic Light class.
///
/// Owns the opened HID interface. All methods run on the caller's thread; the only sleep is the reference's 13 ms
/// gap between per-header packets in the unsynchronized per-LED protocol.
pub struct MysticLight185 {
    dev: Box<dyn HidTransport>,
    desc: DeviceDescription,
    board: Board,
    board_id: u16,
    firmware: Firmware,
    /// The configuration read at open, or the remembered last good own lighting when the board was opened with the
    /// "unreleased" marker (ADR 0001 #38); `release()` sends it back with the save byte forced to zero.
    original: ConfigPacket,
    /// `original` is a state CaseControl left on the board (opened with the "unreleased" marker and no usable
    /// remembered own lighting): it is not reported as own lighting, and the marker stays after a release.
    left_by_us: bool,
    /// The board does not show `original` yet (restoring the remembered own lighting at open failed): `release()`
    /// sends it even if nothing was sent since open.
    restore_pending: bool,
    /// Working configuration (`data` in the reference): the original plus our header sizes and mode changes.
    data: ConfigPacket,
    per_led: PerLedBuffers,
    colors: Vec<Rgb>,
    sync: bool,
    direct: bool,
    direct_configured: bool,
    /// Something was sent since open or the last `release()`.
    touched: bool,
}

impl std::fmt::Debug for MysticLight185 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysticLight185")
            .field("id", &self.desc.info.id)
            .field("board_id", &format_args!("{:04x}", self.board_id))
            .field("firmware", &self.firmware)
            .field("sync", &self.sync)
            .field("direct", &self.direct)
            .finish_non_exhaustive()
    }
}

/// Reads the configuration report into a buffer of `len` bytes and returns the byte count.
fn get_config(dev: &mut dyn HidTransport, len: usize) -> Result<(Vec<u8>, usize), DeviceError> {
    let mut buf = vec![0u8; len];
    buf[0] = CONFIG_REPORT_ID;
    let n = dev.get_feature_report(&mut buf).map_err(hid_err)?;
    Ok((buf, n))
}

fn read_config(dev: &mut dyn HidTransport) -> Result<ConfigPacket, DeviceError> {
    let (buf, n) = get_config(dev, CONFIG_LEN)?;
    if n != CONFIG_LEN {
        return Err(DeviceError::Protocol(format!(
            "configuration report 0x52 returned {n} bytes, expected {CONFIG_LEN}"
        )));
    }
    ConfigPacket::from_report(&buf)
        .ok_or_else(|| DeviceError::Protocol("configuration report has the wrong report id".into()))
}

/// Reads the length of the configuration report as the reference detector does (`hid_get_feature_report(0x52)` into a
/// 200-byte buffer). Read-only.
pub fn read_config_len(dev: &mut dyn HidTransport) -> Result<usize, DeviceError> {
    get_config(dev, DETECT_BUFFER_LEN).map(|(_, n)| n)
}

/// One firmware version query (`ReadFwVersion`): write the request, read 64 bytes, take byte 2. Like the reference,
/// a response that does not arrive leaves the version at the previous response's value (zero at first).
fn query_version(
    dev: &mut dyn HidTransport,
    command: u8,
    response: &mut [u8; FW_REPORT_LEN],
) -> Result<u8, DeviceError> {
    dev.write(&protocol::firmware_request(command)).map_err(hid_err)?;
    match dev.read_timeout(response, FW_READ_TIMEOUT) {
        Ok(_) => {}
        Err(HidError::Timeout) => {
            tracing::warn!(target: "cc_drivers::msi", command, "firmware version request timed out");
        }
        Err(e) => return Err(hid_err(e)),
    }
    Ok(response[2])
}

fn read_firmware(dev: &mut dyn HidTransport) -> Result<Firmware, DeviceError> {
    let mut response = [0u8; FW_REPORT_LEN];
    let aprom = query_version(dev, FW_CMD_APROM, &mut response)?;
    let ldrom = query_version(dev, FW_CMD_LDROM, &mut response)?;
    Ok(Firmware { aprom, ldrom })
}

/// The board id: the USB PID, or for common-PID boards the first four hex digits of the serial
/// (`std::stoi(GetSerial().substr(0, 4), nullptr, 16)`).
pub fn board_id(pid: u16, serial: Option<&str>) -> Result<u16, DeviceError> {
    if pid != PID_COMMON {
        return Ok(pid);
    }
    serial
        .and_then(|s| s.get(..4))
        .and_then(|s| u16::from_str_radix(s, 16).ok())
        .ok_or_else(|| {
            DeviceError::Protocol(format!(
                "common-PID board without a usable board id in its serial ({serial:?})"
            ))
        })
}

impl MysticLight185 {
    /// Brings up the controller on an opened interface, as the reference detector and constructor do:
    ///
    /// 1. `get_feature_report(0x52)` into a 200-byte buffer; anything but the 185-byte class fails.
    /// 2. Firmware version queries (output reports `01 B0 CC..` and `01 B6 CC..`, each followed by a 64-byte read).
    /// 3. The firmware gate (`GetFirmwareVersionInvalid`).
    /// 4. `get_feature_report(0x52)` again into a 185-byte buffer: the configuration that `release()` restores.
    ///
    /// Boards whose reference configuration uses the zone-based direct mode fail before step 2.
    ///
    /// Same as [`open_with_stored`](Self::open_with_stored) without a remembered own lighting.
    pub fn open(dev: Box<dyn HidTransport>, candidate: &Candidate) -> Result<Self, DeviceError> {
        Self::open_with_stored(dev, candidate, None, false)
    }

    /// [`open`](Self::open) with what the engine remembers about the board (ADR 0001 #38): `stored`, its last good
    /// own lighting (the bytes an earlier controller reported through
    /// [`Controller::own_lighting`](cc_core::Controller::own_lighting)), and `unreleased`, whether CaseControl wrote
    /// to it and did not hand it back (crash, sleep, re-enumeration).
    ///
    /// - `unreleased`: the configuration read at open is CaseControl's, not the board's own lighting, and is never
    ///   reported as own lighting. If `stored` passes [`ConfigPacket::from_own_lighting`] (185 bytes, report id 0x52,
    ///   save byte 0, no zone in the per-LED effect), it is restored right away (the same two reports as `release()`)
    ///   and becomes what `release()` restores and [`hardware_mode`](cc_core::Controller::hardware_mode) reports, so
    ///   the board and the UI agree; if that write fails, `release()` sends it even when nothing else was sent.
    ///   Without a usable `stored`, `release()` restores the readback and
    ///   [`shows_own_lighting`](cc_core::Controller::shows_own_lighting) stays `false`.
    /// - Not `unreleased` and the readback is per-LED: another program drives the board. Nothing is stored or
    ///   restored, and the release of an untouched controller sends nothing (warning).
    /// - Not `unreleased` and the readback is not per-LED: it is what `release()` restores, and it is reported as the
    ///   new own lighting to remember.
    pub fn open_with_stored(
        mut dev: Box<dyn HidTransport>,
        candidate: &Candidate,
        stored: Option<&[u8]>,
        unreleased: bool,
    ) -> Result<Self, DeviceError> {
        let len = read_config_len(dev.as_mut())?;
        match protocol::classify(len) {
            PacketClass::Bytes185 => {}
            class => {
                return Err(DeviceError::Protocol(format!(
                    "configuration report is {len} bytes ({class:?}); only the 185-byte class is supported"
                )));
            }
        }

        let board_id = board_id(candidate.pid, candidate.serial.as_deref())?;
        let listed = boards::board_config(board_id);
        if listed.is_some_and(|c| c.direct == DirectSupport::ZoneBased) {
            return Err(DeviceError::Protocol(format!(
                "board {board_id:04x} uses the zone-based direct mode, which is not supported"
            )));
        }

        let firmware = read_firmware(dev.as_mut())?;
        if boards::firmware_version_invalid(firmware.aprom, listed) {
            return Err(DeviceError::Protocol(format!(
                "controller firmware {} is below the minimum supported version",
                firmware.display()
            )));
        }

        let read = read_config(dev.as_mut())?;
        let good = if unreleased {
            let good = stored.and_then(ConfigPacket::from_own_lighting);
            if good.is_none() && stored.is_some() {
                tracing::warn!(
                    target: "cc_drivers::msi",
                    id = %candidate.id,
                    "stored own lighting is not a valid 185-byte configuration; ignoring it"
                );
            }
            good
        } else {
            if read.has_direct_effect() {
                tracing::warn!(
                    target: "cc_drivers::msi",
                    id = %candidate.id,
                    "board is in per-LED mode, driven by another program; an untouched release leaves it that way"
                );
            }
            None
        };
        let left_by_us = unreleased && good.is_none();
        if left_by_us {
            tracing::warn!(
                target: "cc_drivers::msi",
                id = %candidate.id,
                "board shows a state CaseControl left and no own lighting is stored for it; release() restores that state"
            );
        }
        let restore = good.is_some();
        let original = good.unwrap_or(read);

        let config = listed.copied().unwrap_or(boards::DEFAULT_BOARD);
        let board = Board::new(board_id, config);
        let name = if config.name.is_empty() {
            format!("MSI Mystic Light MS_{board_id:04X}")
        } else {
            format!("MSI {}", config.name)
        };
        let info = DeviceInfo {
            id: candidate.id.clone(),
            name,
            vendor: "MSI".to_owned(),
            kind: DeviceKind::Motherboard,
            bus: Bus::Hid,
            location: candidate.path.clone(),
            serial: candidate.serial.clone(),
            firmware: Some(firmware.display()),
        };
        let mut c = Self::assemble(dev, info, board, board_id, firmware, original);
        c.left_by_us = left_by_us;
        if restore {
            match c.hand_back() {
                Ok(()) => tracing::info!(
                    target: "cc_drivers::msi",
                    id = %c.desc.info.id,
                    "board showed a state CaseControl left; restored the last good own lighting stored for it"
                ),
                Err(e) => {
                    tracing::warn!(
                        target: "cc_drivers::msi",
                        id = %c.desc.info.id,
                        error = %e,
                        "restoring the stored own lighting at open failed; release() will try again"
                    );
                    c.restore_pending = true;
                }
            }
            // Only the restore was sent: the board is not under software control.
            c.touched = false;
        }
        Ok(c)
    }

    fn assemble(
        dev: Box<dyn HidTransport>,
        info: DeviceInfo,
        board: Board,
        board_id: u16,
        firmware: Firmware,
        original: ConfigPacket,
    ) -> Self {
        // Header sizes start from the LED-count bytes the board reported (`cycle_or_led_num`; JCORSAIR has none and
        // starts at 0), clamped to the resizable range by `layout_zones`. OpenRGB starts at 0 instead, which would
        // make the first hardware mode write count 0 to every header.
        let lens: Vec<usize> = board
            .config
            .zones
            .iter()
            .map(|z| usize::from(original.led_count(*z).unwrap_or(0)))
            .collect();
        let (zones, leds) = layout_zones(&board, &lens);
        let desc = DeviceDescription {
            info,
            zones,
            leds,
            modes: modes(&board),
            direct_mode: board.per_led().then_some(u32::from(mode::DIRECT_DUMMY)),
            keepalive: None,
        };
        debug_assert!(desc.validate().is_ok(), "{:?}", desc.validate());

        // SetupZones calls SetZoneLEDCount with each header's size on the working copy (unchanged unless the
        // read count was clamped).
        let mut data = original.clone();
        for (z, zone) in board.config.zones.iter().zip(&desc.zones) {
            data.set_led_count(*z, u8::try_from(zone.len).unwrap_or(u8::MAX));
        }
        let sync = protocol::sync_direct_mode(&data);
        let colors = vec![Rgb::BLACK; desc.leds.len()];
        Self {
            dev,
            desc,
            board,
            board_id,
            firmware,
            original,
            left_by_us: false,
            restore_pending: false,
            data,
            per_led: PerLedBuffers::new(),
            colors,
            sync,
            direct: false,
            direct_configured: false,
            touched: false,
        }
    }

    /// The configuration `release()` restores: the one read from the device at open, or the remembered own lighting
    /// restored at open (see [`open_with_stored`](Self::open_with_stored)).
    pub fn original_config(&self) -> &ConfigPacket {
        &self.original
    }

    /// The working configuration the next hardware-mode update sends.
    pub fn working_config(&self) -> &ConfigPacket {
        &self.data
    }

    /// Firmware versions read at open.
    pub fn firmware(&self) -> Firmware {
        self.firmware
    }

    /// The board id (USB PID or the common-PID board's virtual PID).
    pub fn board_id(&self) -> u16 {
        self.board_id
    }

    /// The MSI zone behind each zone of the description, in the same order.
    pub fn msi_zones(&self) -> &'static [MsiZone] {
        self.board.config.zones
    }

    /// Whether the board has mixed-mode support (board id `>= 0x7D03`), which moves JRGB2 in the config packet.
    pub fn mixed_mode(&self) -> bool {
        self.board.layout.mixed_mode
    }

    /// Whether the synchronized per-LED protocol is in use (all headers within 40/40/120 LEDs).
    pub fn sync_direct(&self) -> bool {
        self.sync
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
        self.touched = true;
        self.dev.send_feature_report(bytes).map_err(hid_err)
    }

    /// `SelectPerLedProtocol`: recompute the sync flag from the working configuration and, in direct mode on a
    /// per-LED board, send the enable message.
    fn select_per_led_protocol(&mut self) -> Result<(), DeviceError> {
        self.sync = protocol::sync_direct_mode(&self.data);
        if self.direct && self.board.per_led() {
            let packet = protocol::enable_direct_packet(self.sync, self.board.headers);
            self.send(&packet)?;
            self.direct_configured = true;
        }
        Ok(())
    }

    /// `SetDirectMode(true)` unless direct mode is already active.
    fn enter_direct(&mut self) -> Result<(), DeviceError> {
        if !self.board.per_led() {
            return Err(DeviceError::Unsupported);
        }
        if !self.direct {
            self.direct = true;
            self.direct_configured = false;
            self.select_per_led_protocol()?;
        }
        Ok(())
    }

    /// `SetLedColor` on a per-LED board.
    fn set_led_color(&mut self, zone: MsiZone, index: usize, color: Rgb) {
        let mut max = self.board.max_direct_leds(zone);
        if self.sync {
            match zone {
                MsiZone::JRainbow1 | MsiZone::JRainbow2 => max = SYNC_JRAINBOW_LEDS,
                MsiZone::JRainbow3 | MsiZone::JCorsair => max = SYNC_CORSAIR_LEDS,
                _ => {}
            }
        }
        if index >= max {
            return;
        }
        let (packet, base) = match zone {
            MsiZone::JRainbow1 if !self.sync => (&mut self.per_led.jrainbow1, 0),
            MsiZone::JRainbow2 if !self.sync => (&mut self.per_led.jrainbow2, 0),
            MsiZone::JRainbow3 | MsiZone::JCorsair if !self.sync => (&mut self.per_led.jcorsair, 0),
            _ => match self.board.sync_offset(zone) {
                Some(base) => (&mut self.per_led.onboard_and_sync, base),
                None => return,
            },
        };
        packet.set(base + index, color);
    }

    /// Copies zone `zone`'s colors from the color buffer into the per-LED packets (`UpdateLed`, last LED first).
    fn stage_zone(&mut self, zone: usize) {
        let Some(&msi) = self.board.config.zones.get(zone) else {
            return;
        };
        let range = self.desc.zones[zone].range();
        for (index, led) in range.enumerate().rev() {
            let color = self.colors[led];
            self.set_led_color(msi, index, color);
        }
    }

    /// `Update(false)` in direct mode on a per-LED board.
    fn update_direct(&mut self) -> Result<(), DeviceError> {
        if !self.board.has_per_led_data() {
            // The reference sends nothing when no on-board or addressable zone exists.
            return Ok(());
        }
        if !self.direct_configured {
            self.select_per_led_protocol()?;
        }
        if !self.sync {
            let headers = self.board.headers;
            if headers.jrainbow1 {
                let p = self.per_led.jrainbow1.0;
                self.send(&p)?;
                thread::sleep(NON_SYNC_PACKET_DELAY);
            }
            if headers.jrainbow2 {
                let p = self.per_led.jrainbow2.0;
                self.send(&p)?;
                thread::sleep(NON_SYNC_PACKET_DELAY);
            }
            if headers.jcorsair {
                let p = self.per_led.jcorsair.0;
                self.send(&p)?;
                thread::sleep(NON_SYNC_PACKET_DELAY);
            }
        }
        let p = self.per_led.onboard_and_sync.0;
        self.send(&p)
    }

    /// `Update(save)` outside direct mode: read the board's current configuration, send it back, then send `next`
    /// ("Windows Mystic Light sends two consecutive reports"). `save` sets the save-to-flash byte of both.
    fn update_hardware(&mut self, next: &ConfigPacket, save: bool) -> Result<(), DeviceError> {
        let current = read_config(self.dev.as_mut())?;
        self.send(&current.with_save(save))?;
        self.send(&next.with_save(save))
    }

    /// The hand-back transition shared by `release()` and the restore at open: leave direct mode, read the current
    /// configuration and send it back, then send `original` with save_data forced to 0 (the same two reports as a
    /// hardware-mode update). If reading the current state fails, the original is still sent.
    fn hand_back(&mut self) -> Result<(), DeviceError> {
        self.direct = false;
        self.direct_configured = false;
        self.sync = protocol::sync_direct_mode(&self.data);
        match read_config(self.dev.as_mut()) {
            Ok(current) => {
                if let Err(e) = self.send(&current.with_save(false)) {
                    tracing::warn!(target: "cc_drivers::msi", error = %e, "release: resending current state failed");
                }
            }
            Err(e) => {
                tracing::warn!(target: "cc_drivers::msi", error = %e, "release: reading current state failed");
            }
        }
        let original = self.original.with_save(false);
        self.send(&original)
    }

    fn check_zone(&self, zone: usize) -> Result<(), DeviceError> {
        if zone < self.desc.zones.len() {
            Ok(())
        } else {
            Err(DeviceError::InvalidArgument(format!("no zone {zone}")))
        }
    }
}

impl Controller for MysticLight185 {
    fn description(&self) -> &DeviceDescription {
        &self.desc
    }

    fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError> {
        if colors.len() != self.colors.len() {
            return Err(DeviceError::InvalidArgument(format!(
                "{} colors given, device has {} LEDs",
                colors.len(),
                self.colors.len()
            )));
        }
        self.enter_direct()?;
        self.colors.copy_from_slice(colors);
        for zone in 0..self.desc.zones.len() {
            self.stage_zone(zone);
        }
        self.update_direct()
    }

    fn set_zone_direct(&mut self, zone: usize, colors: &[Rgb]) -> Result<(), DeviceError> {
        self.check_zone(zone)?;
        let range = self.desc.zones[zone].range();
        if colors.len() != range.len() {
            return Err(DeviceError::InvalidArgument(format!(
                "{} colors given, zone {zone} has {} LEDs",
                colors.len(),
                range.len()
            )));
        }
        self.enter_direct()?;
        self.colors[range].copy_from_slice(colors);
        self.stage_zone(zone);
        self.update_direct()
    }

    fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError> {
        self.desc
            .validate_state(state)
            .map_err(|e| DeviceError::InvalidArgument(e.to_string()))?;

        if state.mode == u32::from(mode::DIRECT_DUMMY) {
            // SetDirectMode(true) always re-sends the enable message, then the colors are shown.
            self.direct = false;
            self.enter_direct()?;
            return self.set_direct(&state.colors);
        }

        let m = self
            .desc
            .mode(state.mode)
            .ok_or(DeviceError::InvalidArgument(format!(
                "unknown mode {}",
                state.mode
            )))?;
        let random = m.color_mode == ColorMode::Random;
        let has_speed = m.caps.contains(ModeCaps::SPEED);
        if m.color_mode == ColorMode::PerLed {
            self.colors.copy_from_slice(&state.colors);
        }
        let effect = u8::try_from(state.mode).map_err(|_| DeviceError::InvalidArgument("mode id".into()))?;
        // Reference defaults: speed MEDIUM for modes with speed, 0 otherwise; brightness LEVEL_100.
        let default_speed = if has_speed { protocol::speed::MEDIUM } else { 0 };
        let speed = state
            .speed
            .and_then(|s| u8::try_from(s).ok())
            .unwrap_or(default_speed);
        let brightness = state
            .brightness
            .and_then(|b| u8::try_from(b).ok())
            .unwrap_or(protocol::brightness::LEVEL_100);

        // SetDirectMode(false): no packet outside direct mode, only the protocol selection.
        self.direct = false;
        self.direct_configured = false;
        self.select_per_led_protocol()?;

        // DeviceUpdateLEDs -> UpdateLed: for each zone with LEDs, SetMode and SetZoneColor with its first LED. The
        // off output (Static at brightness 0, ADR 0001 #38) also darkens headers sized 0, which may still have LEDs
        // showing their previous effect.
        let off = state.mode == u32::from(mode::STATIC) && brightness == protocol::brightness::OFF;
        let layout = self.board.layout;
        for (msi, zone) in self.board.config.zones.iter().zip(&self.desc.zones) {
            let color = if zone.len > 0 {
                self.colors[zone.start]
            } else if off {
                Rgb::BLACK
            } else {
                continue;
            };
            self.data
                .apply_mode(&layout, *msi, effect, speed, brightness, random);
            self.data.apply_zone_color(&layout, *msi, color, color);
        }
        let next = self.data.clone();
        self.update_hardware(&next, false)
    }

    fn resize_zone(&mut self, zone: usize, len: usize) -> Result<(), DeviceError> {
        self.check_zone(zone)?;
        let Some(allowed) = self.desc.zones[zone].resizable.clone() else {
            return Err(DeviceError::Unsupported);
        };
        if !allowed.contains(&len) {
            return Err(DeviceError::InvalidArgument(format!(
                "zone {zone} accepts {}..={} LEDs, got {len}",
                allowed.start(),
                allowed.end()
            )));
        }
        let mut lens: Vec<usize> = self.desc.zones.iter().map(|z| z.len).collect();
        lens[zone] = len;
        let (zones, leds) = layout_zones(&self.board, &lens);

        // Keep each zone's colors where the zone still has room.
        let mut colors = vec![Rgb::BLACK; leds.len()];
        for (old, new) in self.desc.zones.iter().zip(&zones) {
            let n = old.len.min(new.len);
            colors[new.start..new.start + n].copy_from_slice(&self.colors[old.start..old.start + n]);
        }
        self.desc.zones = zones;
        self.desc.leds = leds;
        self.colors = colors;
        debug_assert!(self.desc.validate().is_ok(), "{:?}", self.desc.validate());

        // SetZoneLEDCount: the header's count byte in the working configuration, then SelectPerLedProtocol (which
        // re-sends the enable message in direct mode). The reference does this once per resizable zone; one send
        // is enough because the message depends only on the final counts.
        let msi = self.board.config.zones[zone];
        self.data.set_led_count(msi, u8::try_from(len).unwrap_or(u8::MAX));
        self.select_per_led_protocol()
    }

    fn save_to_hardware(&mut self) -> Result<(), DeviceError> {
        if self.direct {
            // In direct mode the reference's Update(true) only sends a per-LED frame; there is nothing to save.
            return Err(DeviceError::Unsupported);
        }
        let next = self.data.clone();
        self.update_hardware(&next, true)
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        if !self.touched && !self.restore_pending {
            return Ok(());
        }
        self.hand_back()?;
        self.touched = false;
        self.restore_pending = false;
        Ok(())
    }

    /// The mode in the configuration `release()` restores (read at open, or the remembered own lighting).
    fn hardware_mode(&self) -> Option<ModeState> {
        hardware_state(&self.original, &self.board, &self.desc)
    }

    /// `original` (save byte 0) unless it is a state CaseControl left or has a zone in per-LED mode. When `original`
    /// is the remembered own lighting restored at open, these are the bytes already stored.
    fn own_lighting(&self) -> Option<Vec<u8>> {
        if self.left_by_us {
            None
        } else {
            self.original.own_lighting()
        }
    }

    /// `false` while `original` is a state CaseControl left, or while the restore at open is still pending.
    fn shows_own_lighting(&self) -> bool {
        !self.left_by_us && !self.restore_pending
    }

    /// Static at brightness 0: a hardware state, so brightness 0 does not switch the board to per-LED mode (whose
    /// enable message outlives the app if it ends without a release; ADR 0001 #38).
    fn off_mode(&self) -> Option<u32> {
        Some(u32::from(mode::STATIC))
    }
}
