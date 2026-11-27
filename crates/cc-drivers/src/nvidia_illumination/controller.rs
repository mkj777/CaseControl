// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! The Illumination V1 [`Controller`], generic over the two NvAPI calls it needs so tests can run without the DLL.

use std::sync::{Arc, Mutex};

use cc_core::{
    Bus, ColorMode, Controller, DeviceDescription, DeviceError, DeviceId, DeviceInfo, DeviceKind, Led, Mode,
    ModeCaps, ModeState, OwnLighting, OwnSource, Rgb, Zone, ZoneKind as CoreZoneKind,
};
use cc_nvapi::{CtrlMode, GpuHandle, NvApi, NvApiError, ZoneColor, ZoneControlParams, ZoneKind};

use super::mapping;

/// Mode id of "Off" (upstream `NVIDIA_ILLUMINATION_OFF`).
pub const MODE_OFF: u32 = 0;
/// Mode id of "Direct" (upstream `NVIDIA_ILLUMINATION_DIRECT`), the direct mode.
pub const MODE_DIRECT: u32 = 1;

/// The two illumination calls the controller makes. Implemented for a real GPU by [`NvApiGpu`] and by fakes in tests.
pub trait IllumApi {
    /// `NvAPI_GPU_ClientIllumZonesGetControl` with `bDefault = 0` (the currently active values).
    fn get_control(&mut self) -> Result<ZoneControlParams, NvApiError>;
    /// `NvAPI_GPU_ClientIllumZonesSetControl`.
    fn set_control(&mut self, params: &ZoneControlParams) -> Result<(), NvApiError>;
}

/// One GPU behind a shared NvAPI handle.
///
/// All controllers from one detection pass share one [`NvApi`] through a mutex: `NvApi` is `Send` but not `Sync`,
/// GPU handles are only meaningful to the instance that enumerated them, and the mutex also serialises (and,
/// through `NvApi`'s own pacing, spaces by [`cc_nvapi::ILLUM_CALL_GAP`]) the calls of several GPUs.
#[derive(Debug, Clone)]
pub struct NvApiGpu {
    nvapi: Arc<Mutex<NvApi>>,
    gpu: GpuHandle,
}

impl NvApiGpu {
    /// Wraps a GPU handle obtained from `nvapi`.
    #[must_use]
    pub fn new(nvapi: Arc<Mutex<NvApi>>, gpu: GpuHandle) -> Self {
        Self { nvapi, gpu }
    }

    fn with<T>(&self, f: impl FnOnce(&NvApi) -> T) -> T {
        // A panic while holding the lock cannot leave NvApi in a broken state (it holds no invariants across
        // calls besides the pacing timestamp), so a poisoned lock is simply taken over.
        let guard = self
            .nvapi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&guard)
    }
}

impl IllumApi for NvApiGpu {
    fn get_control(&mut self) -> Result<ZoneControlParams, NvApiError> {
        // NvApi sleeps only the rest of ILLUM_CALL_GAP since its previous illumination call.
        self.with(|n| n.illum_zones_get_control(&self.gpu, false))
    }

    fn set_control(&mut self, params: &ZoneControlParams) -> Result<(), NvApiError> {
        self.with(|n| n.illum_zones_set_control(&self.gpu, params))
    }
}

/// Maps an NvAPI error to the device error the engine understands.
pub(crate) fn device_error(e: NvApiError) -> DeviceError {
    match e {
        NvApiError::Unsupported(msg) => DeviceError::Protocol(msg),
        other => DeviceError::Transport(other.to_string()),
    }
}

/// Upstream zone names: `"<index> - <type>"` with the type names from `RGBController_NVIDIAIlluminationV1::SetupZones`.
fn zone_name(index: usize, kind: ZoneKind) -> String {
    let ty = match kind {
        ZoneKind::Rgb => "RGB",
        ZoneKind::Rgbw => "RGBW",
        ZoneKind::ColorFixed => "FIXED COLOR",
        ZoneKind::SingleColor => "SINGLE COLOR",
        // Upstream has no name for these (its map lookup yields a null string); ours is explicit.
        ZoneKind::Invalid | ZoneKind::Other(_) => "UNKNOWN",
    };
    format!("{index} - {ty}")
}

/// Whether NvAPI zones of type `kind` show a fixed colour and take only a brightness (`SINGLE_COLOR`, `COLOR_FIXED`).
#[must_use]
pub fn is_brightness_only(kind: ZoneKind) -> bool {
    matches!(kind, ZoneKind::SingleColor | ZoneKind::ColorFixed)
}

/// The device description for a GPU whose `GetControl` reported `zones`: one single-LED zone per NvAPI zone (LED
/// name "Entire Zone"), modes "Off" and "Direct" (brightness 0..=100, per-LED colour), as the reference exposes.
/// `SINGLE_COLOR` and `COLOR_FIXED` zones are marked [`brightness_only`](cc_core::Zone::brightness_only).
#[must_use]
pub fn describe(id: DeviceId, name: &str, location: String, zones: &[ZoneKind]) -> DeviceDescription {
    DeviceDescription {
        info: DeviceInfo {
            id,
            name: name.to_owned(),
            vendor: "NVIDIA".into(),
            kind: DeviceKind::Gpu,
            bus: Bus::NvApi,
            location,
            serial: None,
            firmware: None,
        },
        zones: zones
            .iter()
            .enumerate()
            .map(|(i, &kind)| Zone {
                name: zone_name(i, kind),
                kind: CoreZoneKind::Single,
                start: i,
                len: 1,
                resizable: None,
                brightness_only: is_brightness_only(kind),
            })
            .collect(),
        leds: zones
            .iter()
            .map(|_| Led {
                name: "Entire Zone".into(),
            })
            .collect(),
        modes: vec![
            Mode {
                id: MODE_OFF,
                name: "Off".into(),
                caps: ModeCaps::empty(),
                color_mode: ColorMode::None,
                speed: None,
                brightness: None,
                colors: None,
            },
            Mode {
                id: MODE_DIRECT,
                name: "Direct".into(),
                caps: ModeCaps::BRIGHTNESS | ModeCaps::PER_LED_COLOR,
                color_mode: ColorMode::PerLed,
                speed: None,
                brightness: Some(0..=u32::from(mapping::MAX_BRIGHTNESS)),
                colors: None,
            },
        ],
        direct_mode: Some(MODE_DIRECT),
        keepalive: None,
    }
}

/// The lighting `params` (a `GetControl` result) shows, as a state of `desc`: "Direct" when every zone with a known
/// type is in manual mode, with each zone's colour scaled by its brightness relative to the brightest zone and that
/// brightness as the mode's brightness (so sending it back reproduces every zone), or "Off" when every such zone is
/// dark. `None` while any zone runs an animation (piecewise linear), whose look this driver cannot express.
#[must_use]
pub fn readback_state(desc: &DeviceDescription, params: &ZoneControlParams) -> Option<ModeState> {
    let mut zones = Vec::with_capacity(params.zones.len());
    for zone in &params.zones {
        if matches!(zone.kind, ZoneKind::Invalid | ZoneKind::Other(_)) {
            zones.push(None);
            continue;
        }
        if zone.mode != cc_nvapi::CtrlMode::Manual {
            return None;
        }
        zones.push(Some(mapping::current_color(zone)?));
    }
    let max = zones
        .iter()
        .flatten()
        .filter(|(c, _)| *c != Rgb::BLACK)
        .map(|(_, b)| *b)
        .max()
        .unwrap_or(0);
    let state = if max == 0 {
        ModeState {
            mode: MODE_OFF,
            speed: None,
            brightness: None,
            direction: None,
            colors: Vec::new(),
        }
    } else {
        let colors = zones
            .iter()
            .map(|z| match z {
                Some((c, b)) => {
                    // In 0..=255: channel * b / max with b <= max.
                    let ch = |v: u8| {
                        u8::try_from((u32::from(v) * u32::from(*b) + u32::from(max) / 2) / u32::from(max))
                            .unwrap_or(v)
                    };
                    Rgb::new(ch(c.r), ch(c.g), ch(c.b))
                }
                None => Rgb::BLACK,
            })
            .collect();
        ModeState {
            mode: MODE_DIRECT,
            speed: None,
            brightness: Some(u32::from(max.min(mapping::MAX_BRIGHTNESS))),
            direction: None,
            colors,
        }
    };
    desc.validate_state(&state).is_ok().then_some(state)
}

/// Version byte of an own-lighting record (`encode_own`).
pub const OWN_VERSION: u8 = 1;
/// Bytes per zone in an own-lighting record: zone type (`u32`, little-endian), colour tag, R, G, B, W, brightness.
const OWN_ZONE_LEN: usize = 10;

/// The lighting `params` shows, as bytes for the engine to remember as the GPU's own lighting (ADR 0001 #38):
/// `OWN_VERSION`, the zone count, then per zone its type and its manual colour (tag 0 RGB, 1 RGBW, 2 brightness
/// only). `None` unless every zone is in manual mode with a decoded colour (an animation cannot be remembered).
#[must_use]
pub fn encode_own(params: &ZoneControlParams) -> Option<Vec<u8>> {
    let count = u8::try_from(params.zones.len()).ok()?;
    let mut out = vec![OWN_VERSION, count];
    for zone in &params.zones {
        if zone.mode != CtrlMode::Manual {
            return None;
        }
        let record = match zone.manual {
            ZoneColor::Rgb { r, g, b, brightness } => [0, r, g, b, 0, brightness],
            ZoneColor::Rgbw {
                r,
                g,
                b,
                w,
                brightness,
            } => [1, r, g, b, w, brightness],
            ZoneColor::Single { brightness } => [2, 0, 0, 0, 0, brightness],
            ZoneColor::Other => return None,
        };
        out.extend_from_slice(&zone.kind.to_raw().to_le_bytes());
        out.extend_from_slice(&record);
    }
    Some(out)
}

/// `current` (a fresh `GetControl` result) with every zone set to the manual colour of an `encode_own` record, or
/// `None` when the bytes do not fit: wrong version or length, another zone count, a zone type that differs from the
/// GPU's, a colour that does not fit its zone type, or a brightness above 100. Everything else is `current`'s.
#[must_use]
pub fn decode_own(bytes: &[u8], current: &ZoneControlParams) -> Option<ZoneControlParams> {
    let (&[version, count], records) = bytes.split_first_chunk::<2>()?;
    if version != OWN_VERSION
        || usize::from(count) != current.zones.len()
        || records.len() != OWN_ZONE_LEN * current.zones.len()
    {
        return None;
    }
    let mut out = current.clone();
    for (record, zone) in records.as_chunks::<OWN_ZONE_LEN>().0.iter().zip(&mut out.zones) {
        let (kind, color) = record.split_first_chunk::<4>()?;
        if u32::from_le_bytes(*kind) != zone.kind.to_raw() {
            return None;
        }
        let &[tag, r, g, b, w, brightness] = color else {
            return None;
        };
        if brightness > mapping::MAX_BRIGHTNESS {
            return None;
        }
        zone.manual = match (tag, zone.kind) {
            (0, ZoneKind::Rgb) if w == 0 => ZoneColor::Rgb { r, g, b, brightness },
            (1, ZoneKind::Rgbw) => ZoneColor::Rgbw {
                r,
                g,
                b,
                w,
                brightness,
            },
            (2, ZoneKind::SingleColor | ZoneKind::ColorFixed) if [r, g, b, w] == [0; 4] => {
                ZoneColor::Single { brightness }
            }
            _ => return None,
        };
        zone.mode = CtrlMode::Manual;
    }
    Some(out)
}

/// NVIDIA Illumination V1 controller.
///
/// Lifecycle: constructed from the params of a successful `GetControl` (the hardware state to restore). The first
/// write after construction or after [`release`](Controller::release) re-reads `GetControl` if the kept state was
/// already written back, so `SetControl` is never called without a preceding successful `GetControl`. Every write
/// sends the whole params struct (all zones), and is skipped when it equals the last params sent.
///
/// Own lighting (ADR 0001 #38, the shared [`OwnLighting`] seam): built without the engine's "unreleased" marker, the
/// params read at construction are the GPU's own lighting and are reported (`encode_own`). Built with the marker,
/// they are CaseControl's state: a remembered own lighting that fits the GPU (`decode_own`) is what `release()`
/// writes back (even if nothing was sent) and what `hardware_mode()` reports, and
/// [`Controller::shows_own_lighting`] stays `false` until that release; without one, `release()` writes back what it
/// read and the marker stays.
#[derive(Debug)]
pub struct IllumController<A> {
    desc: DeviceDescription,
    api: A,
    kinds: Vec<ZoneKind>,
    treats_rgbw_as_rgb: bool,
    /// Hardware state to restore on release. `None` after a successful release until control is taken again.
    initial: Option<ZoneControlParams>,
    /// Params being built for the next write (starts as a copy of `initial`).
    working: ZoneControlParams,
    /// Last params sent with `SetControl` since taking control.
    last_sent: Option<ZoneControlParams>,
    /// Whether a `SetControl` was attempted since `initial` was read, i.e. whether release must write it back.
    dirty: bool,
    /// The lighting the GPU showed on its own when control was last taken (what `release()` writes back).
    hardware: Option<ModeState>,
    /// What `release()` writes back and where it came from.
    own: OwnLighting<ZoneControlParams>,
}

impl<A: IllumApi> IllumController<A> {
    /// Builds a controller from the params `api.get_control()` just returned, for a GPU without the "unreleased"
    /// marker (see [`new_with_stored`](Self::new_with_stored)).
    ///
    /// # Errors
    /// [`DeviceError::Protocol`] if the GPU reports no illumination zones or the description fails validation.
    pub fn new(
        api: A,
        id: DeviceId,
        name: &str,
        location: String,
        treats_rgbw_as_rgb: bool,
        current: ZoneControlParams,
    ) -> Result<Self, DeviceError> {
        Self::new_with_stored(api, id, name, location, treats_rgbw_as_rgb, current, None, false)
    }

    /// [`new`](Self::new) with what the engine remembers (ADR 0001 #38): `unreleased`, CaseControl wrote to the GPU
    /// and did not hand it back, so `current` is its state; `stored`, the GPU's last good own lighting
    /// (`encode_own` bytes), used only when `unreleased` and when it fits (`decode_own`).
    ///
    /// # Errors
    /// As [`new`](Self::new).
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_stored(
        api: A,
        id: DeviceId,
        name: &str,
        location: String,
        treats_rgbw_as_rgb: bool,
        current: ZoneControlParams,
        stored: Option<&[u8]>,
        unreleased: bool,
    ) -> Result<Self, DeviceError> {
        let kinds: Vec<ZoneKind> = current.zones.iter().map(|z| z.kind).collect();
        if kinds.is_empty() {
            return Err(DeviceError::Protocol("GPU reports no illumination zones".into()));
        }
        let desc = describe(id, name, location, &kinds);
        desc.validate()
            .map_err(|e| DeviceError::Protocol(format!("invalid description: {e}")))?;
        let remembered = if unreleased {
            let params = stored.and_then(|b| decode_own(b, &current));
            if params.is_none() && stored.is_some() {
                tracing::warn!(id = %desc.info.id, "stored own lighting does not fit the GPU; ignoring it");
            }
            params
        } else {
            None
        };
        let own = OwnLighting::at_open(current.clone(), true, remembered, unreleased);
        if own.source() == OwnSource::LeftByUs {
            tracing::warn!(
                id = %desc.info.id,
                "GPU shows a state CaseControl left and no own lighting is stored for it; release() writes that state back"
            );
        }
        let hardware = readback_state(&desc, own.handback());
        Ok(Self {
            hardware,
            desc,
            api,
            kinds,
            treats_rgbw_as_rgb,
            working: current,
            initial: Some(own.handback().clone()),
            last_sent: None,
            dirty: false,
            own,
        })
    }

    /// The hardware state that [`release`](Controller::release) writes back, if control is held.
    #[must_use]
    pub fn initial_params(&self) -> Option<&ZoneControlParams> {
        self.initial.as_ref()
    }

    /// Reads the current hardware state if it is not held yet (after a release).
    fn take_control(&mut self) -> Result<(), DeviceError> {
        if self.initial.is_some() {
            return Ok(());
        }
        let current = self.api.get_control().map_err(device_error)?;
        let kinds: Vec<ZoneKind> = current.zones.iter().map(|z| z.kind).collect();
        if kinds != self.kinds {
            return Err(DeviceError::Protocol(format!(
                "illumination zones changed from {:?} to {kinds:?}",
                self.kinds
            )));
        }
        self.own.retaken(current.clone());
        self.hardware = readback_state(&self.desc, &current);
        self.working = current.clone();
        self.initial = Some(current);
        self.last_sent = None;
        self.dirty = false;
        Ok(())
    }

    /// Writes `color` into zone `zone` at mode brightness `brightness`. Direct colours (`set_direct`,
    /// `set_zone_direct`) always use [`mapping::MAX_BRIGHTNESS`], so a brightness-only zone's level comes from the
    /// colour's intensity alone (the engine applies device and master brightness to the colours already); only a
    /// "Direct" mode state (`set_mode`) carries its own brightness, and it does not stick to later direct writes.
    fn set_zone_color(&mut self, zone: usize, color: Rgb, brightness: u8) {
        let kind = self.kinds[zone];
        if let Some(c) = mapping::direct_color(kind, color, brightness, self.treats_rgbw_as_rgb) {
            mapping::apply(&mut self.working.zones[zone], c);
        }
    }

    /// Sends `working` unless it equals what was sent last.
    fn flush(&mut self) -> Result<(), DeviceError> {
        if self.last_sent.as_ref() == Some(&self.working) {
            return Ok(());
        }
        self.dirty = true;
        self.api.set_control(&self.working).map_err(device_error)?;
        self.last_sent = Some(self.working.clone());
        Ok(())
    }
}

impl<A: IllumApi + Send> Controller for IllumController<A> {
    fn description(&self) -> &DeviceDescription {
        &self.desc
    }

    fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError> {
        if colors.len() != self.kinds.len() {
            return Err(DeviceError::InvalidArgument(format!(
                "{} colors for {} LEDs",
                colors.len(),
                self.kinds.len()
            )));
        }
        self.take_control()?;
        for (zone, &color) in colors.iter().enumerate() {
            self.set_zone_color(zone, color, mapping::MAX_BRIGHTNESS);
        }
        self.flush()
    }

    fn set_zone_direct(&mut self, zone: usize, colors: &[Rgb]) -> Result<(), DeviceError> {
        if zone >= self.kinds.len() {
            return Err(DeviceError::InvalidArgument(format!("no zone {zone}")));
        }
        let [color] = colors else {
            return Err(DeviceError::InvalidArgument(format!(
                "zone {zone} has 1 LED, got {} colors",
                colors.len()
            )));
        };
        self.take_control()?;
        self.set_zone_color(zone, *color, mapping::MAX_BRIGHTNESS);
        self.flush()
    }

    fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError> {
        self.desc
            .validate_state(state)
            .map_err(|e| DeviceError::InvalidArgument(e.to_string()))?;
        self.take_control()?;
        if state.mode == MODE_OFF {
            for (zone, &kind) in self.kinds.iter().enumerate() {
                if let Some(c) = mapping::off_color(kind) {
                    mapping::apply(&mut self.working.zones[zone], c);
                }
            }
        } else {
            let brightness = state.brightness.unwrap_or(u32::from(mapping::DEFAULT_BRIGHTNESS));
            let brightness = u8::try_from(brightness)
                .unwrap_or(mapping::MAX_BRIGHTNESS)
                .min(mapping::MAX_BRIGHTNESS);
            for (zone, &color) in state.colors.iter().enumerate() {
                self.set_zone_color(zone, color, brightness);
            }
        }
        self.flush()
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        let Some(initial) = self.initial.as_ref() else {
            return Ok(());
        };
        // A remembered own lighting the GPU does not show yet is written back even if nothing was sent.
        if self.dirty || self.own.handback_pending() {
            self.api.set_control(initial).map_err(device_error)?;
        }
        self.own.handed_back();
        self.initial = None;
        self.last_sent = None;
        self.dirty = false;
        Ok(())
    }

    fn hardware_mode(&self) -> Option<ModeState> {
        self.hardware.clone()
    }

    /// The params `release()` writes back (`encode_own`) when they are the GPU's own lighting.
    fn own_lighting(&self) -> Option<Vec<u8>> {
        self.own.own().and_then(encode_own)
    }

    /// `false` while built with the "unreleased" marker until a release wrote the remembered own lighting back, and
    /// for good when none was remembered.
    fn shows_own_lighting(&self) -> bool {
        self.own.shows_own_lighting()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_nvapi::{CtrlMode, ZoneColor, ZoneControl, raw};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Get,
        Set(ZoneControlParams),
    }

    /// Records calls; `get_control` returns `state`, `set_control` stores into it (like the driver would).
    #[derive(Debug, Clone, Default)]
    struct Fake {
        log: Arc<Mutex<Vec<Call>>>,
        state: Arc<Mutex<Option<ZoneControlParams>>>,
        fail_set: bool,
    }

    impl Fake {
        fn with_state(p: ZoneControlParams) -> Self {
            let f = Self::default();
            *f.state.lock().expect("lock") = Some(p);
            f
        }
        fn calls(&self) -> Vec<Call> {
            self.log.lock().expect("lock").clone()
        }
        fn clear(&self) {
            self.log.lock().expect("lock").clear();
        }
    }

    impl IllumApi for Fake {
        fn get_control(&mut self) -> Result<ZoneControlParams, NvApiError> {
            self.log.lock().expect("lock").push(Call::Get);
            Ok(self.state.lock().expect("lock").clone().expect("state set"))
        }
        fn set_control(&mut self, params: &ZoneControlParams) -> Result<(), NvApiError> {
            self.log.lock().expect("lock").push(Call::Set(params.clone()));
            if self.fail_set {
                return Err(NvApiError::Unsupported("fake failure".into()));
            }
            *self.state.lock().expect("lock") = Some(params.clone());
            Ok(())
        }
    }

    /// The RTX 5080 FE as `dump_illum` saw it: three SINGLE_COLOR zones in manual mode at 50/60/60 %.
    fn rtx5080_params() -> ZoneControlParams {
        ZoneControlParams::new(
            false,
            [50, 60, 60]
                .into_iter()
                .map(|b| ZoneControl::new(ZoneKind::SingleColor, ZoneColor::Single { brightness: b }))
                .collect(),
        )
    }

    fn controller(fake: &Fake) -> IllumController<Fake> {
        let current = fake.clone().get_control().expect("fake get");
        fake.clear();
        IllumController::new(
            fake.clone(),
            DeviceId::nvapi(0x2C02_10DE, 0x2095_10DE),
            "NVIDIA GeForce RTX 5080 FE",
            "NvAPI GPU 0".into(),
            false,
            current,
        )
        .expect("controller")
    }

    fn single(brightness: u8) -> ZoneControl {
        ZoneControl::new(ZoneKind::SingleColor, ZoneColor::Single { brightness })
    }

    #[test]
    fn hardware_mode_reads_the_manual_zones() {
        // Single-colour zones at 50, 60, 60: Direct at brightness 60; zone 0 white scaled by 50 / 60 (255 * 50 / 60 =
        // 212.5 -> 213), which the driver maps back to brightness (213 * 60 * 2 + 255) / 510 = 50.
        let fake = Fake::with_state(rtx5080_params());
        let c = controller(&fake);
        let state = c.hardware_mode().expect("manual zones decode");
        assert_eq!(state.mode, MODE_DIRECT);
        assert_eq!(state.brightness, Some(60));
        assert_eq!(
            state.colors,
            vec![Rgb::new(213, 213, 213), Rgb::WHITE, Rgb::WHITE]
        );
        assert_eq!(mapping::scaled_brightness(state.colors[0], 60), 50);

        // All dark: Off.
        let dark = ZoneControlParams::new(false, vec![single(0), single(0), single(0)]);
        let off = readback_state(c.description(), &dark).expect("dark decodes");
        assert_eq!(off.mode, MODE_OFF);

        // An animated zone: unknown.
        let mut animated = rtx5080_params();
        animated.zones[1].mode = CtrlMode::PiecewiseLinear;
        assert_eq!(readback_state(c.description(), &animated), None);
    }

    #[test]
    fn description_matches_reference() {
        let fake = Fake::with_state(rtx5080_params());
        let c = controller(&fake);
        let d = c.description();
        assert_eq!(d.validate(), Ok(()));
        assert_eq!(d.info.id.as_str(), "nvapi:10de:2c02:10de:2095");
        assert_eq!(d.info.bus, Bus::NvApi);
        assert_eq!(d.info.kind, DeviceKind::Gpu);
        assert_eq!(d.info.vendor, "NVIDIA");
        let names: Vec<_> = d.zones.iter().map(|z| z.name.as_str()).collect();
        assert_eq!(
            names,
            ["0 - SINGLE COLOR", "1 - SINGLE COLOR", "2 - SINGLE COLOR"]
        );
        assert!(d.leds.iter().all(|l| l.name == "Entire Zone"));
        // SINGLE COLOR zones show a fixed colour: brightness only, so the whole device is.
        assert!(d.zones.iter().all(|z| z.brightness_only));
        assert!(d.is_brightness_only());
        assert_eq!(d.direct_mode, Some(MODE_DIRECT));
        assert_eq!(d.mode(MODE_DIRECT).expect("direct").brightness, Some(0..=100));
        assert_eq!(d.mode(MODE_OFF).expect("off").color_mode, ColorMode::None);
        assert!(fake.calls().is_empty(), "construction makes no calls");
    }

    #[test]
    fn mixed_zone_names() {
        let d = describe(
            DeviceId::nvapi(0, 0),
            "x",
            "y".into(),
            &[
                ZoneKind::Rgb,
                ZoneKind::Rgbw,
                ZoneKind::ColorFixed,
                ZoneKind::Other(7),
            ],
        );
        let names: Vec<_> = d.zones.iter().map(|z| z.name.as_str()).collect();
        assert_eq!(names, ["0 - RGB", "1 - RGBW", "2 - FIXED COLOR", "3 - UNKNOWN"]);
        let fixed: Vec<_> = d.zones.iter().map(|z| z.brightness_only).collect();
        assert_eq!(fixed, [false, false, true, false]);
        assert!(!d.is_brightness_only());
        assert_eq!(d.validate(), Ok(()));
    }

    #[test]
    fn no_zones_is_an_error() {
        let fake = Fake::default();
        let r = IllumController::new(
            fake,
            DeviceId::nvapi(0, 0),
            "x",
            "y".into(),
            false,
            ZoneControlParams::new(false, vec![]),
        );
        assert!(matches!(r, Err(DeviceError::Protocol(_))));
    }

    #[test]
    fn set_direct_sends_whole_struct_and_skips_repeats() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        let red = Rgb::new(255, 0, 0);
        c.set_direct(&[red, Rgb::BLACK, red]).expect("set");
        let expected = ZoneControlParams::new(false, vec![single(100), single(0), single(100)]);
        assert_eq!(fake.calls(), vec![Call::Set(expected.clone())]);

        // Same frame again, and a different colour that maps to the same brightness: no call.
        c.set_direct(&[red, Rgb::BLACK, red]).expect("set");
        c.set_direct(&[Rgb::WHITE, Rgb::BLACK, Rgb::new(0, 0, 255)])
            .expect("set");
        assert_eq!(fake.calls().len(), 1, "no change, no call");

        c.set_direct(&[red, red, red]).expect("set");
        assert_eq!(fake.calls().len(), 2);
    }

    /// Byte-level check of what reaches the driver for the 5080 shape. Layout from NVFC/nvapi.h:
    /// header = version u32 (72012) | flags u32 (bit0 bDefault) | numIllumZonesControl u32 | rsvd[64];
    /// zone = type u32 | ctrlMode u32 | data union, SINGLE_COLOR manual params start with brightnessPct u8.
    #[test]
    fn set_direct_bytes() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        c.set_direct(&[Rgb::new(0, 255, 0), Rgb::BLACK, Rgb::new(9, 9, 9)])
            .expect("set");
        let Call::Set(sent) = &fake.calls()[0] else {
            panic!("expected a set")
        };
        let bytes = sent.to_raw().expect("encodes").to_bytes();
        assert_eq!(&bytes[0..4], &72012u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0], "bDefault = 0: active values");
        assert_eq!(&bytes[8..12], &3u32.to_le_bytes());
        let z = |i: usize| raw::ZONE_CONTROL_PARAMS_V1_HEADER_SIZE + i * raw::ZONE_CONTROL_V1_SIZE;
        // type SINGLE_COLOR = 4, ctrlMode MANUAL = 0, brightnessPct.
        assert_eq!(&bytes[z(0)..z(0) + 9], &[4, 0, 0, 0, 0, 0, 0, 0, 100]);
        assert_eq!(&bytes[z(1)..z(1) + 9], &[4, 0, 0, 0, 0, 0, 0, 0, 0]);
        // (9, 9, 9): round(9 * 100 / 255) = round(3.53) = 4.
        assert_eq!(&bytes[z(2)..z(2) + 9], &[4, 0, 0, 0, 0, 0, 0, 0, 4]);
    }

    /// Per-LED white levels (ADR 0001 #41): a slider value per zone is stored as the grey `Rgb::white_level(p)`, and
    /// the driver writes that zone's own brightness `p` from it, independently of the other zones.
    #[test]
    fn per_zone_white_levels_reach_each_zone() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        for p in 0..=100u8 {
            // Zone 0 follows the slider, zones 1 and 2 stay at fixed, different levels.
            let colors = [Rgb::white_level(p), Rgb::white_level(25), Rgb::white_level(80)];
            c.set_direct(&colors).expect("set");
            let state = fake.state.lock().expect("lock").clone().expect("state");
            assert_eq!(state.zones[0], single(p), "slider {p} %");
            assert_eq!(state.zones[1], single(25));
            assert_eq!(state.zones[2], single(80));
            // What the profile stores reads back as the same slider value.
            assert_eq!(colors[0].level_percent(), p);
        }

        // One zone at a time (set_zone): only that zone changes.
        c.set_zone_direct(1, &[Rgb::white_level(7)]).expect("zone");
        let state = fake.state.lock().expect("lock").clone().expect("state");
        assert_eq!(
            state.zones,
            vec![single(100), single(7), single(80)],
            "zone 1 alone changed"
        );

        // A device brightness of 50 % scales the stored grey before it reaches the driver (the engine's job): 80 %
        // grey (204) at 50 % is 102, written as 40 %.
        let scaled = Rgb::new(102, 102, 102);
        c.set_zone_direct(2, &[scaled]).expect("zone");
        let state = fake.state.lock().expect("lock").clone().expect("state");
        assert_eq!(state.zones[2], single(40));
    }

    /// A "Direct" mode state's brightness applies to that state only; later direct colours are not dimmed by it.
    #[test]
    fn mode_brightness_does_not_stick_to_direct_writes() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        let state = ModeState {
            mode: MODE_DIRECT,
            speed: None,
            brightness: Some(30),
            direction: None,
            colors: vec![Rgb::WHITE; 3],
        };
        c.set_mode(&state).expect("mode");
        c.set_direct(&[Rgb::WHITE, Rgb::WHITE, Rgb::BLACK]).expect("set");
        assert_eq!(
            fake.calls(),
            vec![
                Call::Set(ZoneControlParams::new(
                    false,
                    vec![single(30), single(30), single(30)]
                )),
                Call::Set(ZoneControlParams::new(
                    false,
                    vec![single(100), single(100), single(0)]
                )),
            ]
        );
    }

    /// Taken over with its own lighting (readback "Direct" at brightness 60), then a level slider at 100 %: the zone
    /// shows 100 %, not 60 %.
    #[test]
    fn white_level_after_readback_takeover_is_not_capped() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        let own = c.hardware_mode().expect("readback");
        assert_eq!(own.brightness, Some(60));
        c.set_mode(&own).expect("takeover");
        assert_eq!(
            fake.state.lock().expect("lock").clone().expect("state").zones,
            vec![single(50), single(60), single(60)]
        );
        c.set_zone_direct(0, &[Rgb::white_level(100)]).expect("level");
        c.set_direct(&[Rgb::white_level(100), Rgb::white_level(40), Rgb::white_level(60)])
            .expect("levels");
        assert_eq!(
            fake.state.lock().expect("lock").clone().expect("state").zones,
            vec![single(100), single(40), single(60)]
        );
    }

    #[test]
    fn off_mode_zeroes_every_zone() {
        let params = ZoneControlParams::new(
            false,
            vec![
                ZoneControl::new(
                    ZoneKind::Rgbw,
                    ZoneColor::Rgbw {
                        r: 1,
                        g: 2,
                        b: 3,
                        w: 4,
                        brightness: 5,
                    },
                ),
                single(60),
            ],
        );
        let fake = Fake::with_state(params);
        let mut c = controller(&fake);
        c.set_mode(&ModeState {
            mode: MODE_OFF,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![],
        })
        .expect("off");
        let off = ZoneControlParams::new(
            false,
            vec![
                ZoneControl::new(
                    ZoneKind::Rgbw,
                    ZoneColor::Rgbw {
                        r: 0,
                        g: 0,
                        b: 0,
                        w: 0,
                        brightness: 0,
                    },
                ),
                single(0),
            ],
        );
        assert_eq!(fake.calls(), vec![Call::Set(off)]);
    }

    #[test]
    fn invalid_arguments_make_no_calls() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        assert!(matches!(
            c.set_direct(&[Rgb::WHITE; 2]),
            Err(DeviceError::InvalidArgument(_))
        ));
        assert!(matches!(
            c.set_zone_direct(3, &[Rgb::WHITE]),
            Err(DeviceError::InvalidArgument(_))
        ));
        assert!(matches!(
            c.set_zone_direct(0, &[Rgb::WHITE, Rgb::WHITE]),
            Err(DeviceError::InvalidArgument(_))
        ));
        let bad = ModeState {
            mode: MODE_DIRECT,
            speed: None,
            brightness: Some(101),
            direction: None,
            colors: vec![Rgb::WHITE; 3],
        };
        assert!(matches!(c.set_mode(&bad), Err(DeviceError::InvalidArgument(_))));
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn set_zone_direct_changes_one_zone() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        c.set_zone_direct(1, &[Rgb::BLACK]).expect("zone");
        assert_eq!(
            fake.calls(),
            vec![Call::Set(ZoneControlParams::new(
                false,
                vec![single(50), single(0), single(60)]
            ))]
        );
    }

    #[test]
    fn release_restores_initial_params_once() {
        let initial = rtx5080_params();
        let fake = Fake::with_state(initial.clone());
        let mut c = controller(&fake);
        c.set_direct(&[Rgb::WHITE; 3]).expect("set");
        c.release().expect("release");
        c.release().expect("second release is harmless");
        let calls = fake.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], Call::Set(initial));
    }

    #[test]
    fn release_without_writes_makes_no_call() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        c.release().expect("release");
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn release_then_set_direct_rereads_before_writing() {
        let initial = rtx5080_params();
        let fake = Fake::with_state(initial.clone());
        let mut c = controller(&fake);
        c.set_direct(&[Rgb::WHITE; 3]).expect("set");
        c.release().expect("release");
        // Another tool changes the lights while we are released.
        let changed = ZoneControlParams::new(false, vec![single(10), single(20), single(30)]);
        *fake.state.lock().expect("lock") = Some(changed.clone());
        fake.clear();

        // Same frame as before release: must still be sent (control was handed back), after a fresh GetControl.
        c.set_direct(&[Rgb::WHITE; 3]).expect("set again");
        let white = ZoneControlParams::new(false, vec![single(100), single(100), single(100)]);
        assert_eq!(fake.calls(), vec![Call::Get, Call::Set(white)]);
        assert_eq!(c.initial_params(), Some(&changed));

        c.release().expect("release");
        assert_eq!(
            fake.calls().last(),
            Some(&Call::Set(changed)),
            "restores the re-read state"
        );
    }

    #[test]
    fn retake_rejects_changed_zone_layout() {
        let fake = Fake::with_state(rtx5080_params());
        let mut c = controller(&fake);
        c.release().expect("release");
        *fake.state.lock().expect("lock") = Some(ZoneControlParams::new(false, vec![single(1)]));
        fake.clear();
        assert!(matches!(
            c.set_direct(&[Rgb::WHITE; 3]),
            Err(DeviceError::Protocol(_))
        ));
        assert_eq!(
            fake.calls(),
            vec![Call::Get],
            "no SetControl after a mismatching GetControl"
        );
    }

    #[test]
    fn failed_write_still_restores_on_release() {
        let initial = rtx5080_params();
        let mut fake = Fake::with_state(initial.clone());
        fake.fail_set = true;
        let mut c = controller(&fake);
        assert!(c.set_direct(&[Rgb::WHITE; 3]).is_err());
        // The failed frame is retried, not skipped.
        assert!(c.set_direct(&[Rgb::WHITE; 3]).is_err());
        assert_eq!(fake.calls().len(), 2);
        assert!(c.release().is_err(), "release error is reported");
        assert_eq!(fake.calls().last(), Some(&Call::Set(initial)));
        assert!(c.initial_params().is_some(), "kept for a retry");
    }

    #[test]
    fn piecewise_zone_is_switched_to_manual() {
        let mut z = ZoneControl::new(ZoneKind::Rgb, ZoneColor::Other);
        z.mode = CtrlMode::PiecewiseLinear;
        let fake = Fake::with_state(ZoneControlParams::new(false, vec![z]));
        let mut c = controller(&fake);
        c.set_direct(&[Rgb::new(1, 2, 3)]).expect("set");
        let Call::Set(sent) = &fake.calls()[0] else {
            panic!("expected a set")
        };
        assert_eq!(sent.zones[0].mode, CtrlMode::Manual);
        assert_eq!(
            sent.zones[0].manual,
            ZoneColor::Rgb {
                r: 1,
                g: 2,
                b: 3,
                brightness: 100
            }
        );
        c.release().expect("release");
        let Call::Set(restored) = &fake.calls()[1] else {
            panic!("expected a set")
        };
        assert_eq!(restored.zones[0].mode, CtrlMode::PiecewiseLinear);
    }

    #[test]
    fn unknown_zone_is_never_touched() {
        let unknown = ZoneControl::new(ZoneKind::Other(9), ZoneColor::Other);
        let fake = Fake::with_state(ZoneControlParams::new(false, vec![unknown, single(5)]));
        let mut c = controller(&fake);
        c.set_direct(&[Rgb::WHITE, Rgb::WHITE]).expect("set");
        let Call::Set(sent) = &fake.calls()[0] else {
            panic!("expected a set")
        };
        assert_eq!(sent.zones[0], unknown);
        assert_eq!(sent.zones[1], single(100));
    }

    #[test]
    fn controller_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<IllumController<NvApiGpu>>();
    }

    fn marked(fake: &Fake, stored: Option<&[u8]>) -> IllumController<Fake> {
        let current = fake.clone().get_control().expect("fake get");
        fake.clear();
        IllumController::new_with_stored(
            fake.clone(),
            DeviceId::nvapi(0x2C02_10DE, 0x2095_10DE),
            "NVIDIA GeForce RTX 5080 FE",
            "NvAPI GPU 0".into(),
            false,
            current,
            stored,
            true,
        )
        .expect("controller")
    }

    #[test]
    fn own_lighting_encoding_round_trips_and_rejects_what_does_not_fit() {
        let params = rtx5080_params();
        let bytes = encode_own(&params).expect("manual zones encode");
        // Version, count, then per zone: type 4 (SINGLE_COLOR, LE), tag 2, R G B W 0, brightness.
        assert_eq!(&bytes[..12], &[1, 3, 4, 0, 0, 0, 2, 0, 0, 0, 0, 50]);
        assert_eq!(bytes.len(), 2 + 3 * 10);
        let dark = ZoneControlParams::new(false, vec![single(0), single(0), single(0)]);
        assert_eq!(decode_own(&bytes, &dark), Some(params.clone()));
        let bad = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = bytes.clone();
            f(&mut b);
            decode_own(&b, &dark).is_none()
        };
        assert!(bad(&|b| b[0] = 2), "version");
        assert!(bad(&|b| b[1] = 2), "zone count");
        assert!(bad(&|b| b.truncate(20)), "length");
        assert!(bad(&|b| b[2] = 1), "zone type differs");
        assert!(bad(&|b| b[6] = 0), "RGB colour on a single-colour zone");
        assert!(bad(&|b| b[11] = 101), "brightness above 100");
        let mut animated = rtx5080_params();
        animated.zones[1].mode = CtrlMode::PiecewiseLinear;
        assert_eq!(encode_own(&animated), None, "an animation cannot be remembered");
    }

    #[test]
    fn unmarked_gpu_reports_its_readback_as_own_lighting() {
        let fake = Fake::with_state(rtx5080_params());
        let c = controller(&fake);
        assert_eq!(c.own_lighting(), encode_own(&rtx5080_params()));
        assert!(c.shows_own_lighting());
    }

    #[test]
    fn marked_gpu_keeps_the_marker_and_release_writes_the_stored_own_lighting() {
        // CaseControl left the zones at 100 %; the remembered own lighting is 50/60/60.
        let stored = encode_own(&rtx5080_params()).expect("encodes");
        let left = ZoneControlParams::new(false, vec![single(100), single(100), single(100)]);
        let fake = Fake::with_state(left);
        let mut c = marked(&fake, Some(&stored));
        assert!(!c.shows_own_lighting(), "the marker stays until a release");
        assert_eq!(c.own_lighting(), Some(stored.clone()));
        assert_eq!(c.hardware_mode().and_then(|s| s.brightness), Some(60));
        // Nothing was sent, yet the release writes the stored state back.
        c.release().expect("release");
        assert_eq!(fake.calls(), vec![Call::Set(rtx5080_params())]);
        assert!(c.shows_own_lighting());
    }

    #[test]
    fn marked_gpu_without_stored_state_writes_back_what_it_read_and_keeps_the_marker() {
        let left = ZoneControlParams::new(false, vec![single(100), single(100), single(100)]);
        let fake = Fake::with_state(left.clone());
        let mut c = marked(&fake, Some(&[1, 2, 3]));
        assert_eq!(c.own_lighting(), None);
        assert!(!c.shows_own_lighting());
        c.set_direct(&[Rgb::BLACK; 3]).expect("set");
        c.release().expect("release");
        assert_eq!(fake.calls().last(), Some(&Call::Set(left)));
        assert!(!c.shows_own_lighting(), "still CaseControl's state");
        // Taking control again keeps it that way.
        c.set_direct(&[Rgb::WHITE; 3]).expect("set");
        assert_eq!(c.own_lighting(), None);
    }
}
