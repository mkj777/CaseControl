//! Logitech G560 speaker: HID++ 2.0 on usage page 0xFF43, zone-only lighting through feature 0x8070.
//!
//! Ported from the zone-only path of `LogitechHIDPP20Controller` / `RGBController_LogitechHIDPP20` (the unified
//! HID++ 2.0 controller that owns the G560 upstream): one LED per 0x8070 zone, Direct rides the Static effect per
//! zone, hardware modes come from zone 0's effect list. Claiming is `SetSWControl(1, 1)`, releasing
//! `SetSWControl(0, 0)` (`ClaimSWControlIfNeeded` / `Shutdown`); nothing else changes device state. Every effect write
//! is volatile (persistence byte 0) except in [`Controller::save_to_hardware`].

use cc_core::{
    Bus, ColorMode, Controller, DeviceDescription, DeviceError, DeviceId, DeviceInfo, DeviceKind, Direction,
    Led, Mode, ModeCaps, ModeState, Rgb, Zone, ZoneKind,
};

use super::hidpp::{
    DecodedEffect, EffectInfo, Link, PAGE_COLOR_LED_EFFECTS, PAGE_DEVICE_NAME_TYPE, PAGE_FEATURE_SET,
    ZoneEffectState, ZoneInfo, effect,
};

/// G560 product id.
pub const G560_PID: u16 = 0x0A78;
/// Vendor usage page of the G560's HID++ collection.
pub const G560_USAGE_PAGE: u16 = 0xFF43;

/// Mode id of Direct (per-zone Static colours, volatile).
pub const DIRECT_MODE: u32 = 0x100;
/// Mode id of Off (Static black on every zone; `Off.value = 0xFF` in the reference).
pub const OFF_MODE: u32 = 0xFF;

/// Name used when the device does not report a plausible one (`HIDPP20_NAME_PLACEHOLDER_STD`).
pub const PLACEHOLDER_NAME: &str = "Logitech HID++ Device";

/// Speed slider range (`HIDPP20_SPEED_SLIDER_MIN..=MAX`); higher is faster.
pub const SPEED_MIN: u32 = 1;
/// See [`SPEED_MIN`].
pub const SPEED_MAX: u32 = 100;
/// Effect period range in ms for the speed slider (`HIDPP20_PERIOD_MIN_MS..=MAX_MS`).
pub const PERIOD_MIN_MS: u16 = 1000;
/// See [`PERIOD_MIN_MS`].
pub const PERIOD_MAX_MS: u16 = 20000;
/// Ripple's own, much faster period range (`HIDPP20_RIPPLE_PERIOD_MIN_MS..=MAX_MS`).
pub const RIPPLE_PERIOD_MIN_MS: u16 = 2;
/// See [`RIPPLE_PERIOD_MIN_MS`].
pub const RIPPLE_PERIOD_MAX_MS: u16 = 200;

/// Logitech device type "speaker" (`LOGITECH_DEVICE_TYPE_SPEAKER`).
pub const DEVICE_TYPE_SPEAKER: u8 = 14;

/// `SliderToPeriodMs`: maps a speed slider value to an effect period; slider 1 is the slowest (`max_ms`).
pub fn slider_to_period(slider: u32, min_ms: u16, max_ms: u16) -> u16 {
    if slider <= SPEED_MIN {
        return max_ms;
    }
    if slider >= SPEED_MAX {
        return min_ms;
    }
    let range = u32::from(max_ms - min_ms);
    let steps = SPEED_MAX - SPEED_MIN;
    // At most 20000, fits u16.
    (u32::from(max_ms) - ((slider - SPEED_MIN) * range) / steps) as u16
}

/// `WaveDirectionToWire`: Logitech's wire value for a wave direction.
pub fn wave_direction(direction: Option<Direction>) -> u8 {
    match direction {
        Some(Direction::Left) => 6,
        Some(Direction::Right) | None => 1,
        Some(Direction::Up) => 7,
        Some(Direction::Down) => 2,
        Some(Direction::Horizontal) => 3,
        Some(Direction::Vertical) => 8,
    }
}

/// `zone_location_name`.
pub fn zone_location_name(location: u16) -> String {
    match location {
        0x0001 => "Primary".into(),
        0x0002 => "Logo".into(),
        0x0003 => "Left Side".into(),
        0x0004 => "Right Side".into(),
        0x0005 => "Combined".into(),
        0x0006 => "Primary 1".into(),
        0x0007 => "Primary 2".into(),
        0x0008 => "Primary 3".into(),
        0x0009 => "Primary 4".into(),
        0x000A => "Primary 5".into(),
        0x000B => "Primary 6".into(),
        other => format!("Zone 0x{other:04X}"),
    }
}

/// `speaker_zone_location_name`: speakers report the generic codes but the lamps are front/rear per satellite.
pub fn speaker_zone_location_name(location: u16) -> String {
    match location {
        0x0001 => "Left Front".into(),
        0x0002 => "Right Front".into(),
        0x0003 => "Left Rear".into(),
        0x0004 => "Right Rear".into(),
        other => zone_location_name(other),
    }
}

/// Maps a Logitech device type byte to a [`DeviceKind`] (the HID++ 2.0 controller's switch).
pub fn device_kind(device_type: u8) -> DeviceKind {
    match device_type {
        0 => DeviceKind::Keyboard,
        2 => DeviceKind::Keypad,
        3 | 5 => DeviceKind::Mouse,
        4 => DeviceKind::Mousemat,
        8 => DeviceKind::Headset,
        11 | 12 => DeviceKind::Gamepad,
        14 => DeviceKind::Speaker,
        15 => DeviceKind::Microphone,
        16 => DeviceKind::Light,
        _ => DeviceKind::Unknown,
    }
}

/// Builds the 16 SetZoneEffect parameter bytes (`SetZoneEffect` in the HID++ 2.0 controller): zone, effect index,
/// 10 effect parameter bytes laid out per effect type, persistence at byte 12.
/// Inverse of [`slider_to_period`]: the speed slider value for `period` (clamped to the range), rounded.
pub fn period_to_slider(period: u16, min_ms: u16, max_ms: u16) -> u32 {
    let p = period.clamp(min_ms, max_ms);
    let range = u32::from(max_ms - min_ms).max(1);
    let steps = SPEED_MAX - SPEED_MIN;
    SPEED_MIN + (u32::from(max_ms - p) * steps + range / 2) / range
}

/// Inverse of [`wave_direction`].
pub fn direction_from_code(code: u8) -> Option<Direction> {
    match code {
        6 => Some(Direction::Left),
        1 => Some(Direction::Right),
        7 => Some(Direction::Up),
        2 => Some(Direction::Down),
        3 => Some(Direction::Horizontal),
        8 => Some(Direction::Vertical),
        _ => None,
    }
}

/// The mode the device shows on its own, from each zone's open-time readback: the first zone whose readback decodes
/// ([`decode_effect`](super::hidpp::decode_effect)) names the mode (its effect index, or "Off"), speed, brightness
/// and direction; per-LED colours come from every zone (black where a zone's readback has none). `None` when no
/// zone decodes or the result is not a valid state of `desc`.
pub fn hardware_state(
    desc: &DeviceDescription,
    zones: &[ZoneInfo],
    initial: &[ZoneEffectState],
) -> Option<ModeState> {
    let decoded: Vec<Option<DecodedEffect>> = zones
        .iter()
        .map(|z| {
            initial
                .iter()
                .find(|s| s.zone == z.index)
                .and_then(|s| super::hidpp::decode_effect(s, z))
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
        if matches!(first.effect.id, effect::RIPPLE | effect::RIPPLE_SAT) {
            period_to_slider(p, RIPPLE_PERIOD_MIN_MS, RIPPLE_PERIOD_MAX_MS)
        } else {
            period_to_slider(p, PERIOD_MIN_MS, PERIOD_MAX_MS)
        }
    });
    let brightness = first
        .brightness
        .zip(mode.brightness.clone())
        .filter(|_| has(ModeCaps::BRIGHTNESS))
        .map(|(b, r)| u32::from(b).clamp(*r.start(), *r.end()));
    let direction = first
        .direction
        .and_then(direction_from_code)
        .filter(|d| has(d.required_cap()));
    let colors = match mode.color_mode {
        ColorMode::PerLed => decoded
            .iter()
            .map(|d| d.and_then(|d| d.color).unwrap_or(Rgb::BLACK))
            .collect(),
        ColorMode::ModeSpecific => vec![first.color.unwrap_or(Rgb::BLACK)],
        ColorMode::None | ColorMode::Random => Vec::new(),
    };
    let state = ModeState {
        mode: id,
        speed,
        brightness,
        direction,
        colors,
    };
    desc.validate_state(&state).is_ok().then_some(state)
}

#[allow(clippy::too_many_arguments)]
pub fn effect_params(
    zone: u8,
    effect_index: u8,
    effect_id: u16,
    color: Rgb,
    period: u16,
    brightness: u8,
    direction: u8,
    persist: bool,
) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[0] = zone;
    d[1] = effect_index;
    let [ph, pl] = period.to_be_bytes();
    let (r, g, b) = (color.r, color.g, color.b);
    let black = color == Rgb::BLACK;
    match effect_id {
        effect::STATIC => {
            d[2..5].copy_from_slice(&[r, g, b]);
            if !black {
                d[5] = 0x02;
            }
        }
        effect::BREATHING => {
            d[2..5].copy_from_slice(&[r, g, b]);
            d[5] = ph;
            d[6] = pl;
            d[8] = brightness;
        }
        effect::CYCLE | effect::SIGNATURE1 | effect::SIGNATURE2 => {
            d[7] = ph;
            d[8] = pl;
            d[9] = brightness;
        }
        effect::AUDIO => {
            if black {
                d[2] = 0x01;
            } else {
                d[2] = 0x00;
                d[3..6].copy_from_slice(&[r, g, b]);
            }
            d[7] = ph;
            d[8] = pl;
        }
        effect::WAVE => {
            d[8] = pl;
            d[9] = direction;
            d[10] = if brightness == 0 { 1 } else { brightness };
            d[11] = ph;
        }
        effect::RIPPLE => {
            d[2..5].copy_from_slice(&[r, g, b]);
            d[6] = ph;
            d[7] = pl;
        }
        effect::DECOMPOSITION => {
            d[8] = ph;
            d[9] = pl;
            d[10] = brightness;
        }
        effect::CYCLE_SAT => {
            d[3] = 0xFF;
            d[8] = ph;
            d[9] = pl;
            d[10] = brightness;
        }
        effect::WAVE_SAT => {
            d[3] = 0xFF;
            d[8] = ph;
            d[9] = pl;
            d[10] = brightness;
            d[11] = direction;
        }
        effect::RIPPLE_SAT => {
            d[2..5].copy_from_slice(&[r, g, b]);
            d[5] = 0xFF;
            d[8] = ph;
            d[9] = pl;
        }
        _ => {
            d[2..5].copy_from_slice(&[r, g, b]);
            d[5] = ph;
            d[6] = pl;
        }
    }
    d[12] = u8::from(persist);
    d
}

fn mode(id: u32, name: &str, caps: ModeCaps, color_mode: ColorMode) -> Mode {
    let has = |c| caps.contains(c);
    Mode {
        id,
        name: name.into(),
        caps,
        color_mode,
        speed: has(ModeCaps::SPEED).then_some(SPEED_MIN..=SPEED_MAX),
        brightness: has(ModeCaps::BRIGHTNESS).then_some(1..=100),
        colors: (color_mode == ColorMode::ModeSpecific).then_some(1..=1),
    }
}

/// Default speed slider value of a mode when a state leaves it out (the reference's `mode.speed` defaults).
fn default_speed(effect_id: u16) -> u32 {
    match effect_id {
        effect::BREATHING | effect::RIPPLE | effect::RIPPLE_SAT => 70,
        _ => 80,
    }
}

/// The mode list of the HID++ 2.0 controller for a zone-only 0x8070 device: Direct, Off, then one mode per
/// recognised effect of zone 0. `per_led_colors` is true when the device has more than one zone.
/// `effect_brightness` is false for 0x8070 keyboards, whose firmware ignores the intensity byte.
pub fn build_modes(zone0: &[EffectInfo], per_led_colors: bool, effect_brightness: bool) -> Vec<Mode> {
    let save = ModeCaps::SAVE_MANUAL;
    let (color_caps, color_mode) = if per_led_colors {
        (ModeCaps::PER_LED_COLOR, ColorMode::PerLed)
    } else {
        (ModeCaps::MODE_SPECIFIC_COLOR, ColorMode::ModeSpecific)
    };
    let bright = if effect_brightness {
        ModeCaps::BRIGHTNESS
    } else {
        ModeCaps::empty()
    };
    let mut modes = vec![
        mode(DIRECT_MODE, "Direct", ModeCaps::PER_LED_COLOR, ColorMode::PerLed),
        mode(OFF_MODE, "Off", save, ColorMode::None),
    ];
    for fx in zone0 {
        let id = u32::from(fx.index);
        let m = match fx.id {
            effect::STATIC => mode(id, "Static", color_caps | save, color_mode),
            effect::CYCLE => mode(
                id,
                "Spectrum Cycle",
                ModeCaps::SPEED | bright | save,
                ColorMode::None,
            ),
            effect::BREATHING => mode(
                id,
                "Breathing",
                color_caps | ModeCaps::SPEED | bright | save,
                color_mode,
            ),
            effect::AUDIO => mode(
                id,
                "Audio Visualizer",
                color_caps | ModeCaps::SPEED | save,
                color_mode,
            ),
            effect::WAVE => mode(id, "Color Wave", ModeCaps::SPEED | bright | save, ColorMode::None),
            effect::RIPPLE => mode(
                id,
                "Ripple",
                ModeCaps::MODE_SPECIFIC_COLOR | ModeCaps::SPEED | ModeCaps::BRIGHTNESS | save,
                ColorMode::ModeSpecific,
            ),
            effect::CYCLE_SAT => mode(
                id,
                "Spectrum Cycle",
                ModeCaps::SPEED | ModeCaps::BRIGHTNESS | save,
                ColorMode::None,
            ),
            effect::WAVE_SAT => mode(
                id,
                "Color Wave",
                ModeCaps::SPEED
                    | ModeCaps::BRIGHTNESS
                    | ModeCaps::DIRECTION_LR
                    | ModeCaps::DIRECTION_UD
                    | ModeCaps::DIRECTION_HV
                    | save,
                ColorMode::None,
            ),
            effect::RIPPLE_SAT => mode(
                id,
                "Ripple",
                ModeCaps::MODE_SPECIFIC_COLOR | ModeCaps::SPEED | save,
                ColorMode::ModeSpecific,
            ),
            effect::DECOMPOSITION => mode(
                id,
                "Decomposition",
                ModeCaps::SPEED | ModeCaps::BRIGHTNESS | save,
                ColorMode::None,
            ),
            effect::SIGNATURE1 => mode(
                id,
                "Signature 1",
                ModeCaps::SPEED | ModeCaps::BRIGHTNESS | save,
                ColorMode::None,
            ),
            effect::SIGNATURE2 => mode(
                id,
                "Signature 2",
                ModeCaps::SPEED | ModeCaps::BRIGHTNESS | save,
                ColorMode::None,
            ),
            _ => continue,
        };
        modes.push(m);
    }
    modes
}

/// Everything detection learned from a G560 (or any zone-only 0x8070 HID++ 2.0 device on this path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct G560Probe {
    /// Name reported by feature 0x0005, or [`PLACEHOLDER_NAME`].
    pub name: String,
    /// Logitech device type byte (14 = speaker).
    pub device_type: u8,
    /// Runtime index of feature 0x8070.
    pub feature: u8,
    /// Discovered zones.
    pub zones: Vec<ZoneInfo>,
    /// What each zone showed before anything was claimed (0x8070 GetEffect, read-only).
    pub initial: Vec<ZoneEffectState>,
}

/// Detection-time conversation (`Probe` reduced to what a zone-only device needs): flush, IRoot lookup of
/// FeatureSet (the "is this HID++" check), device name and type, 0x8070 lookup and discovery, live-effect readback.
/// Read-only: no request here changes device state.
pub fn probe(link: &mut Link) -> Result<G560Probe, DeviceError> {
    link.flush()?;
    if link.feature_index(PAGE_FEATURE_SET)? == 0 {
        return Err(DeviceError::Protocol("IRoot did not report FeatureSet".into()));
    }
    let name_feature = link.feature_index(PAGE_DEVICE_NAME_TYPE)?;
    let (name, device_type) = if name_feature == 0 {
        (None, DEVICE_TYPE_SPEAKER)
    } else {
        let name = link.device_name(name_feature)?;
        (name, link.device_type(name_feature)?)
    };
    let feature = link.feature_index(PAGE_COLOR_LED_EFFECTS)?;
    if feature == 0 {
        return Err(DeviceError::Unsupported);
    }
    let zones = link.discover_8070(feature)?;
    if zones.is_empty() {
        return Err(DeviceError::Protocol("0x8070 reported no zones".into()));
    }
    let initial = link.read_effect_state(feature, &zones)?;
    Ok(G560Probe {
        name: name.unwrap_or_else(|| PLACEHOLDER_NAME.into()),
        device_type,
        feature,
        zones,
        initial,
    })
}

/// Builds the device description for a probed device.
pub fn describe(
    probe: &G560Probe,
    id: DeviceId,
    location: String,
    serial: Option<String>,
) -> DeviceDescription {
    let speaker = probe.device_type == DEVICE_TYPE_SPEAKER;
    let mut zones = Vec::with_capacity(probe.zones.len());
    let mut leds = Vec::with_capacity(probe.zones.len());
    for (i, z) in probe.zones.iter().enumerate() {
        let name = if speaker {
            speaker_zone_location_name(z.location)
        } else {
            zone_location_name(z.location)
        };
        zones.push(Zone {
            name: name.clone(),
            kind: ZoneKind::Single,
            start: i,
            len: 1,
            resizable: None,
            brightness_only: false,
        });
        leds.push(Led { name });
    }
    let zone0 = probe.zones.first().map(|z| z.effects.as_slice()).unwrap_or(&[]);
    let effect_brightness = probe.device_type != 0;
    DeviceDescription {
        info: DeviceInfo {
            id,
            name: probe.name.clone(),
            vendor: "Logitech".into(),
            kind: device_kind(probe.device_type),
            bus: Bus::Hid,
            location,
            serial,
            firmware: None,
        },
        zones,
        leds,
        modes: build_modes(zone0, probe.zones.len() > 1, effect_brightness),
        direct_mode: Some(DIRECT_MODE),
        keepalive: None,
    }
}

/// Controller for a G560 (zone-only HID++ 2.0, feature 0x8070).
#[derive(Debug)]
pub struct G560Controller {
    desc: DeviceDescription,
    link: Link,
    feature: u8,
    zones: Vec<ZoneInfo>,
    initial: Vec<ZoneEffectState>,
    /// SetSWControl(1, 1) was acknowledged.
    claimed: bool,
    /// A claim was attempted (acknowledged or not): release() must run.
    touched: bool,
    sent: Vec<Option<Rgb>>,
    active: Option<ModeState>,
}

impl G560Controller {
    /// Wraps a probed device. Sends nothing.
    pub fn new(link: Link, probe: G560Probe, desc: DeviceDescription) -> Self {
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
            active: None,
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

    /// Whether software control is currently claimed.
    pub fn is_claimed(&self) -> bool {
        self.claimed
    }

    fn claim(&mut self) -> Result<(), DeviceError> {
        if !self.claimed {
            // Marked before sending: a lost ack may still have left the device under software control, and
            // release() must then hand it back.
            self.touched = true;
            self.link.set_sw_control(self.feature, true)?;
            self.claimed = true;
        }
        Ok(())
    }

    fn paint_static(&mut self, i: usize, color: Rgb, persist: bool) -> Result<(), DeviceError> {
        let zone = &self.zones[i];
        let Some(fx) = zone.effect_by_id(effect::STATIC) else {
            return Ok(());
        };
        let params = effect_params(
            zone.index,
            fx.index,
            effect::STATIC,
            color,
            PERIOD_MAX_MS,
            100,
            0,
            persist,
        );
        self.link.set_zone_effect(self.feature, &params)
    }

    fn paint_zone(&mut self, i: usize, color: Rgb) -> Result<(), DeviceError> {
        if self.sent[i] == Some(color) {
            return Ok(());
        }
        self.sent[i] = None;
        self.paint_static(i, color, false)?;
        self.sent[i] = Some(color);
        Ok(())
    }

    fn apply_mode(&mut self, state: &ModeState, persist: bool) -> Result<(), DeviceError> {
        self.claim()?;
        self.sent.iter_mut().for_each(|s| *s = None);
        if state.mode == OFF_MODE {
            for i in 0..self.zones.len() {
                self.paint_static(i, Rgb::BLACK, persist)?;
            }
            return Ok(());
        }
        let mode = self
            .desc
            .mode(state.mode)
            .cloned()
            .ok_or_else(|| DeviceError::InvalidArgument(format!("unknown mode {}", state.mode)))?;
        let effect_index = u8::try_from(state.mode)
            .map_err(|_| DeviceError::InvalidArgument(format!("mode {} is not an effect", state.mode)))?;
        let brightness = if mode.caps.contains(ModeCaps::BRIGHTNESS) {
            state.brightness.unwrap_or(100).min(100) as u8
        } else {
            100
        };
        for i in 0..self.zones.len() {
            let zone = &self.zones[i];
            let Some(fx) = zone.effect_by_index(effect_index) else {
                tracing::debug!(target: "cc_drivers::logitech", zone = zone.index, effect_index, "zone lacks effect, skipped");
                continue;
            };
            let color = match mode.color_mode {
                ColorMode::PerLed => state.colors.get(i).copied().unwrap_or_default(),
                ColorMode::ModeSpecific => state.colors.first().copied().unwrap_or_default(),
                ColorMode::None | ColorMode::Random => Rgb::BLACK,
            };
            let speed = state.speed.unwrap_or_else(|| default_speed(fx.id));
            let period = if matches!(fx.id, effect::RIPPLE | effect::RIPPLE_SAT) {
                slider_to_period(speed, RIPPLE_PERIOD_MIN_MS, RIPPLE_PERIOD_MAX_MS)
            } else {
                slider_to_period(speed, PERIOD_MIN_MS, PERIOD_MAX_MS)
            };
            let params = effect_params(
                zone.index,
                effect_index,
                fx.id,
                color,
                period,
                brightness,
                wave_direction(state.direction),
                persist,
            );
            self.link.set_zone_effect(self.feature, &params)?;
        }
        Ok(())
    }
}

impl Controller for G560Controller {
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
        self.claim()?;
        self.active = None;
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
        self.claim()?;
        self.active = None;
        self.paint_zone(zone, colors[0])
    }

    fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError> {
        self.desc
            .validate_state(state)
            .map_err(|e| DeviceError::InvalidArgument(e.to_string()))?;
        if state.mode == DIRECT_MODE {
            self.sent.iter_mut().for_each(|s| *s = None);
            return self.set_direct(&state.colors);
        }
        self.apply_mode(state, false)?;
        self.active = Some(state.clone());
        Ok(())
    }

    fn save_to_hardware(&mut self) -> Result<(), DeviceError> {
        // DeviceSaveMode: replay the active firmware-effect mode with the persistence byte set. Direct is not savable.
        match self.active.clone() {
            Some(state) => self.apply_mode(&state, true),
            None => Err(DeviceError::Unsupported),
        }
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        // Shutdown: SetSWControl(0, 0) hands lighting back to the firmware effect. The G560 has no onboard-profile
        // feature, and this driver never touches one. Then each zone's open-time effect is replayed (volatile,
        // persistence 0) where its GetEffect readback maps onto SetZoneEffect (`hidpp::restore_params`), so our
        // volatile Static/effect writes do not linger.
        if !self.touched {
            return Ok(());
        }
        self.link.set_sw_control(self.feature, false)?;
        self.claimed = false;
        self.touched = false;
        self.sent.iter_mut().for_each(|s| *s = None);
        self.active = None;
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
    use crate::logitech::hidpp::{fp8070, long_reply, long_request};
    use cc_hid::{FakeTransport, ScriptBuilder};

    const DEV: u8 = 0xFF;
    const FEAT: u8 = 0x04;

    /// Request/reply pair: our request and the device's matching answer.
    fn rr(b: ScriptBuilder, feature: u8, function: u8, params: &[u8], reply: &[u8]) -> ScriptBuilder {
        b.write(long_request(DEV, feature, function, params).to_vec())
            .read(long_reply(DEV, feature, function, reply))
    }

    /// Detection script for a two-zone speaker: zone 0 (location 1) and zone 1 (location 2), each with
    /// effects [0] = Off (0x0000), [1] = Static (0x0001), [2] = Breathing (0x000A).
    pub(crate) fn probe_script() -> ScriptBuilder {
        let mut b = FakeTransport::builder().read_timeout(); // flush: queue empty
        b = rr(b, 0x00, 0x00, &[0x00, 0x01], &[0x01]); // FeatureSet at 1
        b = rr(b, 0x00, 0x00, &[0x00, 0x05], &[0x02]); // DeviceNameType at 2
        b = rr(b, 0x02, 0x00, &[], &[4]); // name length 4
        b = rr(b, 0x02, 0x10, &[0], b"G560");
        b = rr(b, 0x02, 0x20, &[], &[DEVICE_TYPE_SPEAKER]);
        b = rr(b, 0x00, 0x00, &[0x80, 0x70], &[FEAT]); // 0x8070 at 4
        b = rr(b, FEAT, fp8070::INFO, &[], &[2]);
        for (z, loc) in [(0u8, 1u8), (1, 2)] {
            b = rr(b, FEAT, fp8070::ZONE_INFO, &[z, 0], &[z, 0x00, loc, 3]);
            for (e, id) in [(0u8, 0x00u8), (1, 0x01), (2, 0x0A)] {
                b = rr(
                    b,
                    FEAT,
                    fp8070::EFFECT_INFO,
                    &[z, e, 0, 0],
                    &[z, e, 0x00, id, 0, 0, 0x13, 0x88],
                );
            }
        }
        // Zone 0 readback: Breathing (index 2) in SetZoneEffect shape, with a set persistence byte that the
        // restore must clear.
        b = rr(
            b,
            FEAT,
            fp8070::GET_EFFECT,
            &[0],
            &[0, 2, 1, 2, 3, 0x13, 0x88, 0, 50, 0, 0, 0, 1],
        );
        // Zone 1 readback rejected by the device: kept as None, not an error.
        b.write(long_request(DEV, FEAT, fp8070::GET_EFFECT, &[1]).to_vec())
            .read(vec![0x11, DEV, 0xFF, FEAT, 0xE7, 0x02])
    }

    fn controller(extra: ScriptBuilder) -> (G560Controller, FakeTransport) {
        let fake = FakeTransport::new(probe_script().steps(extra.into_steps()).into_steps());
        let mut link = Link::new(Box::new(fake.clone()), DEV);
        let p = probe(&mut link).expect("probe");
        let desc = describe(
            &p,
            DeviceId::hid(0x046D, G560_PID, Some("00000000")),
            "path".into(),
            None,
        );
        desc.validate().expect("valid description");
        (G560Controller::new(link, p, desc), fake)
    }

    /// `11 FF 04 37 <params>`: SetZoneEffect (fn 0x30 | SW_ID 7).
    fn set_effect(params: [u8; 16]) -> Vec<u8> {
        long_request(DEV, FEAT, fp8070::SET_EFFECT, &params).to_vec()
    }

    fn ack(function: u8) -> Vec<u8> {
        long_reply(DEV, FEAT, function, &[])
    }

    /// Release tail after SetSWControl(0, 0): zone 0's readback replayed with persistence 0; zone 1 (readback
    /// rejected at open) is skipped.
    fn restore(b: ScriptBuilder) -> ScriptBuilder {
        b.write(set_effect([
            0, 2, 1, 2, 3, 0x13, 0x88, 0, 50, 0, 0, 0, 0, 0, 0, 0,
        ]))
        .read(ack(fp8070::SET_EFFECT))
    }

    fn claim(b: ScriptBuilder, on: bool) -> ScriptBuilder {
        let v = u8::from(on);
        b.write(long_request(DEV, FEAT, fp8070::SET_SW_CTL, &[v, v]).to_vec())
            .read(ack(fp8070::SET_SW_CTL))
    }

    #[test]
    fn probe_and_description() {
        let (c, fake) = controller(ScriptBuilder::new());
        let d = c.description();
        assert_eq!(d.info.id.as_str(), "hid:046d:0a78");
        assert_eq!(d.info.name, "G560");
        assert_eq!(d.info.kind, DeviceKind::Speaker);
        let zone_names: Vec<_> = d.zones.iter().map(|z| z.name.as_str()).collect();
        assert_eq!(zone_names, ["Left Front", "Right Front"]);
        let modes: Vec<_> = d.modes.iter().map(|m| (m.id, m.name.as_str())).collect();
        assert_eq!(
            modes,
            [
                (DIRECT_MODE, "Direct"),
                (OFF_MODE, "Off"),
                (1, "Static"),
                (2, "Breathing")
            ]
        );
        let breathing = d.mode(2).expect("breathing");
        assert_eq!(breathing.color_mode, ColorMode::PerLed);
        assert!(
            breathing
                .caps
                .contains(ModeCaps::SPEED | ModeCaps::BRIGHTNESS | ModeCaps::SAVE_MANUAL)
        );
        assert_eq!(c.initial_state()[0].raw.map(|r| r[1]), Some(2));
        assert_eq!(c.initial_state()[1].raw, None);
        assert!(!c.is_claimed());
        fake.finish().expect("script consumed");
    }

    #[test]
    fn hardware_mode_from_the_open_time_readback() {
        // Zone 0 read back Breathing (index 2), colour 01 02 03, period 5000 ms, brightness 50; zone 1's readback
        // was rejected, so its LED is black. Speed: 1 + ((20000 - 5000) * 99 + 9500) / 19000 = 79.
        let (c, fake) = controller(ScriptBuilder::new());
        assert_eq!(
            c.hardware_mode(),
            Some(ModeState {
                mode: 2,
                speed: Some(79),
                brightness: Some(50),
                direction: None,
                colors: vec![Rgb::new(1, 2, 3), Rgb::BLACK],
            })
        );
        assert_eq!(
            period_to_slider(PERIOD_MAX_MS, PERIOD_MIN_MS, PERIOD_MAX_MS),
            SPEED_MIN
        );
        assert_eq!(
            period_to_slider(PERIOD_MIN_MS, PERIOD_MIN_MS, PERIOD_MAX_MS),
            SPEED_MAX
        );
        for slider in [1, 40, 79, 100] {
            let p = slider_to_period(slider, PERIOD_MIN_MS, PERIOD_MAX_MS);
            assert_eq!(period_to_slider(p, PERIOD_MIN_MS, PERIOD_MAX_MS), slider);
        }
        for d in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
            Direction::Horizontal,
            Direction::Vertical,
        ] {
            assert_eq!(direction_from_code(wave_direction(Some(d))), Some(d));
        }
        fake.finish().expect("script consumed");
    }

    #[test]
    fn static_params_are_byte_exact() {
        // SetZoneEffect case 0x0001: data[0]=zone, data[1]=effect index, data[2..5]=R,G,B, data[5]=0x02 marker when
        // the colour is not black, data[12]=persist.
        let p = effect_params(
            1,
            1,
            effect::STATIC,
            Rgb::new(0xFF, 0x80, 0x01),
            20000,
            100,
            0,
            false,
        );
        assert_eq!(p, [1, 1, 0xFF, 0x80, 0x01, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let black = effect_params(0, 1, effect::STATIC, Rgb::BLACK, 20000, 100, 0, true);
        assert_eq!(black, [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn effect_params_are_byte_exact() {
        // Breathing 0x000A: R,G,B at 2..5, period BE at 5..7, brightness at 8. Period 5000 = 0x1388.
        let p = effect_params(0, 2, effect::BREATHING, Rgb::new(1, 2, 3), 5000, 50, 0, false);
        assert_eq!(p, [0, 2, 1, 2, 3, 0x13, 0x88, 0, 50, 0, 0, 0, 0, 0, 0, 0]);
        // Cycle 0x0003: period BE at 7..9, brightness at 9; colour ignored.
        let p = effect_params(0, 3, effect::CYCLE, Rgb::new(9, 9, 9), 1000, 100, 0, false);
        assert_eq!(p, [0, 3, 0, 0, 0, 0, 0, 0x03, 0xE8, 100, 0, 0, 0, 0, 0, 0]);
        // Audio 0x0007, black: control byte 1 (self-cycling), period at 7..9.
        let p = effect_params(0, 4, effect::AUDIO, Rgb::BLACK, 0x1234, 100, 0, false);
        assert_eq!(p, [0, 4, 1, 0, 0, 0, 0, 0x12, 0x34, 0, 0, 0, 0, 0, 0, 0]);
        // Audio with a colour: control 0, R,G,B at 3..6.
        let p = effect_params(0, 4, effect::AUDIO, Rgb::new(4, 5, 6), 0x1234, 100, 0, true);
        assert_eq!(p, [0, 4, 0, 4, 5, 6, 0, 0x12, 0x34, 0, 0, 0, 1, 0, 0, 0]);
        // Wave 0x0004: period low at 8, direction at 9, brightness at 10, period high at 11.
        let p = effect_params(0, 5, effect::WAVE, Rgb::BLACK, 0x1234, 0, 6, false);
        assert_eq!(p, [0, 5, 0, 0, 0, 0, 0, 0, 0x34, 6, 1, 0x12, 0, 0, 0, 0]);
    }

    #[test]
    fn speed_slider_maps_to_period() {
        assert_eq!(slider_to_period(1, PERIOD_MIN_MS, PERIOD_MAX_MS), 20000);
        assert_eq!(slider_to_period(100, PERIOD_MIN_MS, PERIOD_MAX_MS), 1000);
        // 20000 - (79 * 19000) / 99 = 20000 - 15161 = 4839
        assert_eq!(slider_to_period(80, PERIOD_MIN_MS, PERIOD_MAX_MS), 4839);
        assert_eq!(
            slider_to_period(0, RIPPLE_PERIOD_MIN_MS, RIPPLE_PERIOD_MAX_MS),
            200
        );
    }

    #[test]
    fn set_direct_claims_once_and_sends_only_changes() {
        let red = Rgb::new(0xFF, 0, 0);
        let blue = Rgb::new(0, 0, 0xFF);
        let mut s = claim(ScriptBuilder::new(), true);
        // Frame 1: both zones (Static is effect index 1 in each zone).
        s = s
            .write(set_effect([0, 1, 0xFF, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
            .read(ack(fp8070::SET_EFFECT))
            .write(set_effect([1, 1, 0xFF, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
            .read(ack(fp8070::SET_EFFECT));
        // Frame 2: only zone 1 changed; a foreign G HUB ack (software id 1) is interleaved and ignored.
        s = s
            .write(set_effect([1, 1, 0, 0, 0xFF, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
            .read(vec![0x11, DEV, FEAT, 0x31])
            .read(ack(fp8070::SET_EFFECT));
        let (mut c, fake) = controller(s);
        c.set_direct(&[red, red]).expect("frame 1");
        c.set_direct(&[red, blue]).expect("frame 2");
        c.set_direct(&[red, blue]).expect("frame 3 sends nothing");
        assert!(c.is_claimed());
        fake.finish().expect("script consumed");
    }

    #[test]
    fn set_mode_breathing_and_off() {
        let s = claim(ScriptBuilder::new(), true)
            // Breathing (effect index 2), per-LED colours, speed 80 -> 4839 ms = 0x12E7, brightness 40.
            .write(set_effect([
                0, 2, 1, 2, 3, 0x12, 0xE7, 0, 40, 0, 0, 0, 0, 0, 0, 0,
            ]))
            .read(ack(fp8070::SET_EFFECT))
            .write(set_effect([
                1, 2, 4, 5, 6, 0x12, 0xE7, 0, 40, 0, 0, 0, 0, 0, 0, 0,
            ]))
            .read(ack(fp8070::SET_EFFECT))
            // Off: Static black on every zone, no marker byte.
            .write(set_effect([0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
            .read(ack(fp8070::SET_EFFECT))
            .write(set_effect([1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
            .read(ack(fp8070::SET_EFFECT))
            // save_to_hardware replays Off with persistence 1.
            .write(set_effect([0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]))
            .read(ack(fp8070::SET_EFFECT))
            .write(set_effect([1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]))
            .read(ack(fp8070::SET_EFFECT));
        let (mut c, fake) = controller(s);
        let breathing = ModeState {
            mode: 2,
            speed: Some(80),
            brightness: Some(40),
            direction: None,
            colors: vec![Rgb::new(1, 2, 3), Rgb::new(4, 5, 6)],
        };
        c.set_mode(&breathing).expect("breathing");
        let off = ModeState {
            mode: OFF_MODE,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![],
        };
        c.set_mode(&off).expect("off");
        c.save_to_hardware().expect("save");
        fake.finish().expect("script consumed");
    }

    #[test]
    fn invalid_mode_state_sends_nothing() {
        let (mut c, fake) = controller(ScriptBuilder::new());
        let bad = ModeState {
            mode: 2,
            speed: Some(500),
            brightness: None,
            direction: None,
            colors: vec![Rgb::BLACK, Rgb::BLACK],
        };
        assert!(matches!(c.set_mode(&bad), Err(DeviceError::InvalidArgument(_))));
        assert_eq!(c.save_to_hardware(), Err(DeviceError::Unsupported));
        fake.finish().expect("script consumed");
    }

    #[test]
    fn release_then_set_direct_reclaims() {
        let red = Rgb::new(0xFF, 0, 0);
        let frame = |s: ScriptBuilder| {
            s.write(set_effect([0, 1, 0xFF, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .read(ack(fp8070::SET_EFFECT))
                .write(set_effect([1, 1, 0xFF, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .read(ack(fp8070::SET_EFFECT))
        };
        let mut s = frame(claim(ScriptBuilder::new(), true));
        // release: SetSWControl(0, 0) = 11 FF 04 87 00 00 ...
        s = restore(claim(s, false));
        // set_direct after release: claim again and repaint everything (the cache was dropped).
        s = frame(claim(s, true));
        let (mut c, fake) = controller(s);
        c.set_direct(&[red, red]).expect("frame");
        c.release().expect("release");
        c.release().expect("second release is a no-op");
        assert!(!c.is_claimed());
        c.set_direct(&[red, red]).expect("frame after release");
        fake.finish().expect("script consumed");
    }

    #[test]
    fn lost_claim_ack_still_releases() {
        // SetSWControl(1, 1) goes out but its reply is lost: set_direct fails, release still hands control back.
        let s = ScriptBuilder::new()
            .write(long_request(DEV, FEAT, fp8070::SET_SW_CTL, &[1, 1]).to_vec())
            .read_timeout();
        let s = restore(claim(s, false));
        let (mut c, fake) = controller(s);
        assert_eq!(c.set_direct(&[Rgb::BLACK, Rgb::BLACK]), Err(DeviceError::Timeout));
        assert!(!c.is_claimed());
        c.release().expect("release");
        c.release().expect("second release is a no-op");
        fake.finish().expect("script consumed");
    }

    #[test]
    fn release_without_claim_sends_nothing() {
        let (mut c, fake) = controller(ScriptBuilder::new());
        c.release().expect("release");
        fake.finish().expect("script consumed");
    }

    #[test]
    fn probe_rejects_non_hidpp_collection() {
        let fake = FakeTransport::builder()
            .read_timeout()
            .write(long_request(DEV, 0, 0, &[0x00, 0x01]).to_vec())
            .read_timeout()
            .build();
        let mut link = Link::new(Box::new(fake.clone()), DEV);
        assert_eq!(probe(&mut link), Err(DeviceError::Timeout));
        fake.finish().expect("script consumed");
    }
}
