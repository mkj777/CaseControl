// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! The 185-byte Mystic Light controller (`MSIMotherboard185Controller` plus `RGBController_MSIMotherboard185`).

use std::thread;
use std::time::Duration;

use cc_core::{
    Bus, ColorMode, Controller, DeviceDescription, DeviceError, DeviceId, DeviceInfo, DeviceKind, Led, Mode,
    ModeCaps, ModeState, OwnLighting, OwnSource, PersistCaps, PersistLook, PersistOutcome, Rgb, Zone,
    ZoneKind,
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
    /// What `release()` sends back (save byte forced to zero) and where it came from (ADR 0001 #38): the configuration
    /// read at open, the remembered own lighting of a board opened with the "unreleased" marker, or what this session
    /// wrote to flash. While the board does not show a remembered own lighting yet (restoring it at open failed, or
    /// was skipped because a setting is about to drive the board), `release()` sends it even if nothing was sent.
    own: OwnLighting<ConfigPacket>,
    /// The board's original saved state for "save colours to the devices" (ADR 0001 #42): the own lighting captured
    /// in a non-direct state before CaseControl's first write of the session (the readback at open, or the
    /// remembered own lighting of a marked board), if it passes [`protocol::validate_backup`]. A save or a restore
    /// does not change it.
    backup: Option<ConfigPacket>,
    /// The only base a flash write builds on (ADR 0001 #42): the validated backup, then what this session last wrote
    /// to flash. `None` when the board was opened in a state CaseControl or another program left and no validated
    /// own lighting exists: nothing is ever written to its flash then.
    trusted: Option<ConfigPacket>,
    /// The hardware mode `set_mode` last sent, while the board shows it (cleared by direct colours and a release);
    /// what a manual save writes.
    mode_state: Option<ModeState>,
    /// Anything was sent since open (never reset), or the board was opened with the "unreleased" marker (an earlier
    /// instance sent something): a readback no longer tells what the flash holds.
    sent_since_open: bool,
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
        Self::open_with_stored(dev, candidate, None, false, false)
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
    ///   the board and the UI agree; if that write fails, `release()` sends it even when nothing else was sent. With
    ///   `setting_pending` (the engine drives the board right after open, ADR 0001 #42) the write at open is skipped,
    ///   so the board does not flash the old look before the setting arrives; `release()` still restores `stored`.
    ///   Without a usable `stored`, `release()` restores the readback and
    ///   [`shows_own_lighting`](cc_core::Controller::shows_own_lighting) stays `false`.
    /// - Not `unreleased` and the readback is per-LED: another program drives the board. Nothing is stored or
    ///   restored, and the release of an untouched controller sends nothing (warning).
    /// - Not `unreleased` and the readback is not per-LED: it is what `release()` restores, and it is reported as the
    ///   new own lighting to remember.
    ///
    /// The own lighting found this way (not the readback of a marked board) is also the backup of the board's
    /// original saved state ([`Controller::saved_state_backup`](cc_core::Controller::saved_state_backup)) when it
    /// passes [`protocol::validate_backup`].
    pub fn open_with_stored(
        mut dev: Box<dyn HidTransport>,
        candidate: &Candidate,
        stored: Option<&[u8]>,
        unreleased: bool,
        setting_pending: bool,
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
            let good = stored.and_then(protocol::validate_backup);
            if good.is_none() && stored.is_some() {
                tracing::warn!(
                    target: "cc_drivers::msi",
                    id = %candidate.id,
                    "stored own lighting is not a valid saved configuration; ignoring it"
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
        let readback_is_own = !read.has_direct_effect();
        let own = OwnLighting::at_open(read, readback_is_own, good, unreleased);
        if own.source() == OwnSource::LeftByUs {
            tracing::warn!(
                target: "cc_drivers::msi",
                id = %candidate.id,
                "board shows a state CaseControl left and no own lighting is stored for it; release() restores that state"
            );
        }
        let restore = own.source() == OwnSource::Stored;
        let backup = own
            .own()
            .and_then(|p| protocol::validate_backup(&p.with_save(false)));

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
        let mut c = Self::assemble(dev, info, board, board_id, firmware, own);
        c.trusted = backup.clone();
        c.backup = backup;
        // A marked board shows what an earlier instance sent (volatile): its readback says nothing about the flash,
        // as if this instance had sent it.
        c.sent_since_open = unreleased;
        if restore && setting_pending {
            tracing::info!(
                target: "cc_drivers::msi",
                id = %c.desc.info.id,
                "board shows a state CaseControl left and a setting is about to drive it; release() restores the stored own lighting"
            );
        } else if restore {
            match c.hand_back() {
                Ok(()) => {
                    c.own.handed_back();
                    tracing::info!(
                        target: "cc_drivers::msi",
                        id = %c.desc.info.id,
                        "board showed a state CaseControl left; restored the last good own lighting stored for it"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "cc_drivers::msi",
                        id = %c.desc.info.id,
                        error = %e,
                        "restoring the stored own lighting at open failed; release() will try again"
                    );
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
        own: OwnLighting<ConfigPacket>,
    ) -> Self {
        let original = own.handback();
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
            own,
            backup: None,
            trusted: None,
            mode_state: None,
            sent_since_open: false,
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
        self.own.handback()
    }

    /// The base the next flash write builds on (see [`Controller::persist_look`]), if the board has one.
    pub fn trusted_base(&self) -> Option<&ConfigPacket> {
        self.trusted.as_ref()
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
        self.sent_since_open = true;
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

    /// `Update(false)` outside direct mode: read the board's current configuration, send it back, then send `next`
    /// ("Windows Mystic Light sends two consecutive reports"), both with the save byte 0. Flash writes go through
    /// [`write_flash`](Self::write_flash) only.
    fn update_hardware(&mut self, next: &ConfigPacket) -> Result<(), DeviceError> {
        let current = read_config(self.dev.as_mut())?;
        self.send(&current.with_save(false))?;
        self.send(&next.with_save(false))
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
        let original = self.own.handback().with_save(false);
        self.send(&original)
    }

    /// The firmware and class gates of a flash write, as at open: the firmware version must not be below the
    /// reference's minimum for this board.
    fn check_flash_gates(&self) -> Result<(), DeviceError> {
        if boards::firmware_version_invalid(self.firmware.aprom, boards::board_config(self.board_id)) {
            return Err(DeviceError::Refused(format!(
                "controller firmware {} is below the minimum supported version; not writing flash",
                self.firmware.display()
            )));
        }
        Ok(())
    }

    /// The packet a save of `look` writes, the trusted base it is built on, and the zones it changes. Built on the
    /// trusted base (never on the working configuration or a direct-mode readback) with the same `apply_mode` /
    /// `apply_zone_color` calls a hardware-mode update uses; zones the look leaves out and every LED count byte keep
    /// the base's bytes. The save byte is 0. Refused without a trusted base.
    fn persist_packet(
        &self,
        look: &PersistLook,
    ) -> Result<(ConfigPacket, ConfigPacket, Vec<MsiZone>), DeviceError> {
        let base = self.trusted.clone().ok_or_else(|| {
            DeviceError::Refused(
                "no trusted configuration to build on (the board was found in a state CaseControl or another program left)"
                    .into(),
            )
        })?;
        let layout = self.board.layout;
        let mut next = base.clone();
        let mut changed = Vec::new();
        let zones = self.board.config.zones.iter().zip(&self.desc.zones);
        let off = |next: &mut ConfigPacket, changed: &mut Vec<MsiZone>| {
            // Every zone, also headers sized 0 (they may still have LEDs), as the off output of set_mode.
            for msi in self.board.config.zones {
                next.apply_mode(&layout, *msi, mode::STATIC, 0, protocol::brightness::OFF, false);
                next.apply_zone_color(&layout, *msi, Rgb::BLACK, Rgb::BLACK);
                changed.push(*msi);
            }
        };
        match look {
            PersistLook::Off => off(&mut next, &mut changed),
            PersistLook::Zones(colors) => {
                if colors.len() != self.desc.zones.len() {
                    return Err(DeviceError::InvalidArgument(format!(
                        "{} zone colours for {} zones",
                        colors.len(),
                        self.desc.zones.len()
                    )));
                }
                for ((msi, zone), color) in zones.zip(colors) {
                    let Some(color) = color else { continue };
                    if zone.len == 0 {
                        continue;
                    }
                    // One colour per header as Static at full level: brightness is baked into the colour.
                    next.apply_mode(
                        &layout,
                        *msi,
                        mode::STATIC,
                        0,
                        protocol::brightness::LEVEL_100,
                        false,
                    );
                    next.apply_zone_color(&layout, *msi, *color, *color);
                    changed.push(*msi);
                }
            }
            PersistLook::Mode(state) => {
                self.desc
                    .validate_state(state)
                    .map_err(|e| DeviceError::InvalidArgument(e.to_string()))?;
                if self.desc.direct_mode == Some(state.mode) {
                    return Err(DeviceError::InvalidArgument(
                        "the direct mode cannot be saved; save its colours per zone".into(),
                    ));
                }
                let m = self
                    .desc
                    .mode(state.mode)
                    .ok_or_else(|| DeviceError::InvalidArgument(format!("unknown mode {}", state.mode)))?;
                let random = m.color_mode == ColorMode::Random;
                let default_speed = if m.caps.contains(ModeCaps::SPEED) {
                    protocol::speed::MEDIUM
                } else {
                    0
                };
                let effect =
                    u8::try_from(state.mode).map_err(|_| DeviceError::InvalidArgument("mode id".into()))?;
                let speed = state
                    .speed
                    .and_then(|s| u8::try_from(s).ok())
                    .unwrap_or(default_speed);
                let brightness = state
                    .brightness
                    .and_then(|b| u8::try_from(b).ok())
                    .unwrap_or(protocol::brightness::LEVEL_100);
                if state.mode == u32::from(mode::STATIC) && brightness == protocol::brightness::OFF {
                    off(&mut next, &mut changed);
                } else {
                    // Exactly the bytes set_mode writes: each zone with LEDs gets the mode and its first LED's colour
                    // (the mode's colours for per-LED colour modes, else the colour buffer, as set_mode uses it).
                    for (msi, zone) in zones {
                        if zone.len == 0 {
                            continue;
                        }
                        let color = if m.color_mode == ColorMode::PerLed {
                            state.colors.get(zone.start).copied().unwrap_or(Rgb::BLACK)
                        } else {
                            self.colors.get(zone.start).copied().unwrap_or(Rgb::BLACK)
                        };
                        next.apply_mode(&layout, *msi, effect, speed, brightness, random);
                        next.apply_zone_color(&layout, *msi, color, color);
                        changed.push(*msi);
                    }
                }
            }
        }
        if changed.is_empty() {
            return Err(DeviceError::InvalidArgument("no zone to save".into()));
        }
        next.0[protocol::SAVE_BYTE] = 0;
        Ok((next, base, changed))
    }

    /// Every gate a packet must pass before it reaches flash (research §1.4), as [`DeviceError::Refused`]: 185 bytes
    /// with report id 0x52 (by construction of [`ConfigPacket`], checked again), every zone record of the whole packet
    /// savable ([`protocol::check_saved_records`]), LED counts within the maxima, and every byte outside the saved
    /// zones' records and the on-board sync/mirror bits equal to the trusted `base` (LED counts included).
    fn check_persist_packet(
        &self,
        next: &ConfigPacket,
        base: &ConfigPacket,
        changed: &[MsiZone],
    ) -> Result<(), DeviceError> {
        let refuse = |why: String| DeviceError::Refused(format!("save refused: {why}"));
        if next.0.len() != CONFIG_LEN || next.0[0] != CONFIG_REPORT_ID {
            return Err(refuse("not a 185-byte 0x52 report".into()));
        }
        protocol::check_saved_records(next).map_err(refuse)?;
        protocol::check_led_counts(next).map_err(refuse)?;
        protocol::check_save_diff(base, next, &self.board.layout, changed).map_err(refuse)?;
        Ok(())
    }

    /// Writes `next` (gated, save byte 0) to flash: leaves direct mode volatile by sending `next` with save 0 and
    /// checks the readback equals it (else [`DeviceError::Refused`]: nothing reached flash), then the reference's
    /// two-report save (the readback, then `next`, both with save 1), then checks the readback again (a mismatch is
    /// a protocol error: the flash may hold anything). The caller records the result
    /// ([`after_flash`](Self::after_flash)) and takes direct mode again.
    fn write_flash(&mut self, next: &ConfigPacket) -> Result<(), DeviceError> {
        let same = |a: &ConfigPacket, b: &ConfigPacket| a.with_save(false) == b.with_save(false);
        self.direct = false;
        self.direct_configured = false;
        self.mode_state = None;
        self.sync = protocol::sync_direct_mode(next);
        self.send(&next.with_save(false))?;
        let shown = read_config(self.dev.as_mut())?;
        if !same(&shown, next) {
            return Err(DeviceError::Refused(
                "the board does not show the packet to save; not writing flash".into(),
            ));
        }
        protocol::check_saved_records(&shown)
            .map_err(|e| DeviceError::Refused(format!("save refused: {e}")))?;
        self.send(&shown.with_save(true))?;
        self.send(&next.with_save(true))?;
        let stored = read_config(self.dev.as_mut())?;
        if !same(&stored, next) {
            return Err(DeviceError::Protocol(
                "the board reads back something else after the save".into(),
            ));
        }
        tracing::info!(
            target: "cc_drivers::msi",
            id = %self.desc.info.id,
            packet = %cc_hid::hex(&next.with_save(false)),
            "configuration saved to flash"
        );
        Ok(())
    }

    /// After a verified flash write of `written`: it is the next trusted base, what `release()` hands back and the own
    /// lighting; the working configuration is `written` with the configured header sizes (volatile updates keep
    /// sending those; flash keeps the base's counts).
    fn after_flash(&mut self, written: &ConfigPacket) {
        let written = ConfigPacket(written.with_save(false));
        self.trusted = Some(written.clone());
        self.own.saved(written.clone());
        let mut data = written;
        for (z, zone) in self.board.config.zones.iter().zip(&self.desc.zones) {
            data.set_led_count(*z, u8::try_from(zone.len).unwrap_or(u8::MAX));
        }
        self.data = data;
    }

    /// The configuration a restore writes (see [`Controller::restore_saved_state`]): a fresh readback outside
    /// per-LED mode with the backed-up records of `backup` replaced, gated like a save, and whether it was written
    /// (not when `may_skip` and the readback already holds the backup). When anything was sent this session (direct
    /// mode, or a hardware mode with the configured header sizes) or the board was opened with the "unreleased" marker
    /// (it shows what an earlier instance sent), the board is first sent the backup with the LED counts of the trusted
    /// base (volatile), so the readback carries the board's stored counts, never the configured sizes.
    fn restore_target(
        &mut self,
        backup: &ConfigPacket,
        may_skip: bool,
    ) -> Result<(ConfigPacket, bool), DeviceError> {
        if self.direct || self.sent_since_open {
            let mut leave = backup.clone();
            if let Some(base) = &self.trusted {
                for z in [MsiZone::JRainbow1, MsiZone::JRainbow2, MsiZone::JRainbow3] {
                    if let Some(n) = base.led_count(z) {
                        leave.set_led_count(z, n);
                    }
                }
            }
            self.mode_state = None;
            self.direct = false;
            self.direct_configured = false;
            self.send(&leave.with_save(false))?;
        }
        let readback = read_config(self.dev.as_mut())?;
        let refuse = |why: String| DeviceError::Refused(format!("restore refused: {why}"));
        if readback.has_direct_effect() {
            return Err(refuse("the board is in per-LED mode".into()));
        }
        let layout = self.board.layout;
        let zones = self.board.config.zones;
        let target = protocol::replace_records(&readback, backup, &layout, zones);
        protocol::check_saved_records(&target).map_err(refuse)?;
        protocol::check_led_counts(&target).map_err(refuse)?;
        protocol::check_save_diff(&readback, &target, &layout, zones).map_err(refuse)?;
        if may_skip && target.with_save(false) == readback.with_save(false) {
            return Ok((target, false));
        }
        self.write_flash(&target)?;
        Ok((target, true))
    }

    /// After a flash write: takes direct mode again and re-sends the last frame when the board was driven with
    /// direct colours (`colors` and the per-LED buffers still hold it). A failure is logged; the next frame retries.
    fn resume_direct(&mut self, was_direct: bool) {
        if !was_direct {
            return;
        }
        let r = self.enter_direct().and_then(|()| self.update_direct());
        if let Err(e) = r {
            tracing::warn!(target: "cc_drivers::msi", error = %e, "re-entering direct mode after the flash write failed");
            self.direct = false;
            self.direct_configured = false;
        }
    }

    /// After a flash write of `written` (or a failed one) outside direct mode: the board shows `written`, with the
    /// trusted base's LED counts, not what it showed before. `shown` (the working configuration with the configured
    /// header sizes, and the hardware mode it is) becomes the working configuration again and is sent as a
    /// hardware-mode update (volatile), so only the flash changed. A failure is logged; the next output sends the
    /// working configuration again.
    fn resume_hardware(
        &mut self,
        written: &ConfigPacket,
        (data, mode_state): (ConfigPacket, Option<ModeState>),
    ) {
        self.data = data;
        self.sync = protocol::sync_direct_mode(&self.data);
        if self.data.with_save(false) == written.with_save(false) {
            self.mode_state = mode_state;
            return;
        }
        let next = self.data.clone();
        match self.update_hardware(&next) {
            Ok(()) => self.mode_state = mode_state,
            Err(e) => {
                tracing::warn!(target: "cc_drivers::msi", error = %e, "showing the output again after the flash write failed");
            }
        }
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
        self.mode_state = None;
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
        self.mode_state = None;
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
        self.mode_state = None;
        self.update_hardware(&next)?;
        self.mode_state = Some(state.clone());
        Ok(())
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

    /// The hardware mode `set_mode` last sent, written to flash through every gate of
    /// [`persist_look`](Controller::persist_look). Refused ([`DeviceError::Unsupported`]) in direct mode (the
    /// reference's `Update(true)` only sends a per-LED frame there) and when no hardware mode was set, so neither the
    /// enable message nor a readback reaches flash.
    fn save_to_hardware(&mut self) -> Result<(), DeviceError> {
        if self.direct {
            return Err(DeviceError::Unsupported);
        }
        let Some(state) = self.mode_state.clone() else {
            return Err(DeviceError::Unsupported);
        };
        self.persist_look(&PersistLook::Mode(state), None).map(|_| ())
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        if !self.touched && !self.own.handback_pending() {
            return Ok(());
        }
        self.hand_back()?;
        self.touched = false;
        self.mode_state = None;
        self.own.handed_back();
        Ok(())
    }

    /// The mode in the configuration `release()` restores (read at open, the remembered own lighting, or what was
    /// written to flash).
    fn hardware_mode(&self) -> Option<ModeState> {
        hardware_state(self.own.handback(), &self.board, &self.desc)
    }

    /// The hand-back configuration (save byte 0) unless it is a state CaseControl or another program left. When it is
    /// the remembered own lighting restored at open, these are the bytes already stored.
    fn own_lighting(&self) -> Option<Vec<u8>> {
        self.own.own().and_then(ConfigPacket::own_lighting)
    }

    /// `false` while the hand-back configuration is a state CaseControl left, or while the restore of the
    /// remembered own lighting is still pending.
    fn shows_own_lighting(&self) -> bool {
        self.own.shows_own_lighting()
    }

    /// Static at brightness 0: a hardware state, so brightness 0 does not switch the board to per-LED mode (whose
    /// enable message outlives the app if it ends without a release; ADR 0001 #38).
    fn off_mode(&self) -> Option<u32> {
        Some(u32::from(mode::STATIC))
    }

    /// One colour per header (Static at full level) and every hardware mode.
    fn persist_caps(&self) -> Option<PersistCaps> {
        Some(PersistCaps {
            zone_colors: true,
            hardware_modes: true,
        })
    }

    /// Writes the configuration packet for `look` to flash (ADR 0001 #42, research §1.4). The packet is the trusted
    /// base (the validated backup, or what this session last wrote to flash; never the working configuration or a
    /// direct-mode readback) with the looked-for zones set exactly as a hardware-mode update sets them: a zone colour
    /// as Static at level 10, a mode as `set_mode` writes it, off as Static at brightness 0; LED counts stay the
    /// base's. Every gate refusal is [`DeviceError::Refused`]. When the board is not in direct mode, a fresh readback
    /// must equal the working configuration first (else another program changed it). The fingerprint is the packet.
    /// After the write `release()` hands back the saved packet; direct mode is taken again with the last frame, or
    /// outside direct mode the working configuration shown before (configured header sizes, not the flash's counts)
    /// is sent again as a hardware-mode update; both also when the write failed.
    fn persist_look(
        &mut self,
        look: &PersistLook,
        last: Option<&[u8]>,
    ) -> Result<PersistOutcome, DeviceError> {
        self.check_flash_gates()?;
        let (next, base, changed) = self.persist_packet(look)?;
        self.check_persist_packet(&next, &base, &changed)?;
        let fingerprint = next.with_save(false).to_vec();
        if last == Some(fingerprint.as_slice()) {
            return Ok(PersistOutcome::Unchanged);
        }
        let was_direct = self.direct;
        if !was_direct {
            let shown = read_config(self.dev.as_mut())?;
            if shown.with_save(false) != self.data.with_save(false) {
                return Err(DeviceError::Refused(
                    "the board shows a configuration CaseControl did not send; not writing flash".into(),
                ));
            }
        }
        let shown = (self.data.clone(), self.mode_state.clone());
        let written = self.write_flash(&next);
        if written.is_ok() {
            self.after_flash(&next);
        }
        if was_direct {
            self.resume_direct(true);
        } else {
            self.resume_hardware(&next, shown);
        }
        written.map(|()| PersistOutcome::Written { fingerprint })
    }

    /// The own lighting captured before CaseControl's first write of the session (or what a restore wrote back), if
    /// it passed [`protocol::validate_backup`] at open.
    fn saved_state_backup(&self) -> Option<Vec<u8>> {
        self.backup.as_ref().map(|p| p.with_save(false).to_vec())
    }

    /// Validates `backup` ([`protocol::validate_backup`]), then writes back the board's configuration with the
    /// backed-up records replaced: when the board is driven in direct mode it first leaves it with the backup
    /// (volatile), then a fresh readback outside per-LED mode gets the backup's zone records (LED counts stay the
    /// readback's), is gated like a save ([`protocol::check_save_diff`] against the readback) and written with the same
    /// sequence. When nothing was sent since open, the board was not opened with the "unreleased" marker and the
    /// readback already holds the backup, nothing is written ([`PersistOutcome::Unchanged`]). Afterwards the written configuration is what `release()` hands back and the
    /// next trusted base; the backup stays. Direct mode is taken again with the last frame, also after a failure.
    fn restore_saved_state(&mut self, backup: &[u8]) -> Result<PersistOutcome, DeviceError> {
        let packet = protocol::validate_backup(backup).ok_or_else(|| {
            DeviceError::InvalidArgument("the backup is not a valid saved configuration; nothing sent".into())
        })?;
        self.check_flash_gates()?;
        let was_direct = self.direct;
        let may_skip = !was_direct && !self.sent_since_open;
        match self.restore_target(&packet, may_skip) {
            Ok((target, false)) => {
                tracing::info!(
                    target: "cc_drivers::msi",
                    id = %self.desc.info.id,
                    "the board already holds its original saved configuration; nothing written"
                );
                self.trusted = Some(target.clone());
                self.own.saved(target);
                Ok(PersistOutcome::Unchanged)
            }
            Ok((target, true)) => {
                self.after_flash(&target);
                self.resume_direct(was_direct);
                Ok(PersistOutcome::Written {
                    fingerprint: target.with_save(false).to_vec(),
                })
            }
            Err(e) => {
                self.resume_direct(was_direct);
                Err(e)
            }
        }
    }
}
