//! HID++ framing shared by the G560 and Lightspeed paths: request builders, reply matching, bounded reads, and the
//! feature 0x0005 / 0x8070 queries both devices use.
//!
//! Ported from `LogitechProtocolCommon.*` and the standard transport of `LogitechHIDPP20Controller.cpp`
//! (`SendStandard`, `ReadStandardDirect`, `SendAcked`). Every request carries the software id [`SW_ID`] in the low
//! nibble of its function byte, and a reply is accepted only when report type, device index, feature index and the
//! full function byte (function plus software id) match. Anything else (G HUB's or `logi_lamparray_service`'s
//! replies, notifications, input reports) is skipped, up to [`FOREIGN_BUDGET`] frames inside one [`REPLY_TIMEOUT`].

use std::time::{Duration, Instant};

use cc_core::DeviceError;
use cc_hid::{HidError, HidTransport};

/// Logitech USB vendor id.
pub const LOGITECH_VID: u16 = 0x046D;

/// HID++ short report id (7 bytes on the wire).
pub const REPORT_SHORT: u8 = 0x10;
/// Length of a short report including the id.
pub const SHORT_LEN: usize = 7;
/// HID++ long report id (20 bytes on the wire).
pub const REPORT_LONG: u8 = 0x11;
/// Length of a long report including the id.
pub const LONG_LEN: usize = 20;
/// HID++ very long report id (64 bytes); only skipped here.
pub const REPORT_VERY_LONG: u8 = 0x12;

/// Software id OR-ed into every function byte we send (`HIDPP20_SW_ID` in the reference). It is never 0x00, which
/// marks firmware notifications, so a reply can be told apart from an event; the reference picks 0x07 and so do we.
pub const SW_ID: u8 = 0x07;

/// Device index of a directly attached HID++ device (`LOGITECH_DEFAULT_DEVICE_INDEX`).
pub const DEFAULT_DEVICE_INDEX: u8 = 0xFF;
/// Device index of a receiver itself (`LOGITECH_RECEIVER_DEVICE_INDEX`).
pub const RECEIVER_INDEX: u8 = 0xFF;

/// How long one request waits for its reply (`LOGITECH_PROTOCOL_TIMEOUT`, 300 ms).
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(300);
/// How many foreign frames one request skips before giving up (`HIDPP20_READ_DRAIN_BUDGET` in spirit).
pub const FOREIGN_BUDGET: usize = 32;
/// Upper bound of queued reports dropped before a probe (the reference's pre-probe flush loop stops at 20).
pub const FLUSH_LIMIT: usize = 20;

/// Feature index of the root feature (IRoot).
pub const ROOT_INDEX: u8 = 0x00;
/// IRoot function 0: GetFeature(page) -> index.
pub const ROOT_GET_FEATURE: u8 = 0x00;

/// Feature page FeatureSet.
pub const PAGE_FEATURE_SET: u16 = 0x0001;
/// Feature page DeviceNameType.
pub const PAGE_DEVICE_NAME_TYPE: u16 = 0x0005;
/// Feature page Color LED Effects.
pub const PAGE_COLOR_LED_EFFECTS: u16 = 0x8070;

/// DeviceNameType function 0: name length.
pub const NAME_GET_COUNT: u8 = 0x00;
/// DeviceNameType function 1: name chunk at an offset.
pub const NAME_GET_NAME: u8 = 0x10;
/// DeviceNameType function 2: device type.
pub const NAME_GET_TYPE: u8 = 0x20;

/// Feature 0x8070 functions (`LOGITECH_FP8070_*`, high nibble only; the low nibble is the software id).
pub mod fp8070 {
    /// GetInfo: zone count in byte 0.
    pub const INFO: u8 = 0x00;
    /// GetZoneInfo(zone): location (BE16) at 1..3, effect count at 3.
    pub const ZONE_INFO: u8 = 0x10;
    /// GetZoneEffectInfo(zone, effect): effect id (BE16) at 2..4, capabilities at 4..6, period at 6..8.
    pub const EFFECT_INFO: u8 = 0x20;
    /// SetZoneEffect(zone, effect, 10 parameter bytes, persistence at 12).
    pub const SET_EFFECT: u8 = 0x30;
    /// SetSWControl(enabled, persist).
    pub const SET_SW_CTL: u8 = 0x80;
    /// GetEffect(zone): read back the live effect of a zone.
    pub const GET_EFFECT: u8 = 0xE0;
}

/// HID++ 1.0 SET_REGISTER sub id.
pub const SET_REGISTER: u8 = 0x80;
/// HID++ 1.0 GET_REGISTER sub id.
pub const GET_REGISTER: u8 = 0x81;
/// HID++ 1.0 error sub id.
pub const ERROR_HIDPP10: u8 = 0x8F;
/// HID++ 2.0 error feature index.
pub const ERROR_HIDPP20: u8 = 0xFF;
/// Receiver register 0x00: HID++ notification flags.
pub const REG_NOTIFICATIONS: u8 = 0x00;
/// Receiver register 0x02: connection state.
pub const REG_CONNECTION_STATE: u8 = 0x02;
/// DJ / HID++ 1.0 device connection notification sub id.
pub const DEVICE_CONNECTION: u8 = 0x41;
/// HID++ 1.0 error code "resource error": the paired device is not connected.
pub const ERR_RESOURCE: u8 = 0x09;
/// HID++ 2.0 error code "busy".
pub const ERR_BUSY: u8 = 0x08;

/// Effect ids reported by 0x8070 GetZoneEffectInfo (`LOGITECH_DEVICE_LED_*` and the HID++ 2.0 controller's table).
pub mod effect {
    /// Off.
    pub const OFF: u16 = 0x0000;
    /// Static / fixed colour ("ON" in the legacy controller).
    pub const STATIC: u16 = 0x0001;
    /// Colour cycle / spectrum.
    pub const CYCLE: u16 = 0x0003;
    /// Colour wave.
    pub const WAVE: u16 = 0x0004;
    /// Audio visualizer (G560 factory default).
    pub const AUDIO: u16 = 0x0007;
    /// Breathing.
    pub const BREATHING: u16 = 0x000A;
    /// Ripple.
    pub const RIPPLE: u16 = 0x000B;
    /// Decomposition.
    pub const DECOMPOSITION: u16 = 0x000E;
    /// Signature 1.
    pub const SIGNATURE1: u16 = 0x000F;
    /// Signature 2.
    pub const SIGNATURE2: u16 = 0x0010;
    /// Colour cycle with saturation.
    pub const CYCLE_SAT: u16 = 0x0015;
    /// Colour wave with saturation.
    pub const WAVE_SAT: u16 = 0x0016;
    /// Ripple with saturation.
    pub const RIPPLE_SAT: u16 = 0x0017;
}

/// Builds a long (report 0x11) HID++ 2.0 request: `11 <device> <feature> <function|SW_ID> <params, zero padded>`.
///
/// `function` is the function id already shifted into the high nibble (0x00, 0x10, ... 0xF0); its low nibble is
/// replaced by [`SW_ID`]. At most 16 parameter bytes are used.
pub fn long_request(device_index: u8, feature_index: u8, function: u8, params: &[u8]) -> [u8; LONG_LEN] {
    let mut buf = [0u8; LONG_LEN];
    buf[0] = REPORT_LONG;
    buf[1] = device_index;
    buf[2] = feature_index;
    buf[3] = (function & 0xF0) | SW_ID;
    let n = params.len().min(LONG_LEN - 4);
    buf[4..4 + n].copy_from_slice(&params[..n]);
    buf
}

/// Builds a short (report 0x10) HID++ 1.0 register request: `10 <device> <sub_id> <address> <p0> <p1> <p2>`.
pub fn short_register(device_index: u8, sub_id: u8, address: u8, params: [u8; 3]) -> [u8; SHORT_LEN] {
    [
        REPORT_SHORT,
        device_index,
        sub_id,
        address,
        params[0],
        params[1],
        params[2],
    ]
}

/// IRoot GetFeature(page) request.
pub fn feature_lookup_request(device_index: u8, page: u16) -> [u8; LONG_LEN] {
    long_request(device_index, ROOT_INDEX, ROOT_GET_FEATURE, &page.to_be_bytes())
}

/// 0x8070 SetSWControl request: `[enabled, enabled]` (the reference sends `[1, 1]` to claim, `[0, 0]` to release).
pub fn sw_control_request(device_index: u8, feature_index: u8, enabled: bool) -> [u8; LONG_LEN] {
    let en = u8::from(enabled);
    long_request(device_index, feature_index, fp8070::SET_SW_CTL, &[en, en])
}

/// How a received frame relates to an outstanding request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// The reply to our request; the payload starts at byte 4.
    Ours,
    /// An error answer to our request, with its error code.
    Error(u8),
    /// Anything else: another program's traffic, a notification, an input report.
    Foreign,
}

fn is_hidpp_frame(frame: &[u8]) -> bool {
    frame.len() >= 4 && matches!(frame[0], REPORT_SHORT | REPORT_LONG | REPORT_VERY_LONG)
}

/// Classifies `frame` against a HID++ 2.0 request to `device_index` / `feature_index` / `function` (high nibble).
///
/// Ours: same device index, same feature index, same function byte including [`SW_ID`]. Error: a HID++ 2.0 error
/// (`<id> <dev> FF <feature> <function|SW_ID> <code>`) or HID++ 1.0 error (`<id> <dev> 8F ...`, same layout) naming
/// exactly our request. Everything else is foreign.
pub fn match_feature_reply(frame: &[u8], device_index: u8, feature_index: u8, function: u8) -> Reply {
    if !is_hidpp_frame(frame) || frame[1] != device_index {
        return Reply::Foreign;
    }
    let fn_byte = (function & 0xF0) | SW_ID;
    if matches!(frame[2], ERROR_HIDPP20 | ERROR_HIDPP10) && frame.len() >= 6 {
        return if frame[3] == feature_index && frame[4] == fn_byte {
            Reply::Error(frame[5])
        } else {
            Reply::Foreign
        };
    }
    if frame[2] == feature_index && frame[3] == fn_byte {
        Reply::Ours
    } else {
        Reply::Foreign
    }
}

/// Classifies `frame` against a HID++ 1.0 register request (`sub_id` 0x80/0x81, register `address`).
///
/// Ours: `<id> <dev> <sub_id> <address> ...`. Error: `<id> <dev> 8F <sub_id> <address> <code>`.
pub fn match_register_reply(frame: &[u8], device_index: u8, sub_id: u8, address: u8) -> Reply {
    if !is_hidpp_frame(frame) || frame[1] != device_index {
        return Reply::Foreign;
    }
    if frame[2] == ERROR_HIDPP10 && frame.len() >= 6 {
        return if frame[3] == sub_id && frame[4] == address {
            Reply::Error(frame[5])
        } else {
            Reply::Foreign
        };
    }
    if frame[2] == sub_id && frame[3] == address {
        Reply::Ours
    } else {
        Reply::Foreign
    }
}

/// Human-readable name of a HID++ 2.0 error code.
pub fn error_name(code: u8) -> &'static str {
    match code {
        0x00 => "no error",
        0x01 => "unknown",
        0x02 => "invalid argument",
        0x03 => "out of range",
        0x04 => "hardware error",
        0x05 => "logitech internal",
        0x06 => "invalid feature index",
        0x07 => "invalid function id",
        0x08 => "busy",
        0x09 => "unsupported / resource error",
        _ => "unrecognised",
    }
}

/// Maps a HID++ error code to a [`DeviceError`]. Busy is retryable and maps to [`DeviceError::Timeout`].
pub fn hidpp_error(code: u8) -> DeviceError {
    if code == ERR_BUSY {
        DeviceError::Timeout
    } else {
        DeviceError::Protocol(format!("HID++ error {code:#04x} ({})", error_name(code)))
    }
}

/// The 16 payload bytes after the 4-byte header of a reply, zero padded.
pub type Payload = [u8; 16];

/// Reads until `matcher` accepts a frame, the timeout expires or [`FOREIGN_BUDGET`] foreign frames were skipped.
///
/// The outer error is a transport error or [`DeviceError::Timeout`]; the inner one is the HID++ error code the
/// device answered our request with.
pub fn read_reply(
    dev: &mut dyn HidTransport,
    timeout: Duration,
    matcher: &dyn Fn(&[u8]) -> Reply,
) -> Result<Result<Payload, u8>, DeviceError> {
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 64];
    for _ in 0..=FOREIGN_BUDGET {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(DeviceError::Timeout);
        }
        let n = dev.read_timeout(&mut buf, remaining)?;
        let frame = &buf[..n.min(buf.len())];
        match matcher(frame) {
            Reply::Ours => {
                let mut payload = [0u8; 16];
                let body = frame.get(4..).unwrap_or(&[]);
                let len = body.len().min(16);
                payload[..len].copy_from_slice(&body[..len]);
                return Ok(Ok(payload));
            }
            Reply::Error(code) => return Ok(Err(code)),
            Reply::Foreign => {
                tracing::trace!(target: "cc_drivers::logitech", frame = %cc_hid::hex(frame), "skipping foreign frame");
            }
        }
    }
    Err(DeviceError::Timeout)
}

/// [`read_reply`] with a HID++ error answer mapped through [`hidpp_error`].
pub fn read_matching(
    dev: &mut dyn HidTransport,
    timeout: Duration,
    matcher: &dyn Fn(&[u8]) -> Reply,
) -> Result<Payload, DeviceError> {
    read_reply(dev, timeout, matcher)?.map_err(hidpp_error)
}

/// Writes a HID++ 1.0 register request and reads its matched reply (outer: transport/timeout, inner: error code).
pub fn register_request(
    dev: &mut dyn HidTransport,
    device_index: u8,
    sub_id: u8,
    address: u8,
    params: [u8; 3],
) -> Result<Result<Payload, u8>, DeviceError> {
    dev.write(&short_register(device_index, sub_id, address, params))?;
    read_reply(dev, REPLY_TIMEOUT, &|f| {
        match_register_reply(f, device_index, sub_id, address)
    })
}

/// Drops up to `limit` queued input reports without waiting. Stops at the first empty read.
pub fn flush(dev: &mut dyn HidTransport, limit: usize) -> Result<usize, DeviceError> {
    let mut buf = [0u8; 64];
    for flushed in 0..limit {
        match dev.read_timeout(&mut buf, Duration::ZERO) {
            Ok(_) => {}
            Err(HidError::Timeout) => return Ok(flushed),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(limit)
}

/// One HID++ 2.0 endpoint: an opened long-report collection plus the device index requests are addressed to.
pub struct Link {
    dev: Box<dyn HidTransport>,
    index: u8,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

/// One 0x8070 effect slot of a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectInfo {
    /// Effect index within the zone (what SetZoneEffect takes).
    pub index: u8,
    /// Effect type id (see [`effect`]).
    pub id: u16,
    /// Capability bitmask as reported.
    pub caps: u16,
    /// Default period in milliseconds as reported.
    pub period: u16,
}

/// One 0x8070 lighting zone ("cluster" in the HID++ 2.0 controller, "LED" in the legacy one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneInfo {
    /// Zone index on the wire.
    pub index: u8,
    /// Location code (1 primary, 2 logo, ...).
    pub location: u16,
    /// Effects the zone offers, in device order.
    pub effects: Vec<EffectInfo>,
}

impl ZoneInfo {
    /// The first effect of type `id`.
    pub fn effect_by_id(&self, id: u16) -> Option<EffectInfo> {
        self.effects.iter().copied().find(|e| e.id == id)
    }

    /// The effect at wire index `index`.
    pub fn effect_by_index(&self, index: u8) -> Option<EffectInfo> {
        self.effects.iter().copied().find(|e| e.index == index)
    }
}

/// What a zone was showing before we touched it: the raw 0x8070 GetEffect reply, if the device answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneEffectState {
    /// Zone index on the wire.
    pub zone: u8,
    /// Raw 16-byte reply payload, `None` if the device rejected the read.
    pub raw: Option<Payload>,
}

/// Upper bound for counts read from the device; anything larger is treated as a garbled reply.
const MAX_ZONES: u8 = 32;
const MAX_EFFECTS: u8 = 32;

/// Whether a device name is plausible: 1..=64 printable ASCII characters (`NameLooksReal`).
pub fn name_looks_real(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|c| (0x20..=0x7E).contains(&c))
}

impl Link {
    /// Wraps an opened collection; requests go to `index`.
    pub fn new(dev: Box<dyn HidTransport>, index: u8) -> Self {
        Self { dev, index }
    }

    /// Device index requests are addressed to.
    pub fn index(&self) -> u8 {
        self.index
    }

    /// The underlying transport.
    pub fn transport(&mut self) -> &mut dyn HidTransport {
        self.dev.as_mut()
    }

    /// Sends one request and waits for its matched reply.
    pub fn request(
        &mut self,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Payload, DeviceError> {
        let req = long_request(self.index, feature_index, function, params);
        self.dev.write(&req)?;
        let index = self.index;
        read_matching(self.dev.as_mut(), REPLY_TIMEOUT, &|f| {
            match_feature_reply(f, index, feature_index, function)
        })
    }

    /// Drops queued reports (see [`flush`]).
    pub fn flush(&mut self) -> Result<usize, DeviceError> {
        flush(self.dev.as_mut(), FLUSH_LIMIT)
    }

    /// IRoot GetFeature: the runtime index of `page`, 0 if the device does not have it.
    pub fn feature_index(&mut self, page: u16) -> Result<u8, DeviceError> {
        Ok(self.request(ROOT_INDEX, ROOT_GET_FEATURE, &page.to_be_bytes())?[0])
    }

    /// DeviceNameType: the device's name, `None` if the reply is implausible (`DiscoverDeviceName`).
    pub fn device_name(&mut self, feature: u8) -> Result<Option<String>, DeviceError> {
        let len = usize::from(self.request(feature, NAME_GET_COUNT, &[])?[0]);
        if len == 0 || len > 64 {
            return Ok(None);
        }
        let mut name = Vec::with_capacity(len);
        let mut offset = 0;
        while offset < len {
            let chunk = self.request(feature, NAME_GET_NAME, &[offset as u8])?;
            let take = (len - offset).min(16);
            name.extend_from_slice(&chunk[..take]);
            offset += 16;
        }
        while name.last() == Some(&0) {
            name.pop();
        }
        Ok(String::from_utf8(name).ok().filter(|n| name_looks_real(n)))
    }

    /// DeviceNameType: the Logitech device type byte (`LOGITECH_DEVICE_TYPE_*`).
    pub fn device_type(&mut self, feature: u8) -> Result<u8, DeviceError> {
        Ok(self.request(feature, NAME_GET_TYPE, &[])?[0])
    }

    /// 0x8070 discovery (`DiscoverRGBEffects` / `getRGBconfig`): zones, their locations and effects.
    ///
    /// A zone or effect the device rejects with a HID++ error is skipped, as the reference does; transport errors
    /// and timeouts abort.
    pub fn discover_8070(&mut self, feature: u8) -> Result<Vec<ZoneInfo>, DeviceError> {
        let count = self.request(feature, fp8070::INFO, &[])?[0];
        if count > MAX_ZONES {
            return Err(DeviceError::Protocol(format!(
                "implausible 0x8070 zone count {count}"
            )));
        }
        let mut zones = Vec::with_capacity(usize::from(count));
        for z in 0..count {
            let info = match self.request(feature, fp8070::ZONE_INFO, &[z, 0x00]) {
                Ok(p) => p,
                Err(DeviceError::Protocol(e)) => {
                    tracing::debug!(target: "cc_drivers::logitech", zone = z, error = %e, "zone info rejected");
                    continue;
                }
                Err(e) => return Err(e),
            };
            let location = u16::from_be_bytes([info[1], info[2]]);
            let effect_count = info[3];
            if effect_count > MAX_EFFECTS {
                return Err(DeviceError::Protocol(format!(
                    "implausible 0x8070 effect count {effect_count} in zone {z}"
                )));
            }
            let mut effects = Vec::with_capacity(usize::from(effect_count));
            for e in 0..effect_count {
                let p = match self.request(feature, fp8070::EFFECT_INFO, &[z, e, 0x00, 0x00]) {
                    Ok(p) => p,
                    Err(DeviceError::Protocol(err)) => {
                        tracing::debug!(target: "cc_drivers::logitech", zone = z, effect = e, error = %err, "effect info rejected");
                        continue;
                    }
                    Err(err) => return Err(err),
                };
                effects.push(EffectInfo {
                    index: e,
                    id: u16::from_be_bytes([p[2], p[3]]),
                    caps: u16::from_be_bytes([p[4], p[5]]),
                    period: u16::from_be_bytes([p[6], p[7]]),
                });
            }
            zones.push(ZoneInfo {
                index: z,
                location,
                effects,
            });
        }
        Ok(zones)
    }

    /// 0x8070 GetEffect per zone: what each zone shows right now, before anything is claimed. Read-only; a rejected
    /// read yields `raw: None`, a timeout or transport error aborts.
    pub fn read_effect_state(
        &mut self,
        feature: u8,
        zones: &[ZoneInfo],
    ) -> Result<Vec<ZoneEffectState>, DeviceError> {
        let mut out = Vec::with_capacity(zones.len());
        for z in zones {
            let raw = match self.request(feature, fp8070::GET_EFFECT, &[z.index]) {
                Ok(p) => Some(p),
                Err(DeviceError::Protocol(_)) => None,
                Err(e) => return Err(e),
            };
            out.push(ZoneEffectState { zone: z.index, raw });
        }
        Ok(out)
    }

    /// 0x8070 SetSWControl(enabled, enabled).
    pub fn set_sw_control(&mut self, feature: u8, enabled: bool) -> Result<(), DeviceError> {
        let en = u8::from(enabled);
        self.request(feature, fp8070::SET_SW_CTL, &[en, en]).map(|_| ())
    }

    /// 0x8070 SetZoneEffect with 16 prepared parameter bytes.
    pub fn set_zone_effect(&mut self, feature: u8, params: &[u8; 16]) -> Result<(), DeviceError> {
        self.request(feature, fp8070::SET_EFFECT, params).map(|_| ())
    }
}

/// Builds a long reply frame for tests and documentation: `11 <dev> <feature> <function|SW_ID> <payload>`.
pub fn long_reply(device_index: u8, feature_index: u8, function: u8, payload: &[u8]) -> Vec<u8> {
    long_request(device_index, feature_index, function, payload).to_vec()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cc_hid::FakeTransport;

    #[test]
    fn feature_lookup_is_byte_exact() {
        // SendStandard on Windows always builds a long frame: buf[0]=0x11, buf[1]=device_index (0xFF wired),
        // buf[2]=LOGITECH_HIDPP_PAGE_ROOT_IDX (0), buf[3]=FN_8071_GET_INFO (0) | HIDPP20_SW_ID (7),
        // data = { page >> 8, page & 0xFF } (GetFeatureIndex), rest zero.
        let mut expect = [0u8; 20];
        expect[..6].copy_from_slice(&[0x11, 0xFF, 0x00, 0x07, 0x80, 0x70]);
        assert_eq!(feature_lookup_request(0xFF, 0x8070), expect);
        expect[..6].copy_from_slice(&[0x11, 0x01, 0x00, 0x07, 0x00, 0x05]);
        assert_eq!(feature_lookup_request(0x01, 0x0005), expect);
    }

    #[test]
    fn sw_control_is_byte_exact() {
        // SetSWControl, sw_control_simple branch: data = { en, en }, fn_sw_control = 0x80, | SW_ID.
        let mut claim = [0u8; 20];
        claim[..6].copy_from_slice(&[0x11, 0xFF, 0x04, 0x87, 0x01, 0x01]);
        assert_eq!(sw_control_request(0xFF, 0x04, true), claim);
        let mut release = [0u8; 20];
        release[..6].copy_from_slice(&[0x11, 0xFF, 0x04, 0x87, 0x00, 0x00]);
        assert_eq!(sw_control_request(0xFF, 0x04, false), release);
    }

    #[test]
    fn register_request_is_byte_exact() {
        // getWirelessDevice: get_connected_devices.init(0xFF, 0x81), feature_command = 0x02, data zero.
        assert_eq!(
            short_register(RECEIVER_INDEX, GET_REGISTER, REG_CONNECTION_STATE, [0; 3]),
            [0x10, 0xFF, 0x81, 0x02, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn feature_reply_matching() {
        let ours = long_reply(0xFF, 0x04, 0x30, &[1]);
        assert_eq!(match_feature_reply(&ours, 0xFF, 0x04, 0x30), Reply::Ours);
        // Other software id (G HUB uses its own): foreign.
        let mut other_sw = ours.clone();
        other_sw[3] = 0x31;
        assert_eq!(match_feature_reply(&other_sw, 0xFF, 0x04, 0x30), Reply::Foreign);
        // Other device index, feature or function: foreign.
        assert_eq!(match_feature_reply(&ours, 0x01, 0x04, 0x30), Reply::Foreign);
        assert_eq!(match_feature_reply(&ours, 0xFF, 0x05, 0x30), Reply::Foreign);
        assert_eq!(match_feature_reply(&ours, 0xFF, 0x04, 0x20), Reply::Foreign);
        // Non HID++ input report: foreign.
        assert_eq!(
            match_feature_reply(&[0x01, 0xFF, 0x04, 0x37], 0xFF, 0x04, 0x30),
            Reply::Foreign
        );
        assert_eq!(match_feature_reply(&[0x11], 0xFF, 0x04, 0x30), Reply::Foreign);
        // HID++ 2.0 error for our request.
        let err = [0x11, 0xFF, 0xFF, 0x04, 0x37, 0x02, 0, 0];
        assert_eq!(match_feature_reply(&err, 0xFF, 0x04, 0x30), Reply::Error(0x02));
        // Error for someone else's request.
        let foreign_err = [0x11, 0xFF, 0xFF, 0x04, 0x31, 0x02, 0, 0];
        assert_eq!(
            match_feature_reply(&foreign_err, 0xFF, 0x04, 0x30),
            Reply::Foreign
        );
    }

    #[test]
    fn register_reply_matching() {
        let ok = [0x10, 0xFF, 0x81, 0x02, 0x00, 0x01, 0x00];
        assert_eq!(match_register_reply(&ok, 0xFF, 0x81, 0x02), Reply::Ours);
        assert_eq!(match_register_reply(&ok, 0xFF, 0x81, 0x00), Reply::Foreign);
        let err = [0x10, 0x01, 0x8F, 0x81, 0x02, 0x09, 0x00];
        assert_eq!(match_register_reply(&err, 0x01, 0x81, 0x02), Reply::Error(0x09));
        assert_eq!(match_register_reply(&err, 0xFF, 0x81, 0x02), Reply::Foreign);
    }

    #[test]
    fn request_skips_interleaved_foreign_reports() {
        let req = long_request(0xFF, 0x00, 0x00, &[0x80, 0x70]);
        // G HUB asking the same thing with its own software id (1).
        let mut ghub = long_reply(0xFF, 0x00, 0x00, &[0x09]);
        ghub[3] = 0x01;
        let fake = FakeTransport::builder()
            .write(req.to_vec())
            .read(ghub)
            // A notification from the device (software id 0).
            .read(vec![0x11, 0xFF, 0x04, 0x00, 0x01])
            // An input report that is not HID++.
            .read(vec![0x03, 0x00, 0x10])
            // Our reply.
            .read(long_reply(0xFF, 0x00, 0x00, &[0x04, 0x00, 0x01]))
            .build();
        let mut link = Link::new(Box::new(fake.clone()), 0xFF);
        assert_eq!(link.feature_index(0x8070).expect("lookup"), 0x04);
        fake.finish().expect("script consumed");
    }

    #[test]
    fn request_reports_errors_and_timeouts() {
        let req = long_request(0xFF, 0x04, 0x30, &[]);
        let fake = FakeTransport::builder()
            .write(req.to_vec())
            .read(vec![0x11, 0xFF, 0xFF, 0x04, 0x37, 0x02])
            .write(req.to_vec())
            .read(vec![0x11, 0xFF, 0xFF, 0x04, 0x37, 0x08])
            .write(req.to_vec())
            .read_timeout()
            .build();
        let mut link = Link::new(Box::new(fake.clone()), 0xFF);
        assert!(matches!(
            link.request(0x04, 0x30, &[]),
            Err(DeviceError::Protocol(_))
        ));
        assert_eq!(link.request(0x04, 0x30, &[]), Err(DeviceError::Timeout));
        assert_eq!(link.request(0x04, 0x30, &[]), Err(DeviceError::Timeout));
        fake.finish().expect("script consumed");
    }

    #[test]
    fn foreign_budget_bounds_the_read() {
        let req = long_request(0xFF, 0x04, 0x30, &[]);
        let mut b = FakeTransport::builder().write(req.to_vec());
        for _ in 0..=FOREIGN_BUDGET {
            b = b.read(vec![0x11, 0xFF, 0x04, 0x00]);
        }
        let fake = b.build();
        let mut link = Link::new(Box::new(fake.clone()), 0xFF);
        assert_eq!(link.request(0x04, 0x30, &[]), Err(DeviceError::Timeout));
        fake.finish().expect("script consumed");
    }

    #[test]
    fn names() {
        assert!(name_looks_real("G560 Gaming Speaker"));
        assert!(!name_looks_real(""));
        assert!(!name_looks_real("bad\u{1}"));
    }
}
