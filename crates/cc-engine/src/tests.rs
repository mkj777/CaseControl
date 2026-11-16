//! Engine tests against fake backends, controllers and hot-plug sources. No hardware is touched.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cc_config::DeviceSetting;
use cc_core::{
    Backend, Bus, ColorMode, Controller, DetectContext, Detected, DeviceDescription, DeviceError, DeviceId,
    DeviceInfo, DeviceKind, EngineEvent, HotplugAction, HotplugEvent, HotplugHandle, HotplugSender, Led,
    Mode, ModeCaps, ModeState, Rgb, Zone, ZoneKind,
};
use parking_lot::Mutex;

use crate::engine::{EngineConfig, EngineHandle, start_with};
use crate::error::EngineError;
use crate::hotplug::HotplugSource;
use crate::view::DeviceStatus;
use crate::worker::Timing;

const RED: Rgb = Rgb::new(255, 0, 0);
const GREEN: Rgb = Rgb::new(0, 255, 0);
const BLUE: Rgb = Rgb::new(0, 0, 255);

// ---------------------------------------------------------------------------------------------------------------
// Fakes

/// Every device call of every fake controller, in order.
type Log = Arc<Mutex<Vec<(Instant, String)>>>;

#[derive(Debug, Default, Clone)]
struct Behavior {
    /// Non-fatal transport errors on every `set_direct`.
    fail_direct: bool,
    /// `Disconnected` on `set_direct`.
    fatal_direct: bool,
    /// How long `set_mode` blocks.
    mode_delay: Duration,
    /// `release` fails with a transport error.
    fail_release: bool,
}

struct FakeCtrl {
    desc: DeviceDescription,
    log: Log,
    behavior: Arc<Mutex<Behavior>>,
}

impl FakeCtrl {
    fn log(&self, what: String) {
        self.log
            .lock()
            .push((Instant::now(), format!("{} {what}", self.desc.info.id)));
    }
}

impl Controller for FakeCtrl {
    fn description(&self) -> &DeviceDescription {
        &self.desc
    }

    fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError> {
        let hex: Vec<String> = colors.iter().map(Rgb::to_string).collect();
        self.log(format!("direct {}", hex.join(",")));
        let b = self.behavior.lock().clone();
        if b.fatal_direct {
            return Err(DeviceError::Disconnected);
        }
        if b.fail_direct {
            return Err(DeviceError::Transport("write failed".into()));
        }
        assert_eq!(colors.len(), self.desc.led_count());
        Ok(())
    }

    fn set_zone_direct(&mut self, zone: usize, _colors: &[Rgb]) -> Result<(), DeviceError> {
        self.log(format!("zone {zone}"));
        Ok(())
    }

    fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError> {
        let delay = self.behavior.lock().mode_delay;
        std::thread::sleep(delay);
        self.log(format!("mode {}", state.mode));
        Ok(())
    }

    fn resize_zone(&mut self, zone: usize, len: usize) -> Result<(), DeviceError> {
        self.log(format!("resize {zone} {len}"));
        let old = self.desc.zones[zone].len;
        self.desc.zones[zone].len = len;
        for z in &mut self.desc.zones[zone + 1..] {
            z.start = z.start + len - old;
        }
        let total = self.desc.zones.iter().map(|z| z.len).sum();
        self.desc.leds = (0..total)
            .map(|n| Led {
                name: format!("LED {n}"),
            })
            .collect();
        Ok(())
    }

    fn keepalive(&mut self) -> Result<(), DeviceError> {
        self.log("keepalive".into());
        Ok(())
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        self.log("release".into());
        if self.behavior.lock().fail_release {
            return Err(DeviceError::Transport("release failed".into()));
        }
        Ok(())
    }
}

/// A device that may be "plugged in" to a [`FakeBackend`].
#[derive(Clone)]
struct Spec {
    desc: DeviceDescription,
    behavior: Arc<Mutex<Behavior>>,
}

fn spec(id: DeviceId, name: &str, kind: DeviceKind) -> Spec {
    Spec {
        desc: description(id, name, kind),
        behavior: Arc::default(),
    }
}

/// 7 LEDs: "Board" (1), "ARGB 1" (3, resizable 0..=120), "Strip" (3). Modes: 0 Direct, 1 Static (1 color).
fn description(id: DeviceId, name: &str, kind: DeviceKind) -> DeviceDescription {
    let zone = |name: &str, kind, start, len, resizable| Zone {
        name: name.into(),
        kind,
        start,
        len,
        resizable,
    };
    DeviceDescription {
        info: DeviceInfo {
            location: format!(r"\\?\HID#{id}#Col01"),
            id,
            name: name.into(),
            vendor: "Test".into(),
            kind,
            bus: Bus::Hid,
            serial: None,
            firmware: None,
        },
        zones: vec![
            zone("Board", ZoneKind::Single, 0, 1, None),
            zone("ARGB 1", ZoneKind::Linear, 1, 3, Some(0..=120)),
            zone("Strip", ZoneKind::Linear, 4, 3, None),
        ],
        leds: (0..7)
            .map(|n| Led {
                name: format!("LED {n}"),
            })
            .collect(),
        modes: vec![
            Mode {
                id: 0,
                name: "Direct".into(),
                caps: ModeCaps::PER_LED_COLOR,
                color_mode: ColorMode::PerLed,
                speed: None,
                brightness: None,
                colors: None,
            },
            Mode {
                id: 1,
                name: "Static".into(),
                caps: ModeCaps::MODE_SPECIFIC_COLOR,
                color_mode: ColorMode::ModeSpecific,
                speed: None,
                brightness: None,
                colors: Some(1..=1),
            },
        ],
        direct_mode: Some(0),
        keepalive: None,
    }
}

struct FakeBackend {
    family: &'static str,
    bus: Bus,
    plugged: Arc<Mutex<Vec<Spec>>>,
    failures: Vec<(String, DeviceError)>,
    log: Log,
    contexts: Arc<Mutex<Vec<DetectContext>>>,
}

impl FakeBackend {
    fn new(family: &'static str, specs: Vec<Spec>, log: &Log) -> Self {
        Self {
            family,
            bus: Bus::Hid,
            plugged: Arc::new(Mutex::new(specs)),
            failures: Vec::new(),
            log: Arc::clone(log),
            contexts: Arc::default(),
        }
    }
}

impl Backend for FakeBackend {
    fn name(&self) -> &'static str {
        self.family
    }

    fn bus(&self) -> Bus {
        self.bus
    }

    fn detect(&self, ctx: &DetectContext) -> Vec<Detected> {
        self.contexts.lock().push(ctx.clone());
        if !ctx.is_enabled(self.family) {
            return Vec::new();
        }
        let mut out: Vec<Detected> = self
            .plugged
            .lock()
            .iter()
            .filter(|s| !ctx.is_skipped(&s.desc.info.id))
            .map(|s| {
                Detected::Device(Box::new(FakeCtrl {
                    desc: s.desc.clone(),
                    log: Arc::clone(&self.log),
                    behavior: Arc::clone(&s.behavior),
                }))
            })
            .collect();
        out.extend(self.failures.iter().map(|(loc, e)| Detected::Failed {
            family: self.family.into(),
            location: loc.clone(),
            error: e.clone(),
        }));
        out
    }

    fn subscribe_hotplug(&self, _tx: HotplugSender) -> Option<Box<dyn HotplugHandle>> {
        None
    }
}

/// Hot-plug source whose sender the test keeps for injecting events.
#[derive(Clone, Default)]
struct FakeHotplug {
    tx: Arc<Mutex<Option<HotplugSender>>>,
    stopped: Arc<Mutex<bool>>,
    /// What `present_paths` reports; `None` makes it fail.
    present: Arc<Mutex<Option<Vec<String>>>>,
}

struct FakeHotplugHandle(FakeHotplug);

impl HotplugHandle for FakeHotplugHandle {
    fn stop(self: Box<Self>) {
        *self.0.stopped.lock() = true;
        self.0.tx.lock().take();
    }
}

impl HotplugSource for FakeHotplug {
    fn watch(&mut self, tx: HotplugSender) -> Result<Box<dyn HotplugHandle>, String> {
        *self.tx.lock() = Some(tx);
        Ok(Box::new(FakeHotplugHandle(self.clone())))
    }

    fn present_paths(&self) -> Result<Vec<String>, String> {
        self.present
            .lock()
            .clone()
            .ok_or_else(|| "enumeration failed".into())
    }
}

impl FakeHotplug {
    fn inject(&self, action: HotplugAction, path: &str) {
        self.inject_with(action, path, 0x9999);
    }

    fn inject_with(&self, action: HotplugAction, path: &str, pid: u16) {
        let tx = self.tx.lock().clone().expect("hot-plug running");
        tx.send(HotplugEvent {
            bus: Bus::Hid,
            vid: 0x1234,
            pid,
            path: path.into(),
            action,
        })
        .expect("hot-plug thread alive");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Helpers

fn fast_timing() -> Timing {
    Timing {
        ack_timeout: Duration::from_secs(3),
        backoff_base: Duration::from_millis(2),
        backoff_max: Duration::from_millis(8),
        max_failures: 10,
        hotplug_poll: Duration::from_millis(10),
    }
}

fn cfg(fps: u32) -> EngineConfig {
    EngineConfig {
        frame_rate: fps,
        hotplug: false,
        ..EngineConfig::default()
    }
}

fn id(n: u16) -> DeviceId {
    DeviceId::hid(0x1234, n, None)
}

fn engine(fps: u32, backends: Vec<Box<dyn Backend>>) -> EngineHandle {
    start_with(cfg(fps), backends, None, fast_timing()).expect("engine starts")
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    cond()
}

fn entries(log: &Log, needle: &str) -> Vec<String> {
    log.lock()
        .iter()
        .map(|(_, s)| s.clone())
        .filter(|s| s.contains(needle))
        .collect()
}

fn static_red() -> ModeState {
    ModeState {
        mode: 1,
        speed: None,
        brightness: None,
        direction: None,
        colors: vec![RED],
    }
}

const SECOND: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------------------------------------------
// Tests

#[test]
fn detection_produces_sorted_views_and_failures() {
    let log = Log::default();
    let mut b = FakeBackend::new(
        "fam_a",
        vec![
            spec(id(1), "Zeta keyboard", DeviceKind::Keyboard),
            spec(id(2), "Board", DeviceKind::Motherboard),
            spec(id(3), "Alpha keyboard", DeviceKind::Keyboard),
        ],
        &log,
    );
    b.failures.push(("HID: broken".into(), DeviceError::Timeout));
    let e = engine(30, vec![Box::new(b)]);

    let names: Vec<String> = e.devices().into_iter().map(|v| v.description.info.name).collect();
    assert_eq!(names, ["Board", "Alpha keyboard", "Zeta keyboard"]);

    let v = e.device(&id(1)).unwrap();
    assert_eq!(v.status, DeviceStatus::Active);
    assert_eq!(v.colors, vec![Rgb::BLACK; 7]);
    assert_eq!(v.setting, None);
    assert!(e.device(&id(9)).is_none());

    let f = e.failures();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].family, "fam_a");
    assert_eq!(f[0].location, "HID: broken");
    assert_eq!(f[0].error, DeviceError::Timeout);

    let json = serde_json::to_value(e.device(&id(1)).unwrap()).unwrap();
    assert_eq!(json["status"], serde_json::json!({"state": "active"}));
    assert!(log.lock().is_empty(), "detection alone sends nothing");
}

#[test]
fn disabled_family_is_not_detected_and_invalid_description_is_rejected() {
    let log = Log::default();
    let mut bad = spec(id(2), "Bad", DeviceKind::Light);
    bad.desc.leds.pop(); // zones no longer cover the buffer
    let a = FakeBackend::new("fam_a", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let b = FakeBackend::new("fam_b", vec![bad], &log);
    let mut c = cfg(30);
    c.disabled_families.insert("fam_a".into());
    let e = start_with(c, vec![Box::new(a), Box::new(b)], None, fast_timing()).unwrap();
    assert!(e.devices().is_empty());
    assert_eq!(e.failures().len(), 1);
    assert!(matches!(e.failures()[0].error, DeviceError::Protocol(_)));
}

#[test]
fn bad_frame_rate_is_rejected() {
    let r = start_with(cfg(0), vec![], None, fast_timing());
    assert!(matches!(r, Err(EngineError::InvalidArgument(_))));
    let r = start_with(cfg(61), vec![], None, fast_timing());
    assert!(matches!(r, Err(EngineError::InvalidArgument(_))));
}

#[test]
fn burst_of_set_direct_is_coalesced_and_last_wins() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(10, vec![Box::new(b)]);
    let mut last = Vec::new();
    for i in 0..100u8 {
        last = vec![Rgb::new(i, 0, 0); 7];
        e.set_direct(&id(1), last.clone()).unwrap();
    }
    let want = format!(
        "direct {}",
        last.iter().map(Rgb::to_string).collect::<Vec<_>>().join(",")
    );
    assert!(wait_until(SECOND, || entries(&log, "direct").last()
        == Some(&format!("{} {want}", id(1)))));
    std::thread::sleep(Duration::from_millis(250));
    let frames = entries(&log, "direct");
    assert!(frames.len() <= 3, "{} frames for a burst of 100", frames.len());
    assert!(frames.last().unwrap().ends_with(&want));

    let v = e.device(&id(1)).unwrap();
    assert_eq!(v.colors, last);
    assert_eq!(v.setting, Some(DeviceSetting::Direct { colors: last }));
}

#[test]
fn frames_are_paced_by_frame_rate() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(20, vec![Box::new(b)]);
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(300) {
        e.set_direct(&id(1), vec![RED; 7]).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(60));
    let times: Vec<Instant> = log.lock().iter().map(|(t, _)| *t).collect();
    // 300 ms at 20 fps: about 6-7 frames, never two closer than the 50 ms interval.
    assert!((4..=8).contains(&times.len()), "{} frames", times.len());
    for w in times.windows(2) {
        assert!(w[1] - w[0] >= Duration::from_millis(49), "{:?}", w[1] - w[0]);
    }
}

#[test]
fn set_zone_updates_only_its_slice() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    e.set_zone(&id(1), 1, vec![GREEN; 3]).unwrap();
    let want = vec![RED, GREEN, GREEN, GREEN, RED, RED, RED];
    assert_eq!(e.device(&id(1)).unwrap().colors, want);
    let hex = want.iter().map(Rgb::to_string).collect::<Vec<_>>().join(",");
    assert!(wait_until(SECOND, || entries(&log, "direct")
        .last()
        .is_some_and(|s| s.ends_with(&hex))));

    assert!(matches!(
        e.set_zone(&id(1), 1, vec![GREEN; 2]),
        Err(EngineError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.set_zone(&id(1), 3, vec![GREEN; 1]),
        Err(EngineError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.set_direct(&id(1), vec![GREEN; 6]),
        Err(EngineError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.set_direct(&id(9), vec![GREEN; 7]),
        Err(EngineError::UnknownDevice(_))
    ));
    // No calls through the per-zone driver path: the engine sends the full buffer.
    assert!(entries(&log, " zone ").is_empty());
}

#[test]
fn acked_commands_return_results_and_emit_state_changes() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(30, vec![Box::new(b)]);
    let events = e.subscribe();

    e.set_mode(&id(1), static_red()).unwrap();
    assert_eq!(entries(&log, "mode"), [format!("{} mode 1", id(1))]);
    assert_eq!(
        e.device(&id(1)).unwrap().setting,
        Some(DeviceSetting::Mode { state: static_red() })
    );
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceStateChanged {
            id: id(1),
            state: static_red()
        }
    );

    let mut bad = static_red();
    bad.colors.clear();
    assert!(matches!(
        e.set_mode(&id(1), bad),
        Err(EngineError::InvalidArgument(_))
    ));
    assert_eq!(
        e.save_to_hardware(&id(1)),
        Err(EngineError::Device(DeviceError::Unsupported))
    );
    assert_eq!(e.release(&id(1)), Ok(()));
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Released);
    assert!(events.try_recv().is_err(), "only mode changes emit events");
}

#[test]
fn acked_command_times_out() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().mode_delay = Duration::from_millis(400);
    let b = FakeBackend::new("f", vec![s], &log);
    let timing = Timing {
        ack_timeout: Duration::from_millis(100),
        ..fast_timing()
    };
    let e = start_with(cfg(30), vec![Box::new(b)], None, timing).unwrap();
    let t = Instant::now();
    assert_eq!(e.set_mode(&id(1), static_red()), Err(EngineError::Timeout));
    assert!(t.elapsed() < Duration::from_millis(350));
    assert_eq!(e.device(&id(1)).unwrap().setting, None);
}

#[test]
fn write_errors_back_off_then_fault_after_ten() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().fail_direct = true;
    let b = FakeBackend::new("f", vec![s], &log);
    let e = engine(60, vec![Box::new(b)]);
    let events = e.subscribe();
    e.set_direct(&id(1), vec![RED; 7]).unwrap();

    match events.recv_timeout(2 * SECOND).unwrap() {
        EngineEvent::DeviceFaulted { id: got, error } => {
            assert_eq!(got, id(1));
            assert!(error.contains("10 consecutive"), "{error}");
        }
        other => panic!("unexpected {other:?}"),
    }
    std::thread::sleep(Duration::from_millis(100));
    let attempts: Vec<Instant> = log.lock().iter().map(|(t, _)| *t).collect();
    assert_eq!(attempts.len(), 10, "no retries after Faulted");
    // Gaps grow 2, 4, 8, 8, ... ms: never a tight loop. Allow for the 60 fps pacing floor too.
    for (i, w) in attempts.windows(2).enumerate() {
        let n = u32::try_from(i).unwrap() + 1;
        let min = fast_timing().backoff(n);
        assert!(w[1] - w[0] >= min, "gap {i}: {:?} < {min:?}", w[1] - w[0]);
    }
    assert!(matches!(
        e.device(&id(1)).unwrap().status,
        DeviceStatus::Faulted { .. }
    ));
    assert!(matches!(
        e.set_direct(&id(1), vec![RED; 7]),
        Err(EngineError::Faulted { .. })
    ));
    assert!(matches!(e.release(&id(1)), Err(EngineError::Faulted { .. })));
}

#[test]
fn recovery_resets_failure_count() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().fail_direct = true;
    let behavior = Arc::clone(&s.behavior);
    let b = FakeBackend::new("f", vec![s], &log);
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    assert!(wait_until(SECOND, || entries(&log, "direct").len() >= 5));
    behavior.lock().fail_direct = false;
    // The failed frame is retried after the back-off and succeeds.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Active);
    let n = entries(&log, "direct").len();
    assert!(n < 10, "{n}");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        entries(&log, "direct").len(),
        n,
        "nothing more once the frame went out"
    );
}

#[test]
fn fatal_error_faults_immediately() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().fatal_direct = true;
    let b = FakeBackend::new("f", vec![s], &log);
    let e = engine(60, vec![Box::new(b)]);
    let events = e.subscribe();
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceFaulted {
            id: id(1),
            error: "device disconnected".into()
        }
    );
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(entries(&log, "direct").len(), 1);

    // Shutdown does not talk to a faulted device.
    e.shutdown(SECOND);
    assert!(entries(&log, "release").is_empty());
}

#[test]
fn keepalive_runs_at_its_cadence_while_active() {
    let log = Log::default();
    let mut s = spec(id(1), "A", DeviceKind::Light);
    s.desc.keepalive = Some(Duration::from_millis(50));
    let b = FakeBackend::new("f", vec![s], &log);
    let e = engine(30, vec![Box::new(b)]);

    std::thread::sleep(Duration::from_millis(120));
    assert!(
        entries(&log, "keepalive").is_empty(),
        "no keepalive before control is taken"
    );

    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    std::thread::sleep(Duration::from_millis(330));
    let n = entries(&log, "keepalive").len();
    assert!((5..=7).contains(&n), "{n} keepalives in 330 ms at 50 ms");

    let times: Vec<Instant> = log
        .lock()
        .iter()
        .filter(|(_, s)| s.contains("keepalive"))
        .map(|(t, _)| *t)
        .collect();
    for w in times.windows(2) {
        assert!(w[1] - w[0] >= Duration::from_millis(49));
    }

    e.release(&id(1)).unwrap();
    let after = entries(&log, "keepalive").len();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        entries(&log, "keepalive").len(),
        after,
        "no keepalive while released"
    );
}

#[test]
fn release_then_set_direct_takes_control_again() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    e.release(&id(1)).unwrap();
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Released);
    e.set_direct(&id(1), vec![BLUE; 7]).unwrap();
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Active));
    let all: Vec<String> = log.lock().iter().map(|(_, s)| s.clone()).collect();
    assert!(all[0].ends_with("release"));
    assert!(all[1].contains("direct"));
}

#[test]
fn set_mode_drops_unsent_frame() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(1, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    assert!(wait_until(SECOND, || entries(&log, "direct").len() == 1));
    // The next frame is held back by the 1 s interval; the mode must win over it.
    e.set_direct(&id(1), vec![GREEN; 7]).unwrap();
    e.set_mode(&id(1), static_red()).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(entries(&log, "direct").len(), 1);
    assert_eq!(log.lock().last().unwrap().1, format!("{} mode 1", id(1)));
}

#[test]
fn resize_zone_updates_description_buffer_and_config_sizes_apply_at_detection() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            spec(id(2), "B", DeviceKind::Light),
        ],
        &log,
    );
    let mut c = cfg(60);
    c.zone_sizes.insert(
        id(2),
        BTreeMap::from([("ARGB 1".to_string(), 5), ("Nope".to_string(), 2)]),
    );
    let e = start_with(c, vec![Box::new(b)], None, fast_timing()).unwrap();
    assert_eq!(e.device(&id(2)).unwrap().description.led_count(), 9);
    assert_eq!(e.device(&id(2)).unwrap().colors.len(), 9);

    e.set_direct(&id(1), vec![RED, GREEN, GREEN, BLUE, RED, RED, RED])
        .unwrap();
    e.resize_zone(&id(1), 1, 5).unwrap();
    let v = e.device(&id(1)).unwrap();
    assert_eq!(v.description.led_count(), 9);
    assert_eq!(v.description.zones[2].start, 6);
    assert_eq!(v.colors, vec![RED, GREEN, GREEN, BLUE, BLUE, BLUE, RED, RED, RED]);
    assert_eq!(
        v.setting,
        Some(DeviceSetting::Direct {
            colors: v.colors.clone()
        })
    );

    assert!(matches!(
        e.resize_zone(&id(1), 0, 2),
        Err(EngineError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.resize_zone(&id(1), 1, 121),
        Err(EngineError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.resize_zone(&id(1), 7, 1),
        Err(EngineError::InvalidArgument(_))
    ));
}

#[test]
fn apply_present_and_pending_devices() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let plugged = Arc::clone(&b.plugged);
    let hp = FakeHotplug::default();
    let e = start_with(
        cfg(60),
        vec![Box::new(b)],
        Some(Box::new(hp.clone())),
        fast_timing(),
    )
    .unwrap();
    let events = e.subscribe();

    let settings = BTreeMap::from([
        (
            id(1),
            DeviceSetting::Direct {
                colors: vec![RED, GREEN],
            },
        ),
        (id(2), DeviceSetting::Mode { state: static_red() }),
    ]);
    let report = e.apply(&settings);
    assert_eq!(report.applied, [id(1)]);
    assert_eq!(report.missing, [id(2)]);
    assert!(report.failed.is_empty());
    let mut want = vec![GREEN; 7];
    want[0] = RED;
    assert_eq!(
        e.device(&id(1)).unwrap().colors,
        want,
        "padded with the last colour"
    );

    // Device 2 arrives: detected with device 1 skipped, gets its pending setting.
    plugged.lock().push(spec(id(2), "B", DeviceKind::Light));
    hp.inject(HotplugAction::Arrived, r"\\?\hid#whatever");
    assert!(wait_until(SECOND, || e.device(&id(2)).is_some()));
    assert_eq!(
        e.device(&id(2)).unwrap().setting,
        Some(DeviceSetting::Mode { state: static_red() })
    );
    assert_eq!(entries(&log, "mode"), [format!("{} mode 1", id(2))]);
    let evs: Vec<EngineEvent> = std::iter::from_fn(|| events.recv_timeout(SECOND / 5).ok()).collect();
    assert!(matches!(&evs[0], EngineEvent::DeviceAdded(info) if info.id == id(2)));
    assert!(matches!(&evs[1], EngineEvent::DeviceStateChanged { id: got, .. } if *got == id(2)));

    let current = e.current_settings();
    assert_eq!(current.len(), 2);
    assert_eq!(current[&id(1)], DeviceSetting::Direct { colors: want });

    // A failing entry is reported.
    let bad = BTreeMap::from([(
        id(1),
        DeviceSetting::Mode {
            state: ModeState {
                mode: 9,
                ..static_red()
            },
        },
    )]);
    let report = e.apply(&bad);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].id, id(1));
}

#[test]
fn rescan_skips_open_devices() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let plugged = Arc::clone(&b.plugged);
    let contexts = Arc::clone(&b.contexts);
    let e = engine(30, vec![Box::new(b)]);
    plugged.lock().push(spec(id(2), "B", DeviceKind::Light));
    e.rescan();
    assert_eq!(e.devices().len(), 2);
    let ctx = contexts.lock().last().cloned().unwrap();
    assert_eq!(ctx.skip, HashSet::from([id(1)]));
}

#[test]
fn hotplug_removal_drops_device_without_release() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            spec(id(2), "B", DeviceKind::Light),
        ],
        &log,
    );
    let hp = FakeHotplug::default();
    let e = start_with(
        cfg(30),
        vec![Box::new(b)],
        Some(Box::new(hp.clone())),
        fast_timing(),
    )
    .unwrap();
    let events = e.subscribe();
    let location = e.device(&id(1)).unwrap().description.info.location;
    hp.inject(HotplugAction::Removed, &location.to_ascii_lowercase());
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceRemoved(id(1))
    );
    assert!(e.device(&id(1)).is_none());
    assert!(e.device(&id(2)).is_some());

    hp.inject(HotplugAction::Removed, r"\\?\hid#unrelated");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(e.devices().len(), 1);

    e.shutdown(SECOND);
    assert!(*hp.stopped.lock(), "hot-plug stopped on shutdown");
    assert_eq!(entries(&log, "release"), [format!("{} release", id(2))]);
}

#[test]
fn disabling_a_family_releases_and_drops_its_devices() {
    let log = Log::default();
    let a = FakeBackend::new("fam_a", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let b = FakeBackend::new("fam_b", vec![spec(id(2), "B", DeviceKind::Light)], &log);
    let e = engine(30, vec![Box::new(a), Box::new(b)]);
    let events = e.subscribe();
    e.set_disabled_families(HashSet::from(["fam_a".to_string()]));
    assert_eq!(entries(&log, "release"), [format!("{} release", id(1))]);
    assert!(e.device(&id(1)).is_none());
    assert!(e.device(&id(2)).is_some());
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceRemoved(id(1))
    );
    e.rescan();
    assert!(e.device(&id(1)).is_none(), "disabled family stays undetected");

    e.set_disabled_families(HashSet::new());
    e.rescan();
    assert!(
        e.device(&id(1)).is_some(),
        "re-enabled family is picked up by rescan"
    );
}

#[test]
fn shutdown_releases_active_devices_in_order_and_is_idempotent() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(3), "C", DeviceKind::Light),
            spec(id(1), "A", DeviceKind::Light),
            spec(id(2), "B", DeviceKind::Light),
        ],
        &log,
    );
    let e = engine(30, vec![Box::new(b)]);
    let events = e.subscribe();
    e.release(&id(2)).unwrap();
    log.lock().clear();

    e.shutdown(SECOND);
    let releases: Vec<String> = log.lock().iter().map(|(_, s)| s.clone()).collect();
    assert_eq!(
        releases,
        [format!("{} release", id(1)), format!("{} release", id(3))],
        "active devices only, in id order"
    );
    e.shutdown(SECOND);
    assert_eq!(log.lock().len(), 2, "second shutdown does nothing");

    assert!(e.devices().is_empty());
    assert_eq!(e.set_direct(&id(1), vec![RED; 7]), Err(EngineError::ShutDown));
    assert_eq!(e.release(&id(1)), Err(EngineError::ShutDown));
    assert!(
        events.recv_timeout(SECOND / 10).is_err(),
        "no events, stream closed"
    );
    assert!(e.subscribe().recv_timeout(SECOND / 10).is_err());
}

#[test]
fn shutdown_with_failing_release_still_stops_everything() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().fail_release = true;
    let b = FakeBackend::new("f", vec![s, spec(id(2), "B", DeviceKind::Light)], &log);
    let e = engine(30, vec![Box::new(b)]);
    e.shutdown(SECOND);
    assert_eq!(entries(&log, "release").len(), 2);
}

#[test]
fn shutdown_does_not_wait_forever_on_a_stuck_device() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().mode_delay = Duration::from_millis(800);
    let b = FakeBackend::new("f", vec![s], &log);
    let timing = Timing {
        ack_timeout: Duration::from_millis(50),
        ..fast_timing()
    };
    let e = start_with(cfg(30), vec![Box::new(b)], None, timing).unwrap();
    assert_eq!(e.set_mode(&id(1), static_red()), Err(EngineError::Timeout));
    let t = Instant::now();
    e.shutdown(Duration::from_millis(200));
    assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
}

#[test]
fn dropping_last_handle_shuts_down() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(30, vec![Box::new(b)]);
    let e2 = e.clone();
    drop(e);
    assert!(entries(&log, "release").is_empty(), "a clone is still alive");
    drop(e2);
    assert_eq!(entries(&log, "release"), [format!("{} release", id(1))]);
}

#[test]
fn release_all_and_frame_rate_and_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineHandle>();

    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            spec(id(2), "B", DeviceKind::Light),
        ],
        &log,
    );
    let e = engine(30, vec![Box::new(b)]);
    e.set_frame_rate(0); // clamped to 1
    e.set_frame_rate(500); // clamped to 60
    let results = e.release_all();
    assert_eq!(results, vec![(id(1), Ok(())), (id(2), Ok(()))]);
    assert!(e.devices().iter().all(|v| v.status == DeviceStatus::Released));
    assert!(format!("{e:?}").contains("devices: 2"));
}

#[test]
fn device_without_direct_mode_rejects_colors() {
    let log = Log::default();
    let mut s = spec(id(1), "A", DeviceKind::Light);
    s.desc.direct_mode = None;
    let b = FakeBackend::new("f", vec![s], &log);
    let e = engine(30, vec![Box::new(b)]);
    assert!(matches!(
        e.set_direct(&id(1), vec![RED; 7]),
        Err(EngineError::InvalidArgument(_))
    ));
}

#[test]
fn removal_falls_back_to_enumeration_by_vid_pid() {
    let log = Log::default();
    // Two identical serial-less devices share vid/pid 1234:0001; a third has another pid.
    let a = spec(id(1).with_ordinal(0), "A0", DeviceKind::Light);
    let b = spec(id(1).with_ordinal(1), "A1", DeviceKind::Light);
    let c = spec(id(2), "B", DeviceKind::Light);
    let (la, lc) = (a.desc.info.location.clone(), c.desc.info.location.clone());
    let backend = FakeBackend::new("f", vec![a, b, c], &log);
    let hp = FakeHotplug::default();
    let e = start_with(
        cfg(30),
        vec![Box::new(backend)],
        Some(Box::new(hp.clone())),
        fast_timing(),
    )
    .unwrap();
    let events = e.subscribe();

    // Enumeration fails: nothing is dropped.
    hp.inject_with(HotplugAction::Removed, r"\\?\hid#other_collection", 0x0001);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(e.devices().len(), 3);

    // The event names a collection we did not open; A1 is no longer enumerated, A0 and B still are.
    *hp.present.lock() = Some(vec![la.to_ascii_uppercase(), lc.clone(), r"\\?\hid#x".into()]);
    hp.inject_with(HotplugAction::Removed, r"\\?\hid#other_collection", 0x0001);
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceRemoved(id(1).with_ordinal(1))
    );
    assert!(e.device(&id(1).with_ordinal(0)).is_some());
    assert!(e.device(&id(2)).is_some());

    // A vid/pid we do not control is ignored even if its devices are absent from enumeration.
    *hp.present.lock() = Some(vec![]);
    hp.inject_with(HotplugAction::Removed, r"\\?\hid#unrelated", 0x0042);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(e.devices().len(), 2);
    assert!(
        entries(&log, "release").is_empty(),
        "removed devices are not released"
    );
}

#[test]
fn apply_truncates_long_direct_buffers() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    let mut long = vec![RED; 7];
    long.extend([BLUE; 3]);
    let report = e.apply(&BTreeMap::from([(id(1), DeviceSetting::Direct { colors: long })]));
    assert_eq!(report.applied, [id(1)]);
    assert_eq!(e.device(&id(1)).unwrap().colors, vec![RED; 7]);
}
