//! The Illumination V1 [`Controller`], generic over the two NvAPI calls it needs so tests can run without the DLL.

use std::sync::{Arc, Mutex};

use cc_core::{
    Bus, ColorMode, Controller, DeviceDescription, DeviceError, DeviceId, DeviceInfo, DeviceKind, Led, Mode,
    ModeCaps, ModeState, Rgb, Zone, ZoneKind as CoreZoneKind,
};
use cc_nvapi::{GpuHandle, NvApi, NvApiError, ZoneControlParams, ZoneKind};

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

/// The device description for a GPU whose `GetControl` reported `zones`: one single-LED zone per NvAPI zone (LED
/// name "Entire Zone"), modes "Off" and "Direct" (brightness 0..=100, per-LED colour), as the reference exposes.
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

/// NVIDIA Illumination V1 controller.
///
/// Lifecycle: constructed from the params of a successful `GetControl` (the hardware state to restore). The first
/// write after construction or after [`release`](Controller::release) re-reads `GetControl` if the kept state was
/// already written back, so `SetControl` is never called without a preceding successful `GetControl`. Every write
/// sends the whole params struct (all zones), and is skipped when it equals the last params sent.
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
    /// Brightness of the "Direct" mode.
    brightness: u8,
}

impl<A: IllumApi> IllumController<A> {
    /// Builds a controller from the params `api.get_control()` just returned.
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
        let kinds: Vec<ZoneKind> = current.zones.iter().map(|z| z.kind).collect();
        if kinds.is_empty() {
            return Err(DeviceError::Protocol("GPU reports no illumination zones".into()));
        }
        let desc = describe(id, name, location, &kinds);
        desc.validate()
            .map_err(|e| DeviceError::Protocol(format!("invalid description: {e}")))?;
        Ok(Self {
            desc,
            api,
            kinds,
            treats_rgbw_as_rgb,
            working: current.clone(),
            initial: Some(current),
            last_sent: None,
            dirty: false,
            brightness: mapping::DEFAULT_BRIGHTNESS,
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
        self.working = current.clone();
        self.initial = Some(current);
        self.last_sent = None;
        self.dirty = false;
        Ok(())
    }

    fn set_zone_color(&mut self, zone: usize, color: Rgb) {
        let kind = self.kinds[zone];
        if let Some(c) = mapping::direct_color(kind, color, self.brightness, self.treats_rgbw_as_rgb) {
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
            self.set_zone_color(zone, color);
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
        self.set_zone_color(zone, *color);
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
            self.brightness = u8::try_from(brightness)
                .unwrap_or(mapping::MAX_BRIGHTNESS)
                .min(mapping::MAX_BRIGHTNESS);
            for (zone, &color) in state.colors.iter().enumerate() {
                self.set_zone_color(zone, color);
            }
        }
        self.flush()
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        let Some(initial) = self.initial.as_ref() else {
            return Ok(());
        };
        if self.dirty {
            self.api.set_control(initial).map_err(device_error)?;
        }
        self.initial = None;
        self.last_sent = None;
        self.dirty = false;
        Ok(())
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

    #[test]
    fn brightness_comes_from_direct_mode_state() {
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
                    vec![single(30), single(30), single(0)]
                )),
            ]
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
}
