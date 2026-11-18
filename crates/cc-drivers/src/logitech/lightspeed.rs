// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! Logitech Lightspeed wireless devices (PRO Wireless) behind the Lightspeed receiver `046D:C539`.
//!
//! Ported from `DetectLogitechLightspeedReceiver` / `getWirelessDevice` / `logitech_device` in
//! `LogitechProtocolCommon.*` and `RGBController_LogitechLightspeed` (the legacy controller that owns wireless PID
//! 0x4079 upstream, see `hidpp20_legacy_wireless_pids`). The receiver exposes a short-report collection (usage 1,
//! HID++ 1.0 register access to the receiver) and a long-report collection (usage 2, HID++ 2.0 feature access to the
//! paired device at its device index 1..6). The receiver accepts one connection.
//!
//! Wire behaviour kept from the reference: detection reads register 0x00 and enables wireless notifications only if
//! they are off, reads register 0x02 (connected count) and writes 0x02 to it ("fake a reconnect") so the receiver
//! lists its paired devices as connection notifications; the paired device is then asked whether it is connected
//! (register 0x02 read at its index), for its name and type (0x0005) and its 0x8070 zones and effects. Direct claims
//! with `SetSWControl(1, 1)` and paints the Static ("ON") effect per zone; every other mode sends
//! `SetSWControl(0, 0)` first and then the effect per zone (`DeviceUpdateMode`).

use std::thread;
use std::time::Duration;

use cc_core::{
    Bus, ColorMode, Controller, DeviceDescription, DeviceError, DeviceId, DeviceInfo, DeviceKind, Led, Mode,
    ModeCaps, ModeState, Rgb, Zone, ZoneKind,
};
use cc_hid::{HidError, HidTransport};

use super::hidpp::{
    self, DEVICE_CONNECTION, ERR_RESOURCE, EffectInfo, GET_REGISTER, Link, PAGE_COLOR_LED_EFFECTS,
    PAGE_DEVICE_NAME_TYPE, RECEIVER_INDEX, REG_CONNECTION_STATE, REG_NOTIFICATIONS, REPLY_TIMEOUT,
    REPORT_SHORT, SET_REGISTER, ZoneEffectState, ZoneInfo, effect,
};

/// Lightspeed receiver product id.
pub const RECEIVER_PID: u16 = 0xC539;
/// Receiver interface carrying the HID++ collections.
pub const RECEIVER_INTERFACE: i32 = 2;
/// Receiver vendor usage page.
pub const RECEIVER_USAGE_PAGE: u16 = 0xFF00;
/// Usage of the short-report (0x10) collection.
pub const USAGE_SHORT: u16 = 1;
/// Usage of the long-report (0x11) collection.
pub const USAGE_LONG: u16 = 2;
/// Wireless (virtual) PID of the PRO Wireless mouse.
pub const PRO_WIRELESS_WPID: u16 = 0x4079;

/// Mode id of Off.
pub const OFF_MODE: u32 = 0xFF;
/// Mode id of Direct.
pub const DIRECT_MODE: u32 = 0x100;

/// Brightness range (`LOGITECH_G_PRO_WIRELESS_BRIGHTNESS_MIN..=MAX`).
pub const BRIGHTNESS_MIN: u32 = 0x01;
/// See [`BRIGHTNESS_MIN`].
pub const BRIGHTNESS_MAX: u32 = 0x64;
/// Raw speed (period / 100 ms) of the slowest setting (`LOGITECH_G_PRO_WIRELESS_SPEED_SLOWEST`).
pub const RAW_SPEED_SLOWEST: u16 = 0xC8;
/// Raw speed of the default setting (`LOGITECH_G_PRO_WIRELESS_SPEED_NORMAL`).
pub const RAW_SPEED_NORMAL: u16 = 0x32;
/// Raw speed of the fastest setting (`LOGITECH_G_PRO_WIRELESS_SPEED_FASTEST`).
pub const RAW_SPEED_FASTEST: u16 = 0x0A;

/// Detection attempts per device (`LOGITECH_LIGHTSPEED_DETECT_MAX_RETRY`).
pub const DETECT_MAX_RETRY: usize = 10;
/// Delay before each detection attempt (`CreateLogitechLightspeedDevice` sleeps 50 ms).
pub const DETECT_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Wireless PIDs the legacy Lightspeed controller owns upstream, with their names (`hidpp20_legacy_wireless_pids`).
pub const LEGACY_WPIDS: &[(u16, &str)] = &[
    (0x4053, "Logitech G900 Wireless Gaming Mouse"),
    (0x405D, "Logitech G403 Wireless Gaming Mouse"),
    (0x405F, "Logitech Powerplay Mat"),
    (0x4067, "Logitech G903 Wireless Gaming Mouse"),
    (0x4070, "Logitech G703 Wireless Gaming Mouse"),
    (PRO_WIRELESS_WPID, "Logitech G Pro Wireless Gaming Mouse"),
    (0x407F, "Logitech G502 Wireless Gaming Mouse"),
    (0x4086, "Logitech G703 HERO Wireless Gaming Mouse"),
    (0x4087, "Logitech G903 HERO Wireless Gaming Mouse"),
];

/// The legacy name of a wireless PID this driver handles, `None` for PIDs it leaves alone.
pub fn legacy_name(wpid: u16) -> Option<&'static str> {
    LEGACY_WPIDS.iter().find(|(p, _)| *p == wpid).map(|(_, n)| *n)
}

/// `logitech_led_locations`.
pub const LED_LOCATIONS: &[&str] = &[
    "Unknown",
    "Primary",
    "Logo",
    "Left",
    "Right",
    "Combined",
    "Group One",
    "Group Two",
    "Group Three",
    "Group Four",
    "Group Five",
    "Group Six",
];

/// Zone and LED name for zone `i` at `location` (`SetupZones`): `"<location> Zone"` / `"<location> LED"`, or
/// `"Zone <i>"` / `"LED <i>"` for unknown locations.
pub fn zone_names(i: usize, location: u16) -> (String, String) {
    match LED_LOCATIONS.get(usize::from(location)) {
        Some(loc) => (format!("{loc} Zone"), format!("{loc} LED")),
        None => (format!("Zone {i}"), format!("LED {i}")),
    }
}

/// Maps the Logitech device type byte as the legacy controller does.
pub fn device_kind(device_type: u8) -> DeviceKind {
    match device_type {
        0 => DeviceKind::Keyboard,
        3 => DeviceKind::Mouse,
        4 => DeviceKind::Mousemat,
        8 => DeviceKind::Headset,
        _ => DeviceKind::Unknown,
    }
}

/// UI speed range: higher is faster. `speed` maps to the reference's raw value as `raw = 210 - speed`, so
/// 10 is the slowest raw 200 (20 s) and 200 the fastest raw 10 (1 s).
pub const SPEED_MIN: u32 = 10;
/// See [`SPEED_MIN`].
pub const SPEED_MAX: u32 = 200;

/// UI speed to the reference's raw speed (period in units of 100 ms).
pub fn raw_speed(speed: u32) -> u16 {
    let s = speed.clamp(SPEED_MIN, SPEED_MAX);
    (SPEED_MIN + SPEED_MAX - s) as u16
}

/// Default UI speed (raw `LOGITECH_G_PRO_WIRELESS_SPEED_NORMAL`).
pub const DEFAULT_SPEED: u32 = SPEED_MIN + SPEED_MAX - RAW_SPEED_NORMAL as u32;

/// The mode the device shows on its own, from each zone's open-time readback: the first zone whose readback decodes
/// names the mode (its effect index, or "Off"), speed (from the raw period, the inverse of [`raw_speed`]) and
/// brightness; per-LED colours come from every zone (black where a zone's readback has none). `None` when no zone
/// decodes or the result is not a valid state of `desc`.
pub fn hardware_state(
    desc: &DeviceDescription,
    zones: &[ZoneInfo],
    initial: &[ZoneEffectState],
) -> Option<ModeState> {
    let decoded: Vec<Option<hidpp::DecodedEffect>> = zones
        .iter()
        .map(|z| {
            initial
                .iter()
                .find(|s| s.zone == z.index)
                .and_then(|s| hidpp::decode_effect(s, z))
        })
        .collect();
    let first = decoded.iter().flatten().next()?;
    let id = if first.effect.id == effect::OFF {
        OFF_MODE
    } else {
        u32::from(first.effect.index)
    };
    let mode = desc.mode(id).filter(|_| desc.direct_mode != Some(id))?;
    let has = |c| mode.caps.contains(c);
    let speed = first.period.filter(|_| has(ModeCaps::SPEED)).map(|p| {
        (SPEED_MIN + SPEED_MAX)
            .saturating_sub(u32::from(p / 100))
            .clamp(SPEED_MIN, SPEED_MAX)
    });
    let brightness = first
        .brightness
        .filter(|_| has(ModeCaps::BRIGHTNESS))
        .map(|b| u32::from(b).clamp(BRIGHTNESS_MIN, BRIGHTNESS_MAX));
    let colors = match mode.color_mode {
        ColorMode::PerLed => decoded
            .iter()
            .map(|d| d.and_then(|d| d.color).unwrap_or(Rgb::BLACK))
            .collect(),
        _ => Vec::new(),
    };
    let state = ModeState {
        mode: id,
        speed,
        brightness,
        direction: None,
        colors,
    };
    desc.validate_state(&state).is_ok().then_some(state)
}

/// The 16 `setMode` parameter bytes for feature 0x8070 (`logitech_device::setMode`, fp8070 branch): zone, effect
/// index, R, G, B, then per effect type: Spectrum period (raw * 100, BE) at 7..9 and brightness at 9; Breathing
/// period at 5..7 and brightness at 8. Byte 12 (persistence) is 0.
pub fn effect_params(
    zone: u8,
    effect_index: u8,
    effect_id: u16,
    color: Rgb,
    raw_speed: u16,
    brightness: u8,
) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[0] = zone;
    d[1] = effect_index;
    d[2..5].copy_from_slice(&[color.r, color.g, color.b]);
    let [ph, pl] = raw_speed.wrapping_mul(100).to_be_bytes();
    match effect_id {
        effect::CYCLE => {
            d[7] = ph;
            d[8] = pl;
            d[9] = brightness;
        }
        effect::BREATHING => {
            d[5] = ph;
            d[6] = pl;
            d[8] = brightness;
        }
        _ => {}
    }
    d
}

/// One device the receiver reported as paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paired {
    /// Device index (1..=6) on the receiver.
    pub index: u8,
    /// Wireless PID.
    pub wpid: u16,
    /// Link flag of the notification: false when paired but not linked (off / asleep).
    pub online: bool,
}

/// Parses a device connection notification `10 <idx> 41 <protocol> <flags> <wpid lo> <wpid hi>`.
/// Bit 0x40 of `flags` means "link not established".
pub fn parse_connection_notification(frame: &[u8]) -> Option<Paired> {
    if frame.len() < 7
        || frame[0] != REPORT_SHORT
        || frame[2] != DEVICE_CONNECTION
        || !(1..=6).contains(&frame[1])
    {
        return None;
    }
    Some(Paired {
        index: frame[1],
        wpid: u16::from_le_bytes([frame[5], frame[6]]),
        online: frame[4] & 0x40 == 0,
    })
}

/// `getWirelessDevice`: asks the receiver (short-report collection) which devices are paired.
///
/// Sends, in order: `10 FF 81 00 00 00 00` (read notification flags); only if wireless notifications are off,
/// `10 FF 80 00 00 01 00` (enable them); `10 FF 81 02 00 00 00` (connected count); if the count is non-zero,
/// `10 FF 80 02 02 00 00` (fake a reconnect) and then collects that many connection notifications.
pub fn scan_receiver(short: &mut dyn HidTransport) -> Result<Vec<Paired>, DeviceError> {
    let flags = hidpp::register_request(short, RECEIVER_INDEX, GET_REGISTER, REG_NOTIFICATIONS, [0; 3])?
        .map_err(hidpp::hidpp_error)?;
    // Wireless notifications off: enable them. A rejection is only logged, as in the reference.
    if flags[1] & 0x01 == 0
        && let Err(code) = hidpp::register_request(
            short,
            RECEIVER_INDEX,
            SET_REGISTER,
            REG_NOTIFICATIONS,
            [0x00, 0x01, 0x00],
        )?
    {
        tracing::warn!(target: "cc_drivers::logitech", code, "receiver rejected enabling wireless notifications");
    }
    let state = hidpp::register_request(short, RECEIVER_INDEX, GET_REGISTER, REG_CONNECTION_STATE, [0; 3])?
        .map_err(hidpp::hidpp_error)?;
    let count = usize::from(state[1]).min(6);
    tracing::debug!(target: "cc_drivers::logitech", count, "receiver connected-device count");
    if count == 0 {
        return Ok(Vec::new());
    }
    short.write(&hidpp::short_register(
        RECEIVER_INDEX,
        SET_REGISTER,
        REG_CONNECTION_STATE,
        [0x02, 0x00, 0x00],
    ))?;
    let mut found: Vec<Paired> = Vec::with_capacity(count);
    let mut buf = [0u8; 64];
    // The reference reads count + 1 reports (the notifications plus the receiver's own ack); stray traffic gets a
    // bounded extra budget.
    for _ in 0..count + 1 + hidpp::FOREIGN_BUDGET {
        let n = match short.read_timeout(&mut buf, REPLY_TIMEOUT) {
            Ok(n) => n,
            Err(HidError::Timeout) => break,
            Err(e) => return Err(e.into()),
        };
        if let Some(p) = parse_connection_notification(&buf[..n.min(buf.len())]) {
            if !found.iter().any(|f| f.index == p.index) {
                found.push(p);
            }
            if found.len() >= count {
                break;
            }
        }
    }
    Ok(found)
}

/// `logitech_device::connected`: reads register 0x02 at the device's index through the receiver. The receiver
/// answers error 0x09 (resource error) for a paired device that is not linked. `None` when nothing answered.
pub fn is_connected(short: &mut dyn HidTransport, index: u8) -> Result<Option<bool>, DeviceError> {
    match hidpp::register_request(short, index, GET_REGISTER, REG_CONNECTION_STATE, [0; 3]) {
        Ok(Ok(p)) => Ok(Some(p[1] != ERR_RESOURCE)),
        Ok(Err(code)) => Ok(Some(code != ERR_RESOURCE)),
        Err(DeviceError::Timeout) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Everything detection learned from a paired device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightspeedProbe {
    /// Name reported by feature 0x0005 (empty if none).
    pub name: String,
    /// Logitech device type byte.
    pub device_type: u8,
    /// Runtime index of feature 0x8070.
    pub feature: u8,
    /// Zones ("LEDs" in the legacy controller) with their effects.
    pub zones: Vec<ZoneInfo>,
    /// What each zone showed before anything was claimed (0x8070 GetEffect, read-only).
    pub initial: Vec<ZoneEffectState>,
}

impl LightspeedProbe {
    /// `logitech_device::is_valid`: a plausible name (longer than 3, printable first char), a known device type
    /// (<= 8), at least one zone, a non-zero RGB feature index.
    pub fn is_valid(&self) -> bool {
        let first_ok = self.name.bytes().next().is_some_and(|c| (32..122).contains(&c));
        first_ok && self.name.len() > 3 && self.device_type <= 8 && !self.zones.is_empty() && self.feature > 0
    }
}

/// `initialiseDevice` on the long-report link: name and type (0x0005), 0x8070 lookup, zones and effects, then a
/// read-only readback of each zone's live effect.
pub fn probe_device(link: &mut Link) -> Result<LightspeedProbe, DeviceError> {
    let name_feature = link.feature_index(PAGE_DEVICE_NAME_TYPE)?;
    let (name, device_type) = if name_feature == 0 {
        (String::new(), u8::MAX)
    } else {
        let name = link.device_name(name_feature)?.unwrap_or_default();
        (name, link.device_type(name_feature)?)
    };
    let feature = link.feature_index(PAGE_COLOR_LED_EFFECTS)?;
    if feature == 0 {
        // The reference falls back to 0x8071; the PRO Wireless has 0x8070 and 0x8071 is not ported.
        return Err(DeviceError::Unsupported);
    }
    let zones = link.discover_8070(feature)?;
    let initial = link.read_effect_state(feature, &zones)?;
    Ok(LightspeedProbe {
        name,
        device_type,
        feature,
        zones,
        initial,
    })
}

/// `CreateLogitechLightspeedDevice`: up to [`DETECT_MAX_RETRY`] attempts, each after [`DETECT_RETRY_DELAY`] and a
/// flush of both collections, until the device answers with a valid identity. `Ok(None)` when it never did.
/// Timeouts and transport errors end the attempts at once.
pub fn probe_with_retry(
    short: &mut dyn HidTransport,
    link: &mut Link,
) -> Result<Option<LightspeedProbe>, DeviceError> {
    for attempt in 0..DETECT_MAX_RETRY {
        thread::sleep(DETECT_RETRY_DELAY);
        hidpp::flush(short, hidpp::FLUSH_LIMIT)?;
        link.flush()?;
        let p = probe_device(link)?;
        if p.is_valid() {
            tracing::debug!(target: "cc_drivers::logitech", attempt, "lightspeed device valid");
            return Ok(Some(p));
        }
        tracing::debug!(target: "cc_drivers::logitech", attempt, name = %p.name, "lightspeed device answered with an invalid identity");
    }
    Ok(None)
}

fn mode(id: u32, name: &str, caps: ModeCaps, color_mode: ColorMode) -> Mode {
    Mode {
        id,
        name: name.into(),
        caps,
        color_mode,
        speed: caps.contains(ModeCaps::SPEED).then_some(SPEED_MIN..=SPEED_MAX),
        brightness: caps
            .contains(ModeCaps::BRIGHTNESS)
            .then_some(BRIGHTNESS_MIN..=BRIGHTNESS_MAX),
        colors: None,
    }
}

/// The legacy controller's mode list built from LED 0's effects: Off, then per effect Direct + Static (ON),
/// Spectrum Cycle, Breathing. Returns the modes and whether a Direct mode exists.
pub fn build_modes(led0: &[EffectInfo]) -> (Vec<Mode>, bool) {
    let mut modes = vec![mode(OFF_MODE, "Off", ModeCaps::empty(), ColorMode::None)];
    let mut direct = false;
    for fx in led0 {
        let id = u32::from(fx.index);
        match fx.id {
            effect::STATIC if !direct => {
                direct = true;
                modes.push(mode(
                    DIRECT_MODE,
                    "Direct",
                    ModeCaps::PER_LED_COLOR,
                    ColorMode::PerLed,
                ));
                modes.push(mode(
                    id,
                    "Static",
                    ModeCaps::PER_LED_COLOR | ModeCaps::BRIGHTNESS,
                    ColorMode::PerLed,
                ));
            }
            effect::CYCLE => modes.push(mode(
                id,
                "Spectrum Cycle",
                ModeCaps::SPEED | ModeCaps::BRIGHTNESS,
                ColorMode::None,
            )),
            effect::BREATHING => modes.push(mode(
                id,
                "Breathing",
                ModeCaps::PER_LED_COLOR | ModeCaps::SPEED | ModeCaps::BRIGHTNESS,
                ColorMode::PerLed,
            )),
            _ => {}
        }
    }
    (modes, direct)
}

/// Builds the device description for a probed device.
pub fn describe(probe: &LightspeedProbe, wpid: u16, id: DeviceId, location: String) -> DeviceDescription {
    let mut zones = Vec::with_capacity(probe.zones.len());
    let mut leds = Vec::with_capacity(probe.zones.len());
    for (i, z) in probe.zones.iter().enumerate() {
        let (zone_name, led_name) = zone_names(i, z.location);
        zones.push(Zone {
            name: zone_name,
            kind: ZoneKind::Single,
            start: i,
            len: 1,
            resizable: None,
            brightness_only: false,
        });
        leds.push(Led { name: led_name });
    }
    let led0 = probe.zones.first().map(|z| z.effects.as_slice()).unwrap_or(&[]);
    let (modes, direct) = build_modes(led0);
    let name = if probe.name.is_empty() {
        legacy_name(wpid)
            .unwrap_or("Logitech Wireless Lightspeed Device")
            .to_owned()
    } else {
        probe.name.clone()
    };
    DeviceDescription {
        info: DeviceInfo {
            id,
            name,
            vendor: "Logitech".into(),
            kind: device_kind(probe.device_type),
            bus: Bus::Hid,
            location,
            serial: None,
            firmware: None,
        },
        zones,
        leds,
        modes,
        direct_mode: direct.then_some(DIRECT_MODE),
        keepalive: None,
    }
}

/// Controller for one Lightspeed device behind a receiver. Talks only to the receiver's long-report collection.
#[derive(Debug)]
pub struct LightspeedController {
    desc: DeviceDescription,
    link: Link,
    feature: u8,
    zones: Vec<ZoneInfo>,
    initial: Vec<ZoneEffectState>,
    /// SetSWControl(1, 1) was acknowledged.
    claimed: bool,
    /// A SW-control or effect write was attempted since the last release: release() must run.
    touched: bool,
    sent: Vec<Option<Rgb>>,
}

impl LightspeedController {
    /// Wraps a probed device. Sends nothing.
    pub fn new(link: Link, probe: LightspeedProbe, desc: DeviceDescription) -> Self {
        let n = probe.zones.len();
        Self {
            desc,
            link,
            feature: probe.feature,
            zones: probe.zones,
            initial: probe.initial,
            claimed: false,
            touched: false,
            sent: vec![None; n],
        }
    }

    /// The live effect of each zone as read before anything was claimed.
    pub fn initial_state(&self) -> &[ZoneEffectState] {
        &self.initial
    }

    /// The discovered zones and their effects.
    pub fn zones(&self) -> &[ZoneInfo] {
        &self.zones
    }

    /// Device index on the receiver.
    pub fn device_index(&self) -> u8 {
        self.link.index()
    }

    /// Whether software control is currently claimed.
    pub fn is_claimed(&self) -> bool {
        self.claimed
    }

    fn set_sw(&mut self, on: bool) -> Result<(), DeviceError> {
        // Marked before sending: any SW-control write may have taken effect even if its ack is lost, so release()
        // must run afterwards. `claimed` only records an acknowledged claim (an unacknowledged one is re-sent).
        self.touched = true;
        if !on {
            self.claimed = false;
        }
        self.link.set_sw_control(self.feature, on)?;
        self.claimed = on;
        Ok(())
    }

    fn clear_sent(&mut self) {
        self.sent.iter_mut().for_each(|s| *s = None);
    }

    fn paint_zone(&mut self, i: usize, color: Rgb) -> Result<(), DeviceError> {
        if self.sent[i] == Some(color) {
            return Ok(());
        }
        let zone = &self.zones[i];
        let Some(fx) = zone.effect_by_id(effect::STATIC) else {
            return Ok(());
        };
        self.sent[i] = None;
        let params = effect_params(zone.index, fx.index, effect::STATIC, color, 0, 0);
        self.link.set_zone_effect(self.feature, &params)?;
        self.sent[i] = Some(color);
        Ok(())
    }
}

impl Controller for LightspeedController {
    fn description(&self) -> &DeviceDescription {
        &self.desc
    }

    fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError> {
        if colors.len() != self.zones.len() {
            return Err(DeviceError::InvalidArgument(format!(
                "{} colors for {} LEDs",
                colors.len(),
                self.zones.len()
            )));
        }
        if !self.claimed {
            self.set_sw(true)?;
            self.clear_sent();
        }
        for (i, &c) in colors.iter().enumerate() {
            self.paint_zone(i, c)?;
        }
        Ok(())
    }

    fn set_zone_direct(&mut self, zone: usize, colors: &[Rgb]) -> Result<(), DeviceError> {
        if zone >= self.zones.len() || colors.len() != 1 {
            return Err(DeviceError::InvalidArgument(format!(
                "zone {zone} with {} colors",
                colors.len()
            )));
        }
        if !self.claimed {
            self.set_sw(true)?;
            self.clear_sent();
        }
        self.paint_zone(zone, colors[0])
    }

    fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError> {
        self.desc
            .validate_state(state)
            .map_err(|e| DeviceError::InvalidArgument(e.to_string()))?;
        // DeviceUpdateMode: setDirectMode(name == "Direct") on every mode change, then all LEDs.
        self.clear_sent();
        if state.mode == DIRECT_MODE {
            self.set_sw(true)?;
            return self.set_direct(&state.colors);
        }
        self.set_sw(false)?;
        let speed = raw_speed(state.speed.unwrap_or(DEFAULT_SPEED));
        let brightness = state.brightness.unwrap_or(BRIGHTNESS_MAX).min(BRIGHTNESS_MAX) as u8;
        for i in 0..self.zones.len() {
            let zone = &self.zones[i];
            let (fx, color) = if state.mode == OFF_MODE {
                // The reference sends effect index 0 for Off; this uses the zone's Off effect, or Static black.
                match zone
                    .effect_by_id(effect::OFF)
                    .or_else(|| zone.effect_by_id(effect::STATIC))
                {
                    Some(fx) => (fx, Rgb::BLACK),
                    None => continue,
                }
            } else {
                let Some(fx) = u8::try_from(state.mode)
                    .ok()
                    .and_then(|m| zone.effect_by_index(m))
                else {
                    continue;
                };
                (fx, state.colors.get(i).copied().unwrap_or_default())
            };
            let params = effect_params(zone.index, fx.index, fx.id, color, speed, brightness);
            self.link.set_zone_effect(self.feature, &params)?;
        }
        Ok(())
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        // The legacy controller sends nothing on shutdown. Here: SetSWControl(0, 0) (`11 <idx> <fi> 87 00 00`),
        // which hands the zones back to the firmware's effect engine, once after we touched the device. Then each
        // zone's open-time effect is replayed (volatile, persistence 0) where its GetEffect readback maps onto
        // SetZoneEffect (`hidpp::restore_params`), so our volatile Static/effect writes do not linger.
        if !self.touched {
            return Ok(());
        }
        self.set_sw(false)?;
        self.touched = false;
        self.clear_sent();
        self.link
            .restore_effects(self.feature, &self.zones, &self.initial)
    }

    /// The open-time readback, which is also what `release()` replays.
    fn hardware_mode(&self) -> Option<ModeState> {
        hardware_state(&self.desc, &self.zones, &self.initial)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::logitech::hidpp::{fp8070, long_reply, long_request, short_register};
    use cc_hid::{FakeTransport, ScriptBuilder};

    pub(crate) const IDX: u8 = 0x01;
    pub(crate) const FEAT: u8 = 0x0A;

    fn rr(b: ScriptBuilder, feature: u8, function: u8, params: &[u8], reply: &[u8]) -> ScriptBuilder {
        b.write(long_request(IDX, feature, function, params).to_vec())
            .read(long_reply(IDX, feature, function, reply))
    }

    /// Receiver scan with notifications already on and one paired PRO Wireless at index 1.
    pub(crate) fn scan_script(b: ScriptBuilder) -> ScriptBuilder {
        b.write(short_register(0xFF, 0x81, 0x00, [0; 3]).to_vec())
            .read(vec![0x10, 0xFF, 0x81, 0x00, 0x00, 0x01, 0x00])
            .write(short_register(0xFF, 0x81, 0x02, [0; 3]).to_vec())
            .read(vec![0x10, 0xFF, 0x81, 0x02, 0x00, 0x01, 0x00])
            .write(short_register(0xFF, 0x80, 0x02, [0x02, 0, 0]).to_vec())
            // Receiver ack, then the connection notification: flags 0x00 (linked), wpid 0x4079 little-endian.
            .read(vec![0x10, 0xFF, 0x80, 0x02, 0x00, 0x00, 0x00])
            .read(vec![0x10, 0x01, 0x41, 0x0C, 0x00, 0x79, 0x40])
    }

    /// Connected check for index 1: a linked HID++ 2.0 device rejects the register read with a non-0x09 error.
    pub(crate) fn connected_script(b: ScriptBuilder) -> ScriptBuilder {
        b.write(short_register(IDX, 0x81, 0x02, [0; 3]).to_vec())
            .read(vec![0x10, IDX, 0x8F, 0x81, 0x02, 0x01, 0x00])
    }

    /// Device probe on the long link: name "PRO", type mouse, 0x8070 at FEAT, two zones (Primary, Logo) with
    /// effects [0] Off, [1] Static, [2] Spectrum, [3] Breathing.
    pub(crate) fn device_script(b: ScriptBuilder) -> ScriptBuilder {
        let mut b = rr(b, 0x00, 0x00, &[0x00, 0x05], &[0x03]);
        b = rr(b, 0x03, 0x00, &[], &[10]);
        b = rr(b, 0x03, 0x10, &[0], b"PRO Wirele");
        b = rr(b, 0x03, 0x20, &[], &[3]);
        b = rr(b, 0x00, 0x00, &[0x80, 0x70], &[FEAT]);
        b = rr(b, FEAT, fp8070::INFO, &[], &[2]);
        for (z, loc) in [(0u8, 1u8), (1, 2)] {
            b = rr(b, FEAT, fp8070::ZONE_INFO, &[z, 0], &[z, 0, loc, 4]);
            for (e, id) in [(0u8, 0x00u8), (1, 0x01), (2, 0x03), (3, 0x0A)] {
                b = rr(
                    b,
                    FEAT,
                    fp8070::EFFECT_INFO,
                    &[z, e, 0, 0],
                    &[z, e, 0, id, 0, 0, 0x27, 0x10],
                );
            }
        }
        // Zone 0 readback maps (Static, index 1, red, marker 0x02); zone 1 echoes the wrong zone and is not
        // restored.
        b = rr(b, FEAT, fp8070::GET_EFFECT, &[0], &[0, 1, 0xFF, 0, 0, 0x02]);
        rr(b, FEAT, fp8070::GET_EFFECT, &[1], &[0, 1, 0xFF, 0, 0])
    }

    fn controller(extra: ScriptBuilder) -> (LightspeedController, FakeTransport) {
        let fake = FakeTransport::new(
            device_script(ScriptBuilder::new())
                .steps(extra.into_steps())
                .into_steps(),
        );
        let mut link = Link::new(Box::new(fake.clone()), IDX);
        let p = probe_device(&mut link).expect("probe");
        assert!(p.is_valid());
        let desc = describe(
            &p,
            PRO_WIRELESS_WPID,
            DeviceId::hid(0x046D, PRO_WIRELESS_WPID, None),
            "path".into(),
        );
        desc.validate().expect("valid description");
        (LightspeedController::new(link, p, desc), fake)
    }

    #[test]
    fn hardware_mode_from_the_open_time_readback() {
        // Zone 0 read back Static (index 1) in red; zone 1 echoed the wrong zone, so its LED is black. Static has no
        // brightness byte in its layout, so none is reported.
        let (c, fake) = controller(ScriptBuilder::new());
        assert_eq!(
            c.hardware_mode(),
            Some(ModeState {
                mode: 1,
                speed: None,
                brightness: None,
                direction: None,
                colors: vec![Rgb::new(0xFF, 0, 0), Rgb::BLACK],
            })
        );
        fake.finish().expect("script consumed");
    }

    fn set_effect(params: [u8; 16]) -> Vec<u8> {
        long_request(IDX, FEAT, fp8070::SET_EFFECT, &params).to_vec()
    }

    fn ack(function: u8) -> Vec<u8> {
        long_reply(IDX, FEAT, function, &[])
    }

    /// Release tail after SetSWControl(0, 0): zone 0's readback replayed verbatim with persistence 0.
    fn restore(b: ScriptBuilder) -> ScriptBuilder {
        b.write(set_effect([0, 1, 0xFF, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
            .read(ack(fp8070::SET_EFFECT))
    }

    fn sw(b: ScriptBuilder, on: bool) -> ScriptBuilder {
        let v = u8::from(on);
        // setDirectMode: set_direct.data[0] = direct, data[1] = data[0]; fn 0x80 | SW_ID.
        b.write(long_request(IDX, FEAT, fp8070::SET_SW_CTL, &[v, v]).to_vec())
            .read(ack(fp8070::SET_SW_CTL))
    }

    #[test]
    fn scan_finds_the_paired_device() {
        let fake = scan_script(FakeTransport::builder()).build();
        let mut short = fake.clone();
        let found = scan_receiver(&mut short).expect("scan");
        assert_eq!(
            found,
            [Paired {
                index: 1,
                wpid: PRO_WIRELESS_WPID,
                online: true
            }]
        );
        fake.finish().expect("script consumed");
    }

    #[test]
    fn scan_enables_notifications_when_off() {
        // Register 0x00 byte 1 bit 0 clear: the reference writes 10 FF 80 00 00 01 00.
        let fake = FakeTransport::builder()
            .write(short_register(0xFF, 0x81, 0x00, [0; 3]).to_vec())
            .read(vec![0x10, 0xFF, 0x81, 0x00, 0x00, 0x00, 0x00])
            .write(vec![0x10, 0xFF, 0x80, 0x00, 0x00, 0x01, 0x00])
            .read(vec![0x10, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x00])
            .write(short_register(0xFF, 0x81, 0x02, [0; 3]).to_vec())
            // A stray notification of another index first, then the reply: zero devices connected.
            .read(vec![0x10, 0x02, 0x41, 0x0C, 0x40, 0x7F, 0x40])
            .read(vec![0x10, 0xFF, 0x81, 0x02, 0x00, 0x00, 0x00])
            .build();
        let mut short = fake.clone();
        assert_eq!(scan_receiver(&mut short).expect("scan"), []);
        fake.finish().expect("script consumed");
    }

    #[test]
    fn connected_check() {
        let fake = connected_script(FakeTransport::builder())
            .write(short_register(IDX, 0x81, 0x02, [0; 3]).to_vec())
            .read(vec![0x10, IDX, 0x8F, 0x81, 0x02, 0x09, 0x00])
            .write(short_register(IDX, 0x81, 0x02, [0; 3]).to_vec())
            .read_timeout()
            .build();
        let mut short = fake.clone();
        assert_eq!(is_connected(&mut short, IDX), Ok(Some(true)));
        assert_eq!(is_connected(&mut short, IDX), Ok(Some(false)));
        assert_eq!(is_connected(&mut short, IDX), Ok(None));
        fake.finish().expect("script consumed");
    }

    #[test]
    fn notification_parsing() {
        assert_eq!(
            parse_connection_notification(&[0x10, 0x01, 0x41, 0x0C, 0x40, 0x79, 0x40]),
            Some(Paired {
                index: 1,
                wpid: 0x4079,
                online: false
            })
        );
        assert_eq!(
            parse_connection_notification(&[0x10, 0xFF, 0x41, 0, 0, 0, 0]),
            None
        );
        assert_eq!(
            parse_connection_notification(&[0x11, 0x01, 0x41, 0, 0, 0, 0]),
            None
        );
    }

    #[test]
    fn description() {
        let (c, fake) = controller(ScriptBuilder::new());
        let d = c.description();
        assert_eq!(d.info.name, "PRO Wirele");
        assert_eq!(d.info.kind, DeviceKind::Mouse);
        let zones: Vec<_> = d.zones.iter().map(|z| z.name.as_str()).collect();
        assert_eq!(zones, ["Primary Zone", "Logo Zone"]);
        assert_eq!(d.leds[1].name, "Logo LED");
        let modes: Vec<_> = d.modes.iter().map(|m| (m.id, m.name.as_str())).collect();
        assert_eq!(
            modes,
            [
                (OFF_MODE, "Off"),
                (DIRECT_MODE, "Direct"),
                (1, "Static"),
                (2, "Spectrum Cycle"),
                (3, "Breathing")
            ]
        );
        assert_eq!(c.initial_state().len(), 2);
        fake.finish().expect("script consumed");
    }

    #[test]
    fn params_are_byte_exact() {
        // setMode fp8070: data[0]=zone, data[1]=mode, data[2..5]=R,G,B, data[12]=0.
        // ON: nothing else.
        assert_eq!(
            effect_params(1, 1, effect::STATIC, Rgb::new(1, 2, 3), 50, 100),
            [1, 1, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        // SPECTRUM: speed *= 100 (50 -> 5000 = 0x1388) at data[7..9], brightness at data[9].
        assert_eq!(
            effect_params(0, 2, effect::CYCLE, Rgb::BLACK, 50, 100),
            [0, 2, 0, 0, 0, 0, 0, 0x13, 0x88, 100, 0, 0, 0, 0, 0, 0]
        );
        // BREATHING: period at data[5..7], brightness at data[8]. 200 -> 20000 = 0x4E20.
        assert_eq!(
            effect_params(0, 3, effect::BREATHING, Rgb::new(9, 8, 7), 200, 1),
            [0, 3, 9, 8, 7, 0x4E, 0x20, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(raw_speed(SPEED_MIN), 200);
        assert_eq!(raw_speed(SPEED_MAX), 10);
        assert_eq!(raw_speed(DEFAULT_SPEED), RAW_SPEED_NORMAL);
    }

    #[test]
    fn direct_then_release_then_direct() {
        let red = Rgb::new(0xFF, 0, 0);
        let frame = |s: ScriptBuilder| {
            s.write(set_effect([0, 1, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .read(ack(fp8070::SET_EFFECT))
                .write(set_effect([1, 1, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .read(ack(fp8070::SET_EFFECT))
        };
        let mut s = frame(sw(ScriptBuilder::new(), true));
        // Second identical frame: nothing. Then release: 11 01 0A 87 00 00, then the restore.
        s = restore(sw(s, false));
        s = frame(sw(s, true));
        let (mut c, fake) = controller(s);
        c.set_direct(&[red, red]).expect("frame");
        c.set_direct(&[red, red]).expect("unchanged frame");
        c.release().expect("release");
        c.release().expect("second release is a no-op");
        c.set_direct(&[red, red]).expect("frame after release");
        assert!(c.is_claimed());
        fake.finish().expect("script consumed");
    }

    #[test]
    fn set_mode_spectrum_and_off_then_release() {
        let mut s = sw(ScriptBuilder::new(), false);
        // Spectrum Cycle (index 2), UI speed 160 -> raw 50 -> 5000 ms, brightness 100.
        for z in 0..2u8 {
            s = s
                .write(set_effect([
                    z, 2, 0, 0, 0, 0, 0, 0x13, 0x88, 100, 0, 0, 0, 0, 0, 0,
                ]))
                .read(ack(fp8070::SET_EFFECT));
        }
        // Off: SW off again, then the zone's Off effect (index 0).
        s = sw(s, false);
        for z in 0..2u8 {
            s = s
                .write(set_effect([z, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .read(ack(fp8070::SET_EFFECT));
        }
        // release after an effect was written: SetSWControl(0, 0), then the restore.
        s = restore(sw(s, false));
        let (mut c, fake) = controller(s);
        c.set_mode(&ModeState {
            mode: 2,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![],
        })
        .expect("spectrum");
        c.set_mode(&ModeState {
            mode: OFF_MODE,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![],
        })
        .expect("off");
        c.release().expect("release");
        c.release().expect("second release is a no-op");
        assert_eq!(c.save_to_hardware(), Err(DeviceError::Unsupported));
        fake.finish().expect("script consumed");
    }

    #[test]
    fn set_mode_direct_claims_and_paints() {
        let blue = Rgb::new(0, 0, 0xFF);
        let mut s = sw(ScriptBuilder::new(), true);
        for z in 0..2u8 {
            s = s
                .write(set_effect([z, 1, 0, 0, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .read(ack(fp8070::SET_EFFECT));
        }
        let (mut c, fake) = controller(s);
        c.set_mode(&ModeState {
            mode: DIRECT_MODE,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![blue, blue],
        })
        .expect("direct");
        fake.finish().expect("script consumed");
    }

    #[test]
    fn lost_claim_ack_still_releases() {
        let s = ScriptBuilder::new()
            .write(long_request(IDX, FEAT, fp8070::SET_SW_CTL, &[1, 1]).to_vec())
            .read_timeout();
        let s = restore(sw(s, false));
        let (mut c, fake) = controller(s);
        assert_eq!(c.set_direct(&[Rgb::BLACK, Rgb::BLACK]), Err(DeviceError::Timeout));
        assert!(!c.is_claimed());
        c.release().expect("release");
        c.release().expect("second release is a no-op");
        fake.finish().expect("script consumed");
    }

    #[test]
    fn retry_gives_up_on_invalid_identity() {
        // No name feature: invalid identity on every attempt, each attempt a full probe after a flush.
        let mut b = FakeTransport::builder();
        for _ in 0..DETECT_MAX_RETRY {
            b = b.read_timeout().read_timeout();
            b = b
                .write(long_request(IDX, 0, 0, &[0x00, 0x05]).to_vec())
                .read(long_reply(IDX, 0, 0, &[0]))
                .write(long_request(IDX, 0, 0, &[0x80, 0x70]).to_vec())
                .read(long_reply(IDX, 0, 0, &[FEAT]))
                .write(long_request(IDX, FEAT, fp8070::INFO, &[]).to_vec())
                .read(long_reply(IDX, FEAT, fp8070::INFO, &[0]));
        }
        let fake = b.build();
        let mut short = fake.clone();
        let mut link = Link::new(Box::new(fake.clone()), IDX);
        assert_eq!(probe_with_retry(&mut short, &mut link), Ok(None));
        fake.finish().expect("script consumed");
    }
}
