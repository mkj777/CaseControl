//! Engine tests against fake backends, controllers and hot-plug sources. No hardware is touched.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cc_config::{DeviceSetting, HardwareState};
use cc_core::{
    Backend, Bus, ColorMode, Controller, DetectContext, Detected, DeviceDescription, DeviceError, DeviceId,
    DeviceInfo, DeviceKind, EngineEvent, HotplugAction, HotplugEvent, HotplugHandle, HotplugSender, Led,
    Mode, ModeCaps, ModeState, Rgb, Zone, ZoneKind,
};
use parking_lot::Mutex;

use crate::engine::{EngineConfig, EngineHandle, start_with};
use crate::error::EngineError;
use crate::hotplug::HotplugSource;
use crate::view::{DeviceStatus, Takeover};
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
    /// How long `resize_zone` blocks.
    resize_delay: Duration,
    /// `release` fails with a transport error.
    fail_release: bool,
    /// What `hardware_mode` reports.
    hardware: Option<ModeState>,
    /// What `own_lighting` reports.
    own_lighting: Option<Vec<u8>>,
    /// What `off_mode` reports.
    off_mode: Option<u32>,
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
        match state.brightness {
            Some(b) => self.log(format!("mode {} b={b}", state.mode)),
            None => self.log(format!("mode {}", state.mode)),
        }
        Ok(())
    }

    fn resize_zone(&mut self, zone: usize, len: usize) -> Result<(), DeviceError> {
        let delay = self.behavior.lock().resize_delay;
        std::thread::sleep(delay);
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

    fn hardware_mode(&self) -> Option<ModeState> {
        self.behavior.lock().hardware.clone()
    }

    fn own_lighting(&self) -> Option<Vec<u8>> {
        self.behavior.lock().own_lighting.clone()
    }

    fn off_mode(&self) -> Option<u32> {
        self.behavior.lock().off_mode
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
        brightness_only: false,
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
    /// How long `detect` blocks.
    delay: Arc<Mutex<Duration>>,
    /// The length of the call log when each `detect` started (to order calls against detection passes).
    detect_marks: Arc<Mutex<Vec<usize>>>,
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
            delay: Arc::default(),
            detect_marks: Arc::default(),
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
        let mark = self.log.lock().len();
        self.detect_marks.lock().push(mark);
        let delay = *self.delay.lock();
        std::thread::sleep(delay);
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
        resume_debounce: Duration::from_millis(300),
        resume_settle: Duration::from_millis(5),
        resume_retry: Duration::from_millis(300),
        resume_window: Duration::from_secs(2),
        resume_poll: Duration::from_millis(10),
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

/// Sends device `n` red colours and waits until they went out (the device is driven).
fn drive(e: &EngineHandle, log: &Log, n: u16) {
    e.set_direct(&id(n), vec![RED; 7]).unwrap();
    assert!(wait_until(SECOND, || frames(log, n).last() == Some(&hex(&[RED; 7]))));
    assert!(wait_until(SECOND, || e.device(&id(n)).unwrap().status
        == DeviceStatus::Active));
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
    assert_eq!(
        v.status,
        DeviceStatus::Released,
        "a device shows its own lighting until it is driven"
    );
    assert_eq!(v.colors, vec![Rgb::BLACK; 7]);
    assert_eq!(v.setting, None);
    assert_eq!(v.hardware, None);
    assert!(e.device(&id(9)).is_none());

    let f = e.failures();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].family, "fam_a");
    assert!(e.devices().iter().all(|v| v.family == "fam_a"));
    assert_eq!(f[0].location, "HID: broken");
    assert_eq!(f[0].error, DeviceError::Timeout);

    let json = serde_json::to_value(e.device(&id(1)).unwrap()).unwrap();
    assert_eq!(json["status"], serde_json::json!({"state": "released"}));
    assert_eq!(json["hardware"], serde_json::Value::Null);
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
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Active);
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceStateChanged {
            id: id(1),
            active: true,
            state: None
        },
        "taken"
    );
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceStateChanged {
            id: id(1),
            active: true,
            state: Some(static_red())
        },
        "mode applied"
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
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceStateChanged {
            id: id(1),
            active: false,
            state: None
        },
        "released"
    );
    assert_eq!(e.release(&id(1)), Ok(()));
    assert!(
        events.try_recv().is_err(),
        "only changes of status or mode emit events"
    );
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

    // Shutdown releases a faulted device too, best effort (ADR 0001 #38): one attempt, nothing else is sent.
    e.shutdown(SECOND);
    assert_eq!(entries(&log, "release").len(), 1);
    assert_eq!(entries(&log, "direct").len(), 1);
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
    let want = vec![RED, GREEN, GREEN, BLUE, BLUE, BLUE, RED, RED, RED];
    assert_eq!(v.colors, want);
    // The carried-over buffer goes out as a new frame, after which it is the recorded setting.
    let hex = want.iter().map(Rgb::to_string).collect::<Vec<_>>().join(",");
    assert!(wait_until(SECOND, || entries(&log, "direct")
        .last()
        .is_some_and(|s| s.ends_with(&hex))));
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting
        == Some(DeviceSetting::Direct { colors: want.clone() })));

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
    let evs: Vec<EngineEvent> = std::iter::from_fn(|| events.recv_timeout(SECOND / 5).ok())
        .filter(|ev| !matches!(ev, EngineEvent::DeviceStateChanged { id: got, .. } if *got == id(1)))
        .collect();
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
    drive(&e, &log, 2);
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
    drive(&e, &log, 1);
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
    for n in 1..=3 {
        drive(&e, &log, n);
    }
    e.release(&id(2)).unwrap();
    let events = e.subscribe();
    log.lock().clear();

    e.shutdown(SECOND);
    let releases: Vec<String> = log.lock().iter().map(|(_, s)| s.clone()).collect();
    assert_eq!(
        releases,
        [format!("{} release", id(1)), format!("{} release", id(3))],
        "driven devices only, in id order"
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
    drive(&e, &log, 1);
    drive(&e, &log, 2);
    e.shutdown(SECOND);
    assert_eq!(entries(&log, "release").len(), 2);
}

#[test]
fn shutdown_releases_never_driven_devices_but_not_handed_back_ones() {
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
    e.release(&id(2)).unwrap();
    log.lock().clear();
    e.shutdown(SECOND);
    assert_eq!(
        entries(&log, "release"),
        [format!("{} release", id(1))],
        "the driver decides what an untouched release does; a handed-back device is left alone"
    );
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
    drive(&e, &log, 1);
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
    // Neither device was driven: both get a release, but none counts as released.
    let report = e.release_all();
    assert_eq!(report.released, 0);
    assert!(report.failed.is_empty());
    assert_eq!(entries(&log, "release").len(), 2);
    assert!(e.devices().iter().all(|v| v.status == DeviceStatus::Released));
    assert!(format!("{e:?}").contains("devices: 2"));

    // Both driven: both count.
    drive(&e, &log, 1);
    drive(&e, &log, 2);
    assert_eq!(e.release_all().released, 2);
    assert!(e.devices().iter().all(|v| v.status == DeviceStatus::Released));
}

#[test]
fn release_all_counts_only_driven_devices_and_reports_failures() {
    let log = Log::default();
    let failing = spec(id(3), "C", DeviceKind::Light);
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            spec(id(2), "B", DeviceKind::Light),
            failing.clone(),
        ],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    drive(&e, &log, 1);
    drive(&e, &log, 3);
    failing.behavior.lock().fail_release = true;
    let report = e.release_all();
    assert_eq!(report.released, 1, "device 1 only: 2 was not driven, 3 failed");
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].id, id(3));
    assert!(
        report.failed[0].error.contains("release failed"),
        "{}",
        report.failed[0].error
    );
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

// ---------------------------------------------------------------------------------------------------------------
// Regression tests for the review findings

/// A colour edit issued while a mode change is in flight must win, and the record must say so.
#[test]
fn set_direct_during_set_mode_ends_direct_and_record_matches() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().mode_delay = Duration::from_millis(200);
    let b = FakeBackend::new("f", vec![s], &log);
    let e = engine(60, vec![Box::new(b)]);
    let events = e.subscribe();

    let e2 = e.clone();
    let mode = std::thread::spawn(move || e2.set_mode(&id(1), static_red()));
    std::thread::sleep(Duration::from_millis(50)); // the worker is inside set_mode now
    e.set_direct(&id(1), vec![BLUE; 7]).unwrap();
    assert_eq!(mode.join().unwrap(), Ok(()));

    let blue = DeviceSetting::Direct {
        colors: vec![BLUE; 7],
    };
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting == Some(blue.clone())));
    std::thread::sleep(Duration::from_millis(100));
    let all: Vec<String> = log.lock().iter().map(|(_, s)| s.clone()).collect();
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all[0].ends_with("mode 1"));
    assert!(
        all[1].contains("direct"),
        "device ends showing the later colour edit"
    );
    assert_eq!(e.device(&id(1)).unwrap().setting, Some(blue.clone()));
    assert_eq!(e.current_settings()[&id(1)], blue);
    // The mode really was applied first, so its events (taken, mode) are correct; nothing else follows.
    assert!(matches!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceStateChanged { state: None, .. }
    ));
    assert!(matches!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceStateChanged { state: Some(_), .. }
    ));
    assert!(events.try_recv().is_err());
}

/// Rescan (and therefore hot-plug arrival, which uses the same pass) retries a faulted device.
#[test]
fn rescan_replaces_a_faulted_device() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().fatal_direct = true;
    let behavior = Arc::clone(&s.behavior);
    let b = FakeBackend::new("f", vec![s], &log);
    let contexts = Arc::clone(&b.contexts);
    let e = engine(60, vec![Box::new(b)]);
    let events = e.subscribe();
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    assert!(matches!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceFaulted { .. }
    ));

    behavior.lock().fatal_direct = false;
    e.rescan();
    assert!(
        contexts.lock().last().unwrap().skip.is_empty(),
        "faulted ids are not skipped"
    );
    assert!(
        matches!(events.recv_timeout(SECOND).unwrap(), EngineEvent::DeviceAdded(info) if info.id == id(1))
    );
    assert_eq!(e.devices().len(), 1);
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Released);
    e.set_direct(&id(1), vec![GREEN; 7]).unwrap();
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting
        == Some(DeviceSetting::Direct {
            colors: vec![GREEN; 7]
        })));

    // A healthy device is still skipped.
    e.rescan();
    assert_eq!(contexts.lock().last().unwrap().skip, HashSet::from([id(1)]));
}

/// A detection pass stuck in a backend must not hold shutdown past its budget, and adds nothing afterwards.
#[test]
fn shutdown_honours_budget_while_detection_is_stuck() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let plugged = Arc::clone(&b.plugged);
    let delay = Arc::clone(&b.delay);
    let hp = FakeHotplug::default();
    let e = start_with(
        cfg(30),
        vec![Box::new(b)],
        Some(Box::new(hp.clone())),
        fast_timing(),
    )
    .unwrap();
    drive(&e, &log, 1);
    let events = e.subscribe();

    *delay.lock() = Duration::from_millis(700);
    plugged.lock().push(spec(id(2), "B", DeviceKind::Light));
    hp.inject(HotplugAction::Arrived, r"\\?\hid#new");
    std::thread::sleep(Duration::from_millis(60)); // hot-plug thread is inside detect, holding the detect lock

    let t = Instant::now();
    e.shutdown(Duration::from_millis(200));
    assert!(t.elapsed() < Duration::from_millis(450), "{:?}", t.elapsed());
    assert_eq!(entries(&log, "release"), [format!("{} release", id(1))]);

    std::thread::sleep(Duration::from_millis(900)); // the stuck detection finishes
    assert!(e.devices().is_empty(), "nothing added after shutdown");
    assert!(
        entries(&log, &id(2).to_string()).is_empty(),
        "late device never driven"
    );
    assert!(events.recv_timeout(SECOND / 10).is_err());
}

/// Shutdown asks the worker to release based on the worker's own state, not the engine's possibly stale copy.
#[test]
fn shutdown_releases_a_device_that_took_control_again() {
    for _ in 0..20 {
        let log = Log::default();
        let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
        let e = engine(60, vec![Box::new(b)]);
        e.release(&id(1)).unwrap();
        e.set_direct(&id(1), vec![RED; 7]).unwrap();
        assert!(wait_until(SECOND, || !entries(&log, "direct").is_empty()));
        e.shutdown(SECOND);
        let all: Vec<String> = log.lock().iter().map(|(_, s)| s.clone()).collect();
        assert_eq!(all.len(), 3, "{all:?}");
        assert!(all[2].ends_with("release"), "{all:?}");
    }
}

/// A resize that outlives its ack timeout still updates the engine's description once it happens.
#[test]
fn late_resize_keeps_description_consistent() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().resize_delay = Duration::from_millis(200);
    let b = FakeBackend::new("f", vec![s], &log);
    let timing = Timing {
        ack_timeout: Duration::from_millis(50),
        ..fast_timing()
    };
    let e = start_with(cfg(60), vec![Box::new(b)], None, timing).unwrap();
    assert_eq!(e.resize_zone(&id(1), 1, 5), Err(EngineError::Timeout));
    assert!(wait_until(SECOND, || e
        .device(&id(1))
        .unwrap()
        .description
        .led_count()
        == 9));
    assert_eq!(e.device(&id(1)).unwrap().colors.len(), 9);
    e.set_direct(&id(1), vec![GREEN; 9]).unwrap();
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting
        == Some(DeviceSetting::Direct {
            colors: vec![GREEN; 9]
        })));
}

/// A late result from a removed device's old thread must not land on the re-plugged device.
#[test]
fn late_result_does_not_touch_a_replugged_device() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    s.behavior.lock().mode_delay = Duration::from_millis(250);
    let behavior = Arc::clone(&s.behavior);
    let b = FakeBackend::new("f", vec![s], &log);
    let hp = FakeHotplug::default();
    let timing = Timing {
        ack_timeout: Duration::from_millis(50),
        ..fast_timing()
    };
    let e = start_with(cfg(60), vec![Box::new(b)], Some(Box::new(hp.clone())), timing).unwrap();
    let location = e.device(&id(1)).unwrap().description.info.location;
    assert_eq!(e.set_mode(&id(1), static_red()), Err(EngineError::Timeout));

    let events = e.subscribe();
    hp.inject(HotplugAction::Removed, &location);
    assert!(wait_until(SECOND, || e.device(&id(1)).is_none()));
    hp.inject(HotplugAction::Arrived, &location);
    assert!(wait_until(SECOND, || e.device(&id(1)).is_some()));
    behavior.lock().mode_delay = Duration::ZERO;

    std::thread::sleep(Duration::from_millis(350)); // the old thread finishes its set_mode
    assert_eq!(entries(&log, "mode").len(), 1, "old controller did apply it");
    assert_eq!(e.device(&id(1)).unwrap().setting, None);
    let evs: Vec<EngineEvent> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    assert!(
        !evs.iter()
            .any(|ev| matches!(ev, EngineEvent::DeviceStateChanged { .. })),
        "{evs:?}"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Brightness (ADR 0001 #33, #35)

const DARK_RED: Rgb = Rgb::new(0x80, 0, 0); // 255 * 0.5 = 127.5 -> 128
const DARKER_RED: Rgb = Rgb::new(0x40, 0, 0); // 255 * 0.25 = 63.75 -> 64

fn hex(colors: &[Rgb]) -> String {
    colors.iter().map(Rgb::to_string).collect::<Vec<_>>().join(",")
}

/// The direct frames device `n` was sent, as colour lists.
fn frames(log: &Log, n: u16) -> Vec<String> {
    let prefix = format!("{} direct ", id(n));
    log.lock()
        .iter()
        .filter_map(|(_, s)| s.strip_prefix(&prefix).map(str::to_owned))
        .collect()
}

/// The mode calls device `n` got ("2 b=41", "3").
fn modes(log: &Log, n: u16) -> Vec<String> {
    let prefix = format!("{} mode ", id(n));
    log.lock()
        .iter()
        .filter_map(|(_, s)| s.strip_prefix(&prefix).map(str::to_owned))
        .collect()
}

/// Adds mode 2 "Breathing" (brightness 1..=100, one colour), mode 3 "Wave" (no brightness) and, with `off`, mode
/// 4 "Off".
fn with_effects(mut s: Spec, off: bool) -> Spec {
    s.desc.modes.push(Mode {
        id: 2,
        name: "Breathing".into(),
        caps: ModeCaps::BRIGHTNESS | ModeCaps::MODE_SPECIFIC_COLOR,
        color_mode: ColorMode::ModeSpecific,
        speed: None,
        brightness: Some(1..=100),
        colors: Some(1..=1),
    });
    s.desc.modes.push(Mode {
        id: 3,
        name: "Wave".into(),
        caps: ModeCaps::SPEED,
        color_mode: ColorMode::None,
        speed: None,
        brightness: None,
        colors: None,
    });
    if off {
        s.desc.modes.push(Mode {
            id: 4,
            name: "Off".into(),
            caps: ModeCaps::empty(),
            color_mode: ColorMode::None,
            speed: None,
            brightness: None,
            colors: None,
        });
    }
    s
}

fn breathing(brightness: u32) -> ModeState {
    ModeState {
        mode: 2,
        speed: None,
        brightness: Some(brightness),
        direction: None,
        colors: vec![RED],
    }
}

fn wave() -> ModeState {
    ModeState {
        mode: 3,
        speed: None,
        brightness: None,
        direction: None,
        colors: vec![],
    }
}

/// Waits until the last direct frame of device `n` is `want`.
fn shows(log: &Log, n: u16, want: &[Rgb]) -> bool {
    let want = hex(want);
    wait_until(SECOND, || frames(log, n).last() == Some(&want))
}

/// Waits until the last mode call of device `n` is `want`.
fn shows_mode(log: &Log, n: u16, want: &str) -> bool {
    wait_until(SECOND, || modes(log, n).last().map(String::as_str) == Some(want))
}

#[test]
fn brightness_scales_the_output_but_never_the_stored_colours() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    assert!(shows(&log, 1, &[RED; 7]));

    e.set_device_brightness(&id(1), 50).unwrap();
    assert!(shows(&log, 1, &[DARK_RED; 7]));
    e.set_master_brightness(50);
    assert!(shows(&log, 1, &[DARKER_RED; 7]));

    let v = e.device(&id(1)).unwrap();
    assert_eq!(v.brightness, 50);
    assert_eq!(v.colors, vec![RED; 7], "the buffer stays unscaled");
    let red = DeviceSetting::Direct { colors: vec![RED; 7] };
    assert_eq!(v.setting, Some(red.clone()), "the record stays unscaled");
    assert_eq!(e.current_settings()[&id(1)], red);
    assert_eq!(e.brightness(), BTreeMap::from([(id(1), 50)]));
    assert_eq!(e.lighting().master_brightness, 50);

    // A colour edit while dimmed is stored as given and shown dimmed.
    e.set_zone(&id(1), 2, vec![GREEN; 3]).unwrap();
    let dim_green = Rgb::new(0, 0x40, 0);
    let dimmed = [
        DARKER_RED, DARKER_RED, DARKER_RED, DARKER_RED, dim_green, dim_green, dim_green,
    ];
    assert!(shows(&log, 1, &dimmed));
    let stored = vec![RED, RED, RED, RED, GREEN, GREEN, GREEN];
    assert_eq!(e.device(&id(1)).unwrap().colors, stored);

    // Back up to full: exactly the stored colours again.
    e.set_master_brightness(100);
    e.set_device_brightness(&id(1), 150).unwrap(); // clamped to 100
    assert!(shows(&log, 1, &stored));
    assert!(e.brightness().is_empty());
    assert_eq!(e.device(&id(1)).unwrap().brightness, 100);
    assert!(matches!(
        e.set_device_brightness(&id(9), 10),
        Err(EngineError::UnknownDevice(_))
    ));
}

#[test]
fn brightness_takes_over_a_released_device_with_its_setting() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    assert!(shows(&log, 1, &[RED; 7]));
    e.release(&id(1)).unwrap();
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Released);
    let events = e.subscribe();

    // Device brightness takes it back with its last colours, dimmed.
    e.set_device_brightness(&id(1), 50).unwrap();
    assert!(shows(&log, 1, &[DARK_RED; 7]));
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Active));
    assert_eq!(
        events.recv_timeout(SECOND).unwrap(),
        EngineEvent::DeviceStateChanged {
            id: id(1),
            active: true,
            state: None
        }
    );

    // Released again, the master brightness takes it back too.
    e.release(&id(1)).unwrap();
    e.set_master_brightness(50);
    assert!(shows(&log, 1, &[DARKER_RED; 7]));
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Active));

    // And brightness 0: it goes black and stays driven.
    e.release(&id(1)).unwrap();
    e.set_device_brightness(&id(1), 0).unwrap();
    assert!(shows(&log, 1, &[Rgb::BLACK; 7]));
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Active));
    assert_eq!(
        e.device(&id(1)).unwrap().setting,
        Some(DeviceSetting::Direct { colors: vec![RED; 7] }),
        "the setting is kept, only the output is dark"
    );

    // A change at 0 is stored but shows black; turning the brightness up shows it.
    e.set_direct(&id(1), vec![BLUE; 7]).unwrap();
    let blue = DeviceSetting::Direct {
        colors: vec![BLUE; 7],
    };
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting == Some(blue.clone())));
    assert_eq!(frames(&log, 1).last().unwrap(), &hex(&[Rgb::BLACK; 7]));
    assert_eq!(e.current_settings()[&id(1)], blue);
    e.set_device_brightness(&id(1), 100).unwrap();
    e.set_master_brightness(100);
    assert!(shows(&log, 1, &[BLUE; 7]));
}

#[test]
fn brightness_adopts_the_hardware_readback_of_a_device_without_a_setting() {
    let log = Log::default();
    let with_hw = with_effects(spec(id(1), "A", DeviceKind::Light), true);
    with_hw.behavior.lock().hardware = Some(breathing(80));
    let b = FakeBackend::new("f", vec![with_hw, spec(id(2), "B", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    let v = e.device(&id(1)).unwrap();
    assert_eq!(v.status, DeviceStatus::Released);
    assert_eq!(v.hardware, Some(breathing(80)));
    assert_eq!(v.setting, None);

    // Device brightness 50 %: the readback becomes the setting and goes out at half its brightness,
    // 1 + (80 - 1) * 0.5 = 40.5 -> 41.
    e.set_device_brightness(&id(1), 50).unwrap();
    assert!(shows_mode(&log, 1, "2 b=41"));
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Active));
    assert_eq!(
        e.device(&id(1)).unwrap().setting,
        Some(DeviceSetting::Mode { state: breathing(80) })
    );

    // Without a setting or a readback the device is taken with the default colour (white), dimmed:
    // 255 * 0.3 = 76.5 -> 77, then with master 50 %: 255 * 0.15 = 38.25 -> 38.
    e.set_device_brightness(&id(2), 30).unwrap();
    assert!(shows(&log, 2, &[Rgb::new(77, 77, 77); 7]));
    e.set_master_brightness(50);
    // 1 + (80 - 1) * 0.25 = 20.75 -> 21
    assert!(shows_mode(&log, 1, "2 b=21"));
    assert!(shows(&log, 2, &[Rgb::new(38, 38, 38); 7]));
    assert!(wait_until(SECOND, || e.device(&id(2)).unwrap().status
        == DeviceStatus::Active));
    assert_eq!(
        e.device(&id(2)).unwrap().setting,
        Some(DeviceSetting::Direct {
            colors: vec![Rgb::WHITE; 7]
        }),
        "stored unscaled"
    );
    assert_eq!(e.device(&id(2)).unwrap().brightness, 30);

    // A per-LED readback also seeds the colour buffer.
    let log = Log::default();
    let s = spec(id(3), "C", DeviceKind::Light);
    let colors = vec![RED, GREEN, BLUE, RED, GREEN, BLUE, RED];
    s.behavior.lock().hardware = Some(ModeState {
        mode: 0,
        speed: None,
        brightness: None,
        direction: None,
        colors: colors.clone(),
    });
    let e = engine(60, vec![Box::new(FakeBackend::new("f", vec![s], &log))]);
    assert_eq!(e.device(&id(3)).unwrap().colors, colors);
}

#[test]
fn master_brightness_zero_takes_over_visible_devices_and_shows_off() {
    let log = Log::default();
    let wave_hw = with_effects(spec(id(2), "B", DeviceKind::Light), true);
    wave_hw.behavior.lock().hardware = Some(wave());
    let hidden_hw = with_effects(spec(id(3), "C", DeviceKind::Light), true);
    hidden_hw.behavior.lock().hardware = Some(wave());
    let b = FakeBackend::new(
        "f",
        vec![spec(id(1), "A", DeviceKind::Light), wave_hw, hidden_hw],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    e.set_excluded(HashSet::from([id(3)]));
    drive(&e, &log, 1);
    e.release(&id(1)).unwrap();
    let events = e.subscribe();

    e.set_master_brightness(0);
    // Its last setting at 0: black. The readback of a mode without a brightness parameter at 0: its Off mode.
    assert!(shows(&log, 1, &[Rgb::BLACK; 7]));
    assert!(shows_mode(&log, 2, "4"));
    for n in [1, 2] {
        assert!(wait_until(SECOND, || e.device(&id(n)).unwrap().status
            == DeviceStatus::Active));
    }
    assert_eq!(
        e.device(&id(2)).unwrap().setting,
        Some(DeviceSetting::Mode { state: wave() }),
        "the readback is the setting, Off is only the output"
    );
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        calls_since(&log, 3, 0).is_empty(),
        "excluded devices are left alone"
    );
    assert_eq!(e.device(&id(3)).unwrap().status, DeviceStatus::Released);
    assert!(wait_until(SECOND, || events.try_iter().any(|ev| ev
        == EngineEvent::DeviceStateChanged {
            id: id(2),
            active: true,
            state: None
        })));

    // Back up: the colours and the mode it showed.
    e.set_master_brightness(100);
    assert!(shows(&log, 1, &[RED; 7]));
    assert!(shows_mode(&log, 2, "3"));
}

#[test]
fn connect_all_takes_over_released_devices_with_setting_readback_or_default_colour() {
    let log = Log::default();
    let wave_hw = with_effects(spec(id(2), "B", DeviceKind::Light), true);
    wave_hw.behavior.lock().hardware = Some(wave());
    let hidden_hw = with_effects(spec(id(4), "D", DeviceKind::Light), true);
    hidden_hw.behavior.lock().hardware = Some(wave());
    let broken = spec(id(5), "E", DeviceKind::Light);
    broken.behavior.lock().fatal_direct = true;
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            wave_hw,
            spec(id(3), "C", DeviceKind::Light),
            hidden_hw,
            broken,
        ],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    e.set_excluded(HashSet::from([id(4)]));
    // Device 1 has a setting and was released; device 5 faulted.
    drive(&e, &log, 1);
    e.release(&id(1)).unwrap();
    let events = e.subscribe();
    e.set_direct(&id(5), vec![RED; 7]).unwrap();
    assert!(wait_until(SECOND, || matches!(
        e.device(&id(5)).unwrap().status,
        DeviceStatus::Faulted { .. }
    )));
    let calls_5 = calls_since(&log, 5, 0).len();

    e.connect_all();
    // Its last setting, and the mode it showed on its own.
    assert!(shows(&log, 1, &[RED; 7]));
    assert!(shows_mode(&log, 2, "3"));
    for n in [1, 2] {
        assert!(wait_until(SECOND, || e.device(&id(n)).unwrap().status
            == DeviceStatus::Active));
    }
    assert_eq!(
        e.device(&id(2)).unwrap().setting,
        Some(DeviceSetting::Mode { state: wave() })
    );
    assert!(wait_until(SECOND, || events.try_iter().any(|ev| matches!(
        ev,
        EngineEvent::DeviceStateChanged { id: got, active: true, .. } if got == id(2)
    ))));
    // Neither a setting nor a readback: the default colour on every LED.
    assert!(shows(&log, 3, &[Rgb::WHITE; 7]));
    assert!(wait_until(SECOND, || e.device(&id(3)).unwrap().status
        == DeviceStatus::Active));
    // Hidden, faulted: left alone.
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        calls_since(&log, 4, 0).is_empty(),
        "excluded devices are left alone"
    );
    assert_eq!(
        calls_since(&log, 5, 0).len(),
        calls_5,
        "faulted devices are left alone"
    );

    // Everything connectable is driven: a second Connect all sends nothing.
    let n = log.lock().len();
    e.connect_all();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(log.lock().len(), n);
}

/// Device `n` without a direct mode: its modes are Breathing (one colour, brightness 1..=100) after Wave (none), so
/// Breathing is the first that takes a colour.
fn without_direct(n: u16) -> Spec {
    let mut s = with_effects(spec(id(n), "X", DeviceKind::Light), false);
    s.desc.direct_mode = None;
    s.desc.modes.retain(|m| m.id >= 2);
    s.desc.modes.sort_by_key(|m| std::cmp::Reverse(m.id));
    s
}

#[test]
fn takeover_prefers_setting_then_readback_then_the_configured_default_colour() {
    let log = Log::default();
    // 1: setting and readback; 2: readback only; 3: neither; 4: neither, hidden zone "ARGB 1"; 5: neither and no
    // direct mode; 6: no direct mode and no mode that takes a colour.
    let with_both = with_effects(spec(id(1), "A", DeviceKind::Light), true);
    with_both.behavior.lock().hardware = Some(wave());
    let readback = with_effects(spec(id(2), "B", DeviceKind::Light), true);
    readback.behavior.lock().hardware = Some(wave());
    let mut colourless = without_direct(6);
    colourless.desc.modes.retain(|m| m.color_mode == ColorMode::None);
    let b = FakeBackend::new(
        "f",
        vec![
            with_both,
            readback,
            spec(id(3), "C", DeviceKind::Light),
            spec(id(4), "D", DeviceKind::Light),
            without_direct(5),
            colourless,
        ],
        &log,
    );
    let e = start_with(
        EngineConfig {
            default_color: GREEN,
            ..cfg(60)
        },
        vec![Box::new(b)],
        None,
        fast_timing(),
    )
    .unwrap();
    drive(&e, &log, 1);
    e.release(&id(1)).unwrap();
    e.set_default_color(BLUE);
    e.set_dark_zones(BTreeMap::from([(id(4), BTreeSet::from(["ARGB 1".to_owned()]))]));

    e.connect_all();
    assert!(shows(&log, 1, &[RED; 7]), "the setting wins over the readback");
    assert!(
        shows_mode(&log, 2, "3"),
        "the readback wins over the default colour"
    );
    assert!(shows(&log, 3, &[BLUE; 7]), "the latest default colour");
    let dark = Rgb::BLACK;
    assert!(
        shows(&log, 4, &[BLUE, dark, dark, dark, BLUE, BLUE, BLUE]),
        "dark zones stay black"
    );
    assert!(shows_mode(&log, 5, "2 b=100"), "first mode that takes a colour");
    assert!(wait_until(SECOND, || e.device(&id(5)).unwrap().setting
        == Some(DeviceSetting::Mode {
            state: ModeState {
                mode: 2,
                speed: None,
                brightness: Some(100),
                direction: None,
                colors: vec![BLUE],
            }
        })));
    for n in 1..=5 {
        assert!(wait_until(SECOND, || e.device(&id(n)).unwrap().status
            == DeviceStatus::Active));
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(calls_since(&log, 6, 0).is_empty(), "nothing it could show");
    assert_eq!(e.device(&id(6)).unwrap().status, DeviceStatus::Released);

    // A new default colour does not repaint driven devices.
    let n = log.lock().len();
    e.set_default_color(RED);
    e.connect_all();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(log.lock().len(), n);

    // Taken over again later, a device with a setting from the default colour keeps it.
    e.release(&id(3)).unwrap();
    let before = frames(&log, 3).len();
    e.set_device_brightness(&id(3), 100).unwrap();
    assert!(
        wait_until(SECOND, || frames(&log, 3).len() > before),
        "taken over again"
    );
    assert!(
        shows(&log, 3, &[BLUE; 7]),
        "its last setting, not the new default"
    );
}

#[test]
fn brightness_takes_a_device_with_nothing_known_over_with_the_default_colour() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![spec(id(1), "A", DeviceKind::Light), without_direct(2)],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    e.set_default_color(GREEN);
    e.set_device_brightness(&id(1), 50).unwrap();
    // 255 * 0.5 = 127.5 -> 128
    assert!(shows(&log, 1, &[Rgb::new(0, 128, 0); 7]));
    e.set_device_brightness(&id(2), 50).unwrap();
    // 1 + (100 - 1) * 0.5 = 50.5 -> 51
    assert!(shows_mode(&log, 2, "2 b=51"));
    for n in [1, 2] {
        assert!(wait_until(SECOND, || e.device(&id(n)).unwrap().status
            == DeviceStatus::Active));
    }
}

#[test]
fn hardware_mode_brightness_maps_into_the_mode_range_and_zero_uses_the_off_mode() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![with_effects(spec(id(1), "A", DeviceKind::Light), true)],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    let events = e.subscribe();
    e.set_mode(&id(1), breathing(80)).unwrap();
    assert_eq!(modes(&log, 1), ["2 b=80"]);

    e.set_device_brightness(&id(1), 50).unwrap();
    // 1 + (80 - 1) * 0.5 = 40.5 -> 41
    assert!(shows_mode(&log, 1, "2 b=41"));
    let stored = DeviceSetting::Mode { state: breathing(80) };
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting
        == Some(stored.clone())));
    assert_eq!(e.current_settings()[&id(1)], stored);

    // Brightness 0: the "Off" mode; the setting stays the mode.
    e.set_device_brightness(&id(1), 0).unwrap();
    assert!(shows_mode(&log, 1, "4"));
    assert_eq!(e.device(&id(1)).unwrap().setting, Some(stored.clone()));
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Active);

    // Back up: the mode at the dimmed level.
    e.set_device_brightness(&id(1), 50).unwrap();
    assert!(shows_mode(&log, 1, "2 b=41"));
    assert_eq!(e.device(&id(1)).unwrap().setting, Some(stored));

    // Only taking the device and the explicit mode change emitted events, not the brightness steps.
    let evs: Vec<EngineEvent> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    assert_eq!(evs.len(), 2, "{evs:?}");
}

#[test]
fn hardware_mode_without_off_mode_goes_black_and_without_brightness_is_not_resent() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            with_effects(spec(id(1), "A", DeviceKind::Light), false),
            with_effects(spec(id(2), "B", DeviceKind::Light), true),
        ],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    e.set_mode(&id(1), breathing(100)).unwrap();
    e.set_mode(&id(2), wave()).unwrap();

    e.set_master_brightness(0);
    assert!(
        shows(&log, 1, &[Rgb::BLACK; 7]),
        "no Off mode: black in direct mode"
    );
    assert!(shows_mode(&log, 2, "4"));
    assert_eq!(modes(&log, 2), ["3", "4"]);
    assert_eq!(
        e.device(&id(1)).unwrap().setting,
        Some(DeviceSetting::Mode {
            state: breathing(100)
        })
    );

    e.set_master_brightness(100);
    assert!(shows_mode(&log, 1, "2 b=100"));
    assert!(shows_mode(&log, 2, "3"));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(modes(&log, 2), ["3", "4", "3"], "restored once");
    // Device and master brightness do not touch a mode without a brightness parameter.
    e.set_device_brightness(&id(2), 20).unwrap();
    e.set_master_brightness(20);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(modes(&log, 2), ["3", "4", "3"]);
}

#[test]
fn master_brightness_zero_at_start_shows_off_without_a_coloured_frame() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            with_effects(spec(id(2), "B", DeviceKind::Light), true),
        ],
        &log,
    );
    let c = EngineConfig {
        master_brightness: 0,
        brightness: BTreeMap::from([(id(1), 50)]),
        ..cfg(60)
    };
    let e = start_with(c, vec![Box::new(b)], None, fast_timing()).unwrap();
    assert_eq!(e.lighting().master_brightness, 0);
    assert_eq!(e.device(&id(1)).unwrap().brightness, 50);
    let profile = BTreeMap::from([
        (id(1), DeviceSetting::Direct { colors: vec![RED; 7] }),
        (id(2), DeviceSetting::Mode { state: breathing(60) }),
    ]);
    let report = e.apply(&profile);
    assert_eq!(report.applied.len(), 2);
    assert!(shows(&log, 1, &[Rgb::BLACK; 7]));
    assert!(wait_until(SECOND, || modes(&log, 2).last().map(String::as_str) == Some("4")));
    std::thread::sleep(Duration::from_millis(100));
    let all = frames(&log, 1);
    assert!(all.iter().all(|f| *f == hex(&[Rgb::BLACK; 7])), "{all:?}");
    let shown = modes(&log, 2);
    assert!(
        shown.iter().all(|m| m == "4"),
        "no coloured mode at brightness 0: {shown:?}"
    );
    assert!(
        wait_until(SECOND, || e.current_settings() == profile),
        "the profile is applied underneath"
    );

    e.set_master_brightness(100);
    assert!(shows(&log, 1, &[DARK_RED; 7]));
    assert!(shows_mode(&log, 2, "2 b=60"));
}

#[test]
fn replace_brightness_resets_unlisted_devices_to_full() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            spec(id(2), "B", DeviceKind::Light),
        ],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    e.set_direct(&id(2), vec![RED; 7]).unwrap();
    e.set_device_brightness(&id(1), 10).unwrap();
    e.replace_brightness(BTreeMap::from([(id(2), 50), (id(9), 20), (id(3), 100)]));
    assert!(shows(&log, 1, &[RED; 7]));
    assert!(shows(&log, 2, &[DARK_RED; 7]));
    assert_eq!(e.brightness(), BTreeMap::from([(id(2), 50), (id(9), 20)]));
}

// ---------------------------------------------------------------------------------------------------------------
// Resume from sleep

/// Every log line of device `n` from index `from` on, without the id prefix.
fn calls_since(log: &Log, n: u16, from: usize) -> Vec<String> {
    let prefix = format!("{} ", id(n));
    log.lock()[from..]
        .iter()
        .filter_map(|(_, s)| s.strip_prefix(&prefix).map(str::to_owned))
        .collect()
}

#[test]
fn reassert_resets_and_resends_driven_devices_only_and_is_debounced() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        (1..=4).map(|n| spec(id(n), "D", DeviceKind::Light)).collect(),
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    e.set_device_brightness(&id(1), 50).unwrap();
    e.set_mode(&id(2), static_red()).unwrap();
    e.set_direct(&id(3), vec![BLUE; 7]).unwrap();
    assert!(shows(&log, 1, &[DARK_RED; 7]));
    assert!(shows(&log, 3, &[BLUE; 7]));
    e.release(&id(3)).unwrap();
    // id(4) is never driven.
    let mark = log.lock().len();

    assert!(e.reassert());
    assert!(!e.reassert(), "a second resume broadcast is debounced");
    let dark = format!("direct {}", hex(&[DARK_RED; 7]));
    assert!(wait_until(SECOND, || calls_since(&log, 1, mark)
        == ["release".to_owned(), dark.clone()]));
    assert!(wait_until(SECOND, || calls_since(&log, 2, mark) == ["release", "mode 1"]));
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        calls_since(&log, 3, mark).is_empty(),
        "released devices are left alone"
    );
    assert!(
        calls_since(&log, 4, mark).is_empty(),
        "undriven devices are left alone"
    );
    assert_eq!(
        calls_since(&log, 1, mark).len(),
        2,
        "exactly one reset and one frame"
    );
    let v = e.device(&id(1)).unwrap();
    assert_eq!(v.status, DeviceStatus::Active);
    assert_eq!(
        v.setting,
        Some(DeviceSetting::Direct { colors: vec![RED; 7] }),
        "stored unscaled"
    );
    assert_eq!(e.device(&id(3)).unwrap().status, DeviceStatus::Released);

    // After the debounce time a new resume works again.
    std::thread::sleep(fast_timing().resume_debounce);
    let mark = log.lock().len();
    assert!(e.reassert());
    assert!(wait_until(SECOND, || calls_since(&log, 2, mark) == ["release", "mode 1"]));

    e.shutdown(SECOND);
    assert!(!e.reassert(), "nothing after shutdown");
}

#[test]
fn reassert_applies_brightness_zero() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    e.set_master_brightness(0);
    assert!(shows(&log, 1, &[Rgb::BLACK; 7]));
    std::thread::sleep(Duration::from_millis(50));
    let mark = log.lock().len();
    assert!(e.reassert());
    let black = format!("direct {}", hex(&[Rgb::BLACK; 7]));
    assert!(wait_until(SECOND, || calls_since(&log, 1, mark)
        == ["release".to_owned(), black.clone()]));
}

#[test]
fn reassert_retries_a_failing_device_with_back_off() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    let behavior = Arc::clone(&s.behavior);
    let b = FakeBackend::new("f", vec![s], &log);
    let e = engine(60, vec![Box::new(b)]);
    e.set_direct(&id(1), vec![RED; 7]).unwrap();
    assert!(shows(&log, 1, &[RED; 7]));
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Active));
    let events = e.subscribe();

    // USB is not back yet: the reset fails for a moment, then works.
    behavior.lock().fail_release = true;
    assert!(e.reassert());
    assert!(wait_until(SECOND, || entries(&log, "release").len() >= 3));
    behavior.lock().fail_release = false;
    assert!(wait_until(SECOND, || frames(&log, 1).len() == 2));
    let after: Vec<String> = log.lock().iter().map(|(_, s)| s.clone()).collect();
    assert!(after.last().unwrap().contains("direct"), "{after:?}");
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Active);
    std::thread::sleep(Duration::from_millis(100));
    let evs: Vec<EngineEvent> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    assert!(evs.is_empty(), "no fault, no removal: {evs:?}");
}

#[test]
fn reassert_faults_an_unreachable_device_and_reopens_it_with_its_setting() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    let behavior = Arc::clone(&s.behavior);
    let b = FakeBackend::new("f", vec![s], &log);
    // Many failures allowed, so the resume retry time is what faults it.
    let timing = Timing {
        max_failures: 1000,
        ..fast_timing()
    };
    let e = start_with(cfg(60), vec![Box::new(b)], None, timing).unwrap();
    e.set_mode(&id(1), static_red()).unwrap();
    let events = e.subscribe();

    // The old handle is dead after the sleep: every reset fails until the resume retry time is over.
    behavior.lock().fail_release = true;
    assert!(e.reassert());
    match events.recv_timeout(SECOND).unwrap() {
        EngineEvent::DeviceFaulted { id: got, error } => {
            assert_eq!(got, id(1));
            assert!(error.contains("after resume"), "{error}");
        }
        ev => panic!("unexpected {ev:?}"),
    }
    // A fresh handle works; the resume loop re-opens the device and gives it its mode back.
    behavior.lock().fail_release = false;
    assert!(
        matches!(events.recv_timeout(SECOND).unwrap(), EngineEvent::DeviceAdded(info) if info.id == id(1))
    );
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting
        == Some(DeviceSetting::Mode { state: static_red() })));
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Active);
    assert_eq!(modes(&log, 1).last().map(String::as_str), Some("1"));
}

#[test]
fn reassert_reopens_a_device_removed_during_sleep() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    let b = FakeBackend::new("f", vec![s.clone()], &log);
    let plugged = Arc::clone(&b.plugged);
    let hp = FakeHotplug::default();
    let e = start_with(
        cfg(60),
        vec![Box::new(b)],
        Some(Box::new(hp.clone())),
        fast_timing(),
    )
    .unwrap();
    e.set_direct(&id(1), vec![GREEN; 7]).unwrap();
    assert!(shows(&log, 1, &[GREEN; 7]));
    let location = e.device(&id(1)).unwrap().description.info.location;

    // The removal of the sleep arrives before the resume broadcast.
    plugged.lock().clear();
    hp.inject(HotplugAction::Removed, &location);
    assert!(wait_until(SECOND, || e.device(&id(1)).is_none()));
    assert!(e.reassert());
    std::thread::sleep(Duration::from_millis(30));
    // USB is back: the next detection pass of the resume loop picks it up (no arrival event needed).
    plugged.lock().push(s);
    assert!(wait_until(SECOND, || e.device(&id(1)).is_some_and(|v| v.setting
        == Some(DeviceSetting::Direct {
            colors: vec![GREEN; 7]
        }))));
    assert_eq!(frames(&log, 1).len(), 2, "shown again on the fresh controller");
    assert!(
        entries(&log, "release").is_empty(),
        "a fresh controller needs no reset"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Dark zones and takeover info (review of ADR 0001 #36)

fn argb_dark(n: u16) -> BTreeMap<DeviceId, BTreeSet<String>> {
    BTreeMap::from([(id(n), BTreeSet::from(["ARGB 1".to_owned()]))])
}

/// Red with zone "ARGB 1" (LEDs 1..4) black.
const RED_ARGB_DARK: [Rgb; 7] = [RED, Rgb::BLACK, Rgb::BLACK, Rgb::BLACK, RED, RED, RED];

#[test]
fn dark_zone_stays_black_when_a_stored_direct_setting_is_taken_over() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let e = engine(60, vec![Box::new(b)]);
    drive(&e, &log, 1);
    e.release(&id(1)).unwrap();
    // Hidden while released: nothing is sent now.
    let n = frames(&log, 1).len();
    e.set_dark_zones(argb_dark(1));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(frames(&log, 1).len(), n);

    e.connect_all();
    assert!(shows(&log, 1, &RED_ARGB_DARK), "taken over with the zone black");
    e.release(&id(1)).unwrap();
    e.set_device_brightness(&id(1), 100).unwrap();
    assert!(shows(&log, 1, &RED_ARGB_DARK), "brightness takeover too");
}

#[test]
fn dark_zone_stays_black_in_a_queued_setting_and_in_colour_edits() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![spec(id(1), "A", DeviceKind::Light)], &log);
    let plugged = Arc::clone(&b.plugged);
    let hp = FakeHotplug::default();
    let e = start_with(
        EngineConfig {
            dark_zones: argb_dark(2),
            ..cfg(60)
        },
        vec![Box::new(b)],
        Some(Box::new(hp.clone())),
        fast_timing(),
    )
    .unwrap();
    let report = e.apply(&BTreeMap::from([(
        id(2),
        DeviceSetting::Direct { colors: vec![RED] },
    )]));
    assert_eq!(report.missing, [id(2)]);

    plugged.lock().push(spec(id(2), "B", DeviceKind::Light));
    hp.inject(HotplugAction::Arrived, r"\\?\hid#whatever");
    assert!(wait_until(SECOND, || e.device(&id(2)).is_some()));
    assert!(
        shows(&log, 2, &RED_ARGB_DARK),
        "the pending setting with the zone black"
    );

    e.set_direct(&id(2), vec![GREEN; 7]).unwrap();
    let mut want = [GREEN; 7];
    want[1..4].fill(Rgb::BLACK);
    assert!(shows(&log, 2, &want), "a colour edit with the zone black");
}

#[test]
fn new_dark_zones_re_send_a_device_driven_with_direct_colours() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            with_effects(spec(id(2), "B", DeviceKind::Light), false),
        ],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    drive(&e, &log, 1);
    e.set_mode(&id(2), static_red()).unwrap();
    let modes_2 = modes(&log, 2).len();

    let mut dark = argb_dark(1);
    dark.extend(argb_dark(2));
    e.set_dark_zones(dark);
    assert!(shows(&log, 1, &RED_ARGB_DARK), "re-sent with the zone black");
    assert_eq!(e.device(&id(1)).unwrap().colors, RED_ARGB_DARK);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(modes(&log, 2).len(), modes_2, "a hardware mode is left alone");

    // The same zones again change nothing, so nothing is sent.
    let n = frames(&log, 1).len();
    e.set_dark_zones(argb_dark(1));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(frames(&log, 1).len(), n);
}

#[test]
fn device_view_reports_what_a_takeover_would_use() {
    let log = Log::default();
    let readback = with_effects(spec(id(2), "B", DeviceKind::Light), true);
    readback.behavior.lock().hardware = Some(wave());
    let mut colourless = without_direct(4);
    colourless.desc.modes.retain(|m| m.color_mode == ColorMode::None);
    let broken = spec(id(5), "E", DeviceKind::Light);
    broken.behavior.lock().fatal_direct = true;
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            readback,
            without_direct(3),
            colourless,
            broken,
        ],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    let takeover = |n: u16| e.device(&id(n)).unwrap().takeover;
    let direct = Some(Takeover { mode_id: None });
    let mode = |m: u32| Some(Takeover { mode_id: Some(m) });

    assert_eq!(takeover(1), direct, "the default colour in direct mode");
    assert_eq!(takeover(2), mode(3), "the mode read back");
    assert_eq!(takeover(3), mode(2), "the first mode that takes a colour");
    assert_eq!(takeover(4), None, "nothing it could show");

    // Driven: what it is driven with; released afterwards: its last setting.
    e.set_mode(&id(1), static_red()).unwrap();
    assert_eq!(takeover(1), mode(1));
    e.release(&id(1)).unwrap();
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting
        == Some(DeviceSetting::Mode { state: static_red() })));
    assert_eq!(takeover(1), mode(1));
    drive(&e, &log, 1);
    assert_eq!(takeover(1), direct);

    // Faulted: cannot be taken over.
    e.set_direct(&id(5), vec![RED; 7]).unwrap();
    assert!(wait_until(SECOND, || matches!(
        e.device(&id(5)).unwrap().status,
        DeviceStatus::Faulted { .. }
    )));
    assert_eq!(takeover(5), None);
}

/// `spec` with every zone brightness-only (a GPU whose LEDs show a fixed white, like the RTX 5080 FE).
fn white_only(n: u16) -> Spec {
    let mut s = spec(id(n), "GPU", DeviceKind::Gpu);
    for z in &mut s.desc.zones {
        z.brightness_only = true;
    }
    s
}

#[test]
fn brightness_only_device_takes_over_at_full_white_and_reduces_colours_to_intensity() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![white_only(1)], &log);
    let e = engine(60, vec![Box::new(b)]);
    // The default colour does not matter: the takeover drives full white, scaled by brightness only.
    e.set_default_color(GREEN);
    e.set_device_brightness(&id(1), 50).unwrap();
    assert!(shows(&log, 1, &[Rgb::new(128, 128, 128); 7]));
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().colors == vec![Rgb::WHITE; 7]));

    // A colour is reduced to its highest channel: (40, 200, 10) -> 200, at 50 % -> 100.
    e.set_direct(&id(1), vec![Rgb::new(40, 200, 10); 7]).unwrap();
    assert!(shows(&log, 1, &[Rgb::new(100, 100, 100); 7]));
    let grey = DeviceSetting::Direct {
        colors: vec![Rgb::new(200, 200, 200); 7],
    };
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting == Some(grey.clone())));
    assert_eq!(e.device(&id(1)).unwrap().colors, vec![Rgb::new(200, 200, 200); 7]);
}

// ---------------------------------------------------------------------------------------------------------------
// Last good own lighting, release hardening, off mode (ADR 0001 #38)

/// A unique path for a machine state file in the system temp folder, removed on drop.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("cc-engine-{tag}-{}-{nanos}.toml", std::process::id())))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Faults device `n` (its next frame fails fatally), then lets it work again.
fn fault(e: &EngineHandle, s: &Spec, n: u16) {
    s.behavior.lock().fatal_direct = true;
    e.set_direct(&id(n), vec![RED; 7]).unwrap();
    assert!(wait_until(SECOND, || matches!(
        e.device(&id(n)).unwrap().status,
        DeviceStatus::Faulted { .. }
    )));
    s.behavior.lock().fatal_direct = false;
}

#[test]
fn own_lighting_is_recorded_once_and_handed_to_later_detections() {
    let file = TempFile::new("own-lighting");
    // Something remembered from an earlier run, for a device that is not connected.
    let mut state = HardwareState::load_from(file.0.clone());
    state.record_own_lighting(&id(9), &[9]).unwrap();

    let log = Log::default();
    let board = spec(id(1), "Board", DeviceKind::Motherboard);
    board.behavior.lock().own_lighting = Some(vec![1, 2, 3]);
    let other = spec(id(2), "Other", DeviceKind::Light);
    let b = FakeBackend::new("f", vec![board.clone(), other], &log);
    let contexts = Arc::clone(&b.contexts);
    let e = start_with(
        EngineConfig {
            hardware_state: state,
            ..cfg(60)
        },
        vec![Box::new(b)],
        None,
        fast_timing(),
    )
    .unwrap();

    // The first detection got what was stored; the board's readback is recorded and written.
    assert_eq!(
        contexts.lock()[0].own_lighting,
        BTreeMap::from([(id(9), vec![9])])
    );
    assert_eq!(e.hardware_state().get(&id(1)), Some(&[1u8, 2, 3][..]));
    assert_eq!(e.hardware_state().get(&id(2)), None, "nothing reported");
    let saved = HardwareState::load_from(file.0.clone());
    assert_eq!(saved.get(&id(1)), Some(&[1u8, 2, 3][..]));
    assert_eq!(saved.get(&id(9)), Some(&[9u8][..]));

    // Re-opened with the same readback: handed to the driver, not written again.
    std::fs::write(&file.0, "marker").unwrap();
    fault(&e, &board, 1);
    e.rescan();
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Released));
    assert_eq!(
        contexts.lock().last().unwrap().own_lighting.get(&id(1)),
        Some(&vec![1, 2, 3])
    );
    assert_eq!(std::fs::read_to_string(&file.0).unwrap(), "marker");

    // A different readback replaces it.
    board.behavior.lock().own_lighting = Some(vec![4]);
    fault(&e, &board, 1);
    e.rescan();
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().status
        == DeviceStatus::Released));
    let saved = HardwareState::load_from(file.0.clone());
    assert_eq!(saved.get(&id(1)), Some(&[4u8][..]));
    e.shutdown(SECOND);
}

#[test]
fn faulted_device_is_released_before_it_is_opened_again() {
    let log = Log::default();
    let s = spec(id(1), "A", DeviceKind::Light);
    let b = FakeBackend::new("f", vec![s.clone()], &log);
    let marks = Arc::clone(&b.detect_marks);
    let plugged = Arc::clone(&b.plugged);
    let e = engine(60, vec![Box::new(b)]);
    fault(&e, &s, 1);
    assert!(entries(&log, "release").is_empty());

    e.rescan();
    let releases: Vec<usize> = log
        .lock()
        .iter()
        .enumerate()
        .filter(|(_, (_, l))| l.ends_with(" release"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(releases.len(), 1, "the faulted controller released once");
    assert!(
        releases[0] < *marks.lock().last().unwrap(),
        "released before detection opened the device again"
    );
    assert_eq!(e.device(&id(1)).unwrap().status, DeviceStatus::Released);

    // Faulted again and gone: released once on the next detection, not again on later ones; shutdown does not
    // wait for the stopped worker.
    fault(&e, &s, 1);
    plugged.lock().clear();
    e.rescan();
    e.rescan();
    assert_eq!(entries(&log, "release").len(), 2);
    assert!(matches!(
        e.device(&id(1)).unwrap().status,
        DeviceStatus::Faulted { .. }
    ));
    let start = Instant::now();
    e.shutdown(SECOND);
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(entries(&log, "release").len(), 2);
}

/// Device `n` like a Mystic Light board: mode 5 "Static" (per-LED colours, brightness 0..=10) is its driver's off
/// mode.
fn with_off_mode(n: u16) -> Spec {
    let mut s = spec(id(n), "Board", DeviceKind::Motherboard);
    s.desc.modes.push(Mode {
        id: 5,
        name: "Static".into(),
        caps: ModeCaps::BRIGHTNESS | ModeCaps::PER_LED_COLOR,
        color_mode: ColorMode::PerLed,
        speed: None,
        brightness: Some(0..=10),
        colors: None,
    });
    s.behavior.lock().off_mode = Some(5);
    s
}

#[test]
fn brightness_zero_uses_the_driver_off_mode_instead_of_direct_black() {
    let log = Log::default();
    let b = FakeBackend::new("f", vec![with_off_mode(1), with_off_mode(2)], &log);
    let e = engine(60, vec![Box::new(b)]);

    // Driven with colours: 0 sends Static at brightness 0 (no black frame), back up sends the colours again.
    drive(&e, &log, 1);
    e.set_device_brightness(&id(1), 0).unwrap();
    assert!(shows_mode(&log, 1, "5 b=0"));
    assert_eq!(frames(&log, 1), [hex(&[RED; 7])]);
    // Colour edits at 0 are recorded but stay dark (the off mode is not re-sent).
    e.set_direct(&id(1), vec![GREEN; 7]).unwrap();
    assert!(wait_until(SECOND, || e.device(&id(1)).unwrap().setting
        == Some(DeviceSetting::Direct {
            colors: vec![GREEN; 7]
        })));
    assert_eq!(modes(&log, 1).len(), 1);
    assert_eq!(frames(&log, 1).len(), 1);
    e.set_device_brightness(&id(1), 100).unwrap();
    assert!(shows(&log, 1, &[GREEN; 7]));

    // A released device taken over at 0: straight to the off mode, never a direct frame.
    e.set_device_brightness(&id(2), 0).unwrap();
    assert!(shows_mode(&log, 2, "5 b=0"));
    assert!(wait_until(SECOND, || e.device(&id(2)).unwrap().status
        == DeviceStatus::Active));
    assert!(frames(&log, 2).is_empty());
    e.set_device_brightness(&id(2), 50).unwrap();
    assert!(shows(&log, 2, &[Rgb::new(128, 128, 128); 7]));
}

/// Run 7 (ADR 0001 #38, item 4): Take over all three times within seconds, each taking every device again. A second
/// Take over all leaves driven devices alone; only a release in between (Release all, which did not log at that
/// build) makes the next one take them again.
#[test]
fn take_over_all_retakes_devices_only_after_a_release() {
    let log = Log::default();
    let b = FakeBackend::new(
        "f",
        vec![
            spec(id(1), "A", DeviceKind::Light),
            spec(id(2), "B", DeviceKind::Light),
        ],
        &log,
    );
    let e = engine(60, vec![Box::new(b)]);
    e.connect_all();
    for n in [1, 2] {
        assert!(shows(&log, n, &[Rgb::WHITE; 7]));
    }
    let n = log.lock().len();
    e.connect_all();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(log.lock().len(), n, "nothing is taken twice");

    for round in 1..=3 {
        assert_eq!(e.release_all().released, 2);
        assert_eq!(entries(&log, "release").len(), 2 * round);
        e.connect_all();
        for n in [1, 2] {
            assert!(wait_until(SECOND, || frames(&log, n).len() == round + 1));
            assert!(wait_until(SECOND, || e.device(&id(n)).unwrap().status
                == DeviceStatus::Active));
        }
    }
}
