//! Shared engine state, detection, and the [`EngineHandle`] API.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cc_config::{DeviceSetting, HardwareSnapshot, HardwareState};
use cc_core::{
    Backend, Bus, Controller, DetectContext, Detected, DeviceDescription, DeviceError, DeviceId, EngineEvent,
    HotplugEvent, ModeState, Rgb,
};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::error::EngineError;
use crate::hotplug::{HotplugRuntime, HotplugSource};
use crate::lighting::{self, FULL, Levels, Output, Want, mask_hidden_zones};
use crate::resume::ResumeRuntime;
use crate::view::{
    ApplyFailure, ApplyReport, DetectFailure, DeviceStatus, DeviceView, LightingView, ReleaseReport, Takeover,
};
use crate::worker::{Command, FrameSlot, Reply, Timing, Worker};

/// Capacity of each device's command channel. Colour frames do not use it (they go through the frame slot), so a
/// small bound suffices.
const COMMAND_CAPACITY: usize = 16;
/// Detection failures kept for diagnostics; older ones are dropped.
const MAX_FAILURES: usize = 64;
/// Removals remembered for a resume; older ones are dropped.
const MAX_REMOVED: usize = 64;
/// A device removed this long before a resume counts as lost in the sleep (Windows may deliver the removals of a
/// sleep shortly before the resume broadcast).
const RESUME_LOOKBACK: Duration = Duration::from_secs(10);
/// Bounds of the frame rate.
const FPS_RANGE: std::ops::RangeInclusive<u32> = 1..=60;

/// Engine configuration. Not `Clone`: it owns the machine state, whose file only the engine writes.
#[derive(Debug)]
pub struct EngineConfig {
    /// Maximum colour frames per second and device, `1..=60`.
    pub frame_rate: u32,
    /// Family ids (see `cc_core::family`) whose detectors must not run.
    pub disabled_families: HashSet<String>,
    /// Zone lengths by zone name, applied right after a device is detected.
    pub zone_sizes: BTreeMap<DeviceId, BTreeMap<String, usize>>,
    /// Whether to watch for HID arrivals and removals.
    pub hotplug: bool,
    /// Whether to watch for the system resuming from sleep or hibernation and then
    /// [`reassert`](EngineHandle::reassert) every driven device.
    pub resume: bool,
    /// Master brightness in percent (clamped to 100), multiplied into every device (ADR 0001 #33).
    pub master_brightness: u8,
    /// Device brightness in percent per device; a device without an entry has 100.
    pub brightness: BTreeMap<DeviceId, u8>,
    /// Devices that brightness changes never take over (the app's hidden devices); see
    /// [`EngineHandle::set_excluded`].
    pub excluded: HashSet<DeviceId>,
    /// The colour a takeover shows on a device that has neither a setting nor a hardware readback (ADR 0001 #36);
    /// see [`EngineHandle::set_default_color`].
    pub default_color: Rgb,
    /// Zones, by name per device, that every direct output leaves black (the app's hidden zones); see
    /// [`EngineHandle::set_dark_zones`].
    pub dark_zones: BTreeMap<DeviceId, BTreeSet<String>>,
    /// The machine state remembered between runs (ADR 0001 #38): the last good own lighting of each device and the
    /// "unreleased" marker. Every detection hands both to the drivers (`DetectContext::own_lighting`,
    /// `DetectContext::unreleased`); what a newly opened controller reports through `Controller::own_lighting` is
    /// recorded in it; a worker sets the marker before its first write after a takeover and clears it after a
    /// successful release (when `Controller::shows_own_lighting`). The file is written only when a value changed.
    pub hardware_state: HardwareState,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            frame_rate: 30,
            disabled_families: HashSet::new(),
            zone_sizes: BTreeMap::new(),
            hotplug: true,
            resume: true,
            master_brightness: FULL,
            brightness: BTreeMap::new(),
            excluded: HashSet::new(),
            default_color: cc_config::DEFAULT_COLOR,
            dark_zones: BTreeMap::new(),
            hardware_state: HardwareState::in_memory(),
        }
    }
}

/// One controlled device.
struct Slot {
    family: String,
    generation: u64,
    description: DeviceDescription,
    status: DeviceStatus,
    setting: Option<DeviceSetting>,
    colors: Vec<Rgb>,
    /// What the device shows on its own, as its driver read it (validated against the description).
    hardware: Option<ModeState>,
    /// The driver's off mode (`Controller::off_mode`), used for brightness 0.
    off_mode: Option<u32>,
    /// What the engine drives the device with (unscaled), or `None` while it does not drive it (never set, or
    /// released). Brightness changes re-send only devices that have one.
    want: Option<Want>,
    /// A reassert after resume was sent and the worker has not confirmed an output since.
    resuming: bool,
    frame: FrameSlot,
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Slot {
    fn view(&self, brightness: u8, fallback: &Fallback) -> DeviceView {
        DeviceView {
            description: self.description.clone(),
            status: self.status.clone(),
            setting: self.setting.clone(),
            colors: self.colors.clone(),
            hardware: self.hardware.clone(),
            family: self.family.clone(),
            brightness,
            takeover: self.takeover(fallback),
        }
    }

    /// What a takeover drives the device with, as the view reports it: what the engine drives it with now, else what
    /// [`take_over`](Self::take_over) would pick. `None` when it cannot be taken over.
    fn takeover(&self, fallback: &Fallback) -> Option<Takeover> {
        if self.is_faulted() {
            return None;
        }
        let mode_id = match &self.want {
            Some(Want::Direct) => None,
            Some(Want::Mode(state)) => Some(state.mode),
            None => match self.takeover_setting(fallback)? {
                DeviceSetting::Direct { .. } => None,
                DeviceSetting::Mode { state } => Some(state.mode),
            },
        };
        Some(Takeover { mode_id })
    }

    fn check_usable(&self) -> Result<(), EngineError> {
        match &self.status {
            DeviceStatus::Faulted { error } => Err(EngineError::Faulted {
                id: self.description.info.id.clone(),
                error: error.clone(),
            }),
            _ => Ok(()),
        }
    }

    /// The unscaled setting the engine drives the device with, if it drives it with one.
    fn wanted_setting(&self) -> Option<DeviceSetting> {
        match self.want.as_ref()? {
            Want::Direct => Some(DeviceSetting::Direct {
                colors: self.colors.clone(),
            }),
            Want::Mode(state) => Some(DeviceSetting::Mode { state: state.clone() }),
        }
    }

    /// What the device is sent for its current want at `levels`, if the engine drives it.
    fn output(&self, levels: Levels) -> Option<Output> {
        self.want
            .as_ref()
            .map(|want| lighting::output_with(&self.description, self.off_mode, want, &self.colors, levels))
    }

    fn is_faulted(&self) -> bool {
        matches!(self.status, DeviceStatus::Faulted { .. })
    }

    /// What an undriven device is taken over with (ADR 0001 #33, #36): its last setting if it fits the device, else
    /// the mode it shows on its own (so nothing visibly changes until the levels do), else `fallback`'s colour (see
    /// [`lighting::default_setting`]). `None` for a faulted device and for a device without a direct mode and without
    /// a mode that takes a colour.
    fn takeover_setting(&self, fallback: &Fallback) -> Option<DeviceSetting> {
        if self.is_faulted() {
            return None;
        }
        match self
            .setting
            .clone()
            .map(|s| s.fit_to(self.description.led_count()))
        {
            Some(s @ DeviceSetting::Direct { .. }) if self.description.direct_mode.is_some() => Some(s),
            Some(DeviceSetting::Mode { state }) if self.description.validate_state(&state).is_ok() => {
                Some(DeviceSetting::Mode { state })
            }
            _ => match self.hardware.clone() {
                Some(state) => Some(DeviceSetting::Mode { state }),
                None => lighting::default_setting(&self.description, fallback.color, &fallback.dark),
            },
        }
    }

    /// Makes the engine drive an undriven device with [`takeover_setting`](Self::takeover_setting); direct colours
    /// get the device's dark zones black. Returns whether the device is driven afterwards.
    fn take_over(&mut self, fallback: &Fallback) -> bool {
        if self.want.is_some() {
            return true;
        }
        let Some(setting) = self.takeover_setting(fallback) else {
            return false;
        };
        self.want = Some(match setting {
            DeviceSetting::Direct { mut colors } => {
                mask_hidden_zones(&self.description, &fallback.dark, &mut colors);
                self.description.reduce_fixed_colors(&mut colors);
                self.colors = colors;
                Want::Direct
            }
            DeviceSetting::Mode { state } => Want::Mode(state),
        });
        true
    }

    /// Hands `output` to the worker as the latest frame (sequence `seq`). The setting is recorded by the worker once
    /// the output has actually been sent.
    fn push_output(&mut self, seq: u64, output: Output) {
        *self.frame.lock() = Some((seq, output));
        // Full channel: the worker is busy and will pick the frame up from the slot anyway.
        let _ = self.tx.try_send(Command::Wake);
    }
}

/// What a takeover of one device needs besides the device itself: the default colour (for a device with neither a
/// setting nor a hardware readback) and the device's dark zones.
struct Fallback {
    color: Rgb,
    dark: BTreeSet<String>,
}

struct State {
    devices: BTreeMap<DeviceId, Slot>,
    failures: Vec<DetectFailure>,
    pending: BTreeMap<DeviceId, DeviceSetting>,
    /// Driven devices that were removed recently: when, and what they were driven with (for a resume).
    removed: BTreeMap<DeviceId, (Instant, DeviceSetting)>,
    disabled_families: HashSet<String>,
    zone_sizes: BTreeMap<DeviceId, BTreeMap<String, usize>>,
    /// Devices brightness changes never take over.
    excluded: HashSet<DeviceId>,
    /// Master brightness, percent.
    master: u8,
    /// Device brightness by id (entries of 100 are dropped).
    brightness: BTreeMap<DeviceId, u8>,
    /// The takeover's default colour.
    default_color: Rgb,
    /// Zones every direct output leaves black, by device.
    dark_zones: BTreeMap<DeviceId, BTreeSet<String>>,
}

impl State {
    fn levels(&self, id: &DeviceId) -> Levels {
        Levels {
            device: self.device_brightness(id),
            master: self.master,
        }
    }

    fn device_brightness(&self, id: &DeviceId) -> u8 {
        self.brightness.get(id).copied().unwrap_or(FULL)
    }

    fn view(&self, id: &DeviceId, slot: &Slot) -> DeviceView {
        slot.view(self.device_brightness(id), &self.fallback(id))
    }

    /// The default colour and the dark zones of device `id`, for a takeover.
    fn fallback(&self, id: &DeviceId) -> Fallback {
        Fallback {
            color: self.default_color,
            dark: self.dark_zones.get(id).cloned().unwrap_or_default(),
        }
    }

    /// Takes over device `id` unless it is excluded (see [`Slot::take_over`]).
    fn take_over(&mut self, id: &DeviceId) {
        if self.excluded.contains(id) {
            return;
        }
        let fallback = self.fallback(id);
        if let Some(slot) = self.devices.get_mut(id) {
            slot.take_over(&fallback);
        }
    }

    /// Takes over every undriven device that is not excluded (see [`Slot::take_over`]); returns the devices it took.
    fn take_over_all(&mut self) -> Vec<DeviceId> {
        let ids: Vec<DeviceId> = self
            .devices
            .iter()
            .filter(|(id, s)| s.want.is_none() && !self.excluded.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();
        let mut taken = Vec::new();
        for id in ids {
            let fallback = self.fallback(&id);
            if let Some(slot) = self.devices.get_mut(&id)
                && slot.take_over(&fallback)
            {
                debug!(%id, "device taken over");
                taken.push(id);
            }
        }
        taken
    }
}

/// Device brightness map without entries of 100, values clamped to 100.
fn normalized_brightness(map: BTreeMap<DeviceId, u8>) -> BTreeMap<DeviceId, u8> {
    map.into_iter()
        .map(|(id, b)| (id, b.min(FULL)))
        .filter(|(_, b)| *b < FULL)
        .collect()
}

/// State shared by the handle, the device threads and the hot-plug thread.
pub(crate) struct Shared {
    state: Mutex<State>,
    subscribers: Mutex<Vec<Sender<EngineEvent>>>,
    backends: Vec<Box<dyn Backend>>,
    frame_interval_ns: AtomicU64,
    /// Set once shutdown starts: API calls are rejected, no events go out, detection stops adding devices.
    closed: AtomicBool,
    /// Makes shutdown idempotent.
    shutdown_started: AtomicBool,
    /// Orders frames, modes, releases and resizes (see the worker module).
    next_seq: AtomicU64,
    /// Serializes detection passes and shutdown.
    detect_lock: Mutex<()>,
    hotplug: Mutex<Option<HotplugRuntime>>,
    resume: Mutex<Option<ResumeRuntime>>,
    /// When the last reassert started (for the debounce).
    last_reassert: Mutex<Option<Instant>>,
    /// The remembered machine state (own lighting per device); see [`EngineConfig::hardware_state`].
    hardware_state: Mutex<HardwareState>,
    next_generation: AtomicU64,
    pub(crate) timing: Timing,
}

fn interval_ns(fps: u32) -> u64 {
    1_000_000_000 / u64::from(fps.clamp(*FPS_RANGE.start(), *FPS_RANGE.end()))
}

impl Shared {
    pub(crate) fn frame_interval(&self) -> Duration {
        Duration::from_nanos(self.frame_interval_ns.load(Ordering::Relaxed))
    }

    fn next_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn emit(&self, ev: EngineEvent) {
        if self.is_closed() {
            return;
        }
        self.subscribers.lock().retain(|s| s.send(ev.clone()).is_ok());
    }

    /// Called by a worker when its device's status changes (taken or released); emits `DeviceStateChanged` when it
    /// did change. Ignored if the slot was replaced or removed meanwhile, or is faulted.
    pub(crate) fn set_status(&self, id: &DeviceId, generation: u64, status: DeviceStatus) {
        let changed = {
            let mut st = self.state.lock();
            match st.devices.get_mut(id).filter(|s| s.generation == generation) {
                Some(slot) if !slot.is_faulted() && slot.status != status => {
                    slot.status = status.clone();
                    true
                }
                _ => false,
            }
        };
        if changed {
            debug!(%id, ?status, "device status changed");
            self.emit(EngineEvent::DeviceStateChanged {
                id: id.clone(),
                active: status == DeviceStatus::Active,
                state: None,
            });
        }
    }

    /// Called by a worker with what its driver reads as the device's own lighting (after a release).
    pub(crate) fn set_hardware(&self, id: &DeviceId, generation: u64, hardware: Option<ModeState>) {
        let mut st = self.state.lock();
        if let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) {
            slot.hardware = hardware.filter(|h| slot.description.validate_state(h).is_ok());
        }
    }

    /// Called by a worker after an output went out: records the unscaled `setting` it stands for. With `event` (an
    /// explicit mode change, not a brightness step) a mode also emits `DeviceStateChanged`.
    pub(crate) fn applied(&self, id: &DeviceId, generation: u64, setting: DeviceSetting, event: bool) {
        {
            let mut st = self.state.lock();
            let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) else {
                return;
            };
            slot.setting = Some(setting.clone());
        }
        if let (true, DeviceSetting::Mode { state }) = (event, setting) {
            self.emit(EngineEvent::DeviceStateChanged {
                id: id.clone(),
                active: true,
                state: Some(state),
            });
        }
    }

    /// Re-sends every driven, usable device at the current brightness levels (or only `only`). Runs under the state
    /// lock, so sequence numbers and wants stay consistent with concurrent edits.
    fn refresh_locked(&self, st: &mut State, only: Option<&DeviceId>) {
        let ids: Vec<DeviceId> = st
            .devices
            .iter()
            .filter(|(id, s)| {
                only.is_none_or(|o| o == *id)
                    && s.want.is_some()
                    && !matches!(s.status, DeviceStatus::Faulted { .. })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let levels = st.levels(&id);
            let seq = self.next_seq();
            if let Some(slot) = st.devices.get_mut(&id)
                && let Some(output) = slot.output(levels)
            {
                slot.push_output(seq, output);
            }
        }
    }

    /// Sets device `id` to direct colours `colors` (already checked) with its dark zones black and sends them at the
    /// current levels.
    fn drive_direct(&self, st: &mut State, id: &DeviceId, mut colors: Vec<Rgb>) {
        let levels = st.levels(id);
        let seq = self.next_seq();
        if let Some(slot) = st.devices.get_mut(id) {
            if let Some(dark) = st.dark_zones.get(id) {
                mask_hidden_zones(&slot.description, dark, &mut colors);
            }
            slot.description.reduce_fixed_colors(&mut colors);
            slot.colors = colors;
            slot.want = Some(Want::Direct);
            if let Some(output) = slot.output(levels) {
                slot.push_output(seq, output);
            }
        }
    }

    /// Records `state` as what device `id` is driven with and returns the sequence number and output of the
    /// `SetMode` command that applies it.
    fn want_mode(&self, st: &mut State, id: &DeviceId, state: &ModeState) -> Option<(u64, lighting::Out)> {
        let levels = st.levels(id);
        let slot = st.devices.get_mut(id)?;
        slot.want = Some(Want::Mode(state.clone()));
        let output = slot.output(levels)?;
        Some((self.next_seq(), output.out))
    }

    /// Called by a worker after zone `zone` was resized: adopts the new description, carries the colour buffer and a
    /// Direct setting over, and remembers the size by zone name. If the device shows direct colours, or a pending
    /// frame was dropped because of the resize, the carried-over buffer is queued as a new frame.
    pub(crate) fn resized(
        &self,
        id: &DeviceId,
        generation: u64,
        zone: usize,
        desc: DeviceDescription,
        hardware: Option<ModeState>,
        dropped: bool,
    ) {
        let mut st = self.state.lock();
        let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) else {
            return;
        };
        let colors = remap_buffer(&slot.description, &desc, &slot.colors);
        let direct = matches!(slot.want, Some(Want::Direct));
        if let Some(DeviceSetting::Direct { colors: c }) = &mut slot.setting {
            *c = remap_buffer(&slot.description, &desc, c);
        }
        let name = desc.zones.get(zone).map(|z| (z.name.clone(), z.len));
        slot.hardware = hardware.filter(|h| desc.validate_state(h).is_ok());
        slot.description = desc;
        slot.colors = colors;
        if let Some((name, len)) = name {
            st.zone_sizes.entry(id.clone()).or_default().insert(name, len);
        }
        if direct || dropped {
            self.refresh_locked(&mut st, Some(id));
        }
    }

    /// Called by a worker whose first output after a reassert went out.
    pub(crate) fn reasserted(&self, id: &DeviceId, generation: u64) {
        let mut st = self.state.lock();
        if let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) {
            slot.resuming = false;
        }
    }

    /// Called by a worker that took its device out of service.
    pub(crate) fn device_faulted(&self, id: &DeviceId, generation: u64, error: String) {
        {
            let mut st = self.state.lock();
            let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) else {
                return;
            };
            slot.status = DeviceStatus::Faulted { error: error.clone() };
            slot.resuming = false;
        }
        self.emit(EngineEvent::DeviceFaulted {
            id: id.clone(),
            error,
        });
    }

    /// Runs detection on every backend (or only those on `bus`), skipping devices already under control, and starts
    /// the new ones. New devices get their pending setting.
    pub(crate) fn detect_new(this: &Arc<Self>, bus: Option<Bus>) {
        let _guard = this.detect_lock.lock();
        if this.is_closed() {
            return;
        }
        let disabled = this.state.lock().disabled_families.clone();
        let in_pass = |backend: &dyn Backend| {
            bus.is_none_or(|b| backend.bus() == b) && !disabled.contains(backend.name())
        };
        let families: HashSet<&str> = this
            .backends
            .iter()
            .filter(|b| in_pass(b.as_ref()))
            .map(|b| b.name())
            .collect();
        // Faulted devices are retried: a fresh controller replaces the faulted one. The faulted controller hands its
        // device back first (best effort), so the new one does not read our own output as the device's lighting. A
        // device whose old worker did not stop in time is left for a later pass.
        let still_running = this.release_faulted(&families);
        let (own_lighting, unreleased) = {
            let hw = this.hardware_state.lock();
            (
                hw.own_lighting().clone(),
                hw.unreleased().iter().cloned().collect(),
            )
        };
        let ctx = {
            let st = this.state.lock();
            DetectContext {
                disabled_families: disabled.clone(),
                skip: st
                    .devices
                    .iter()
                    .filter(|(_, s)| !matches!(s.status, DeviceStatus::Faulted { .. }))
                    .map(|(id, _)| id.clone())
                    .chain(still_running)
                    .collect(),
                own_lighting,
                unreleased,
            }
        };
        let mut found = Vec::new();
        let mut failures = Vec::new();
        for backend in &this.backends {
            if this.is_closed() {
                return;
            }
            if !in_pass(backend.as_ref()) {
                continue;
            }
            for d in backend.detect(&ctx) {
                match d {
                    Detected::Device(ctrl) => found.push((backend.name().to_string(), ctrl)),
                    Detected::Failed {
                        family,
                        location,
                        error,
                    } => {
                        warn!(%family, %location, %error, "device found but could not be brought up");
                        failures.push(DetectFailure {
                            family,
                            location,
                            error,
                        });
                    }
                }
            }
        }
        {
            let mut st = this.state.lock();
            st.failures.extend(failures);
            let excess = st.failures.len().saturating_sub(MAX_FAILURES);
            st.failures.drain(..excess);
        }
        for (family, ctrl) in found {
            if this.is_closed() {
                return;
            }
            Self::add_device(this, family, ctrl);
        }
    }

    /// Stops the worker of every faulted device of `families` that still has one, releasing its device first (best
    /// effort): all stops are sent first, then all are awaited until one deadline ([`Timing::faulted_release`]). The
    /// slot stays (faulted, without a worker) until detection replaces it. Returns the devices whose worker did not
    /// end in time: their thread handle goes back into the slot, and they must not be opened again in this pass, so
    /// an old worker never writes to a device a new controller opened. A later pass tries again.
    fn release_faulted(&self, families: &HashSet<&str>) -> Vec<DeviceId> {
        type Stopping = (
            DeviceId,
            u64,
            Option<Receiver<Result<(), DeviceError>>>,
            JoinHandle<()>,
        );
        let stopping: Vec<Stopping> = {
            let mut st = self.state.lock();
            st.devices
                .iter_mut()
                .filter(|(_, s)| s.is_faulted() && families.contains(s.family.as_str()))
                .filter_map(|(id, s)| {
                    let thread = s.thread.take()?;
                    let (reply, rx) = crossbeam_channel::bounded(1);
                    let sent = s.tx.try_send(Command::Stop {
                        release: true,
                        reply: Some(reply),
                    });
                    // A full queue means a worker stuck in a driver call: no answer to wait for. A closed one means
                    // it already ended.
                    let rx = match sent {
                        Ok(()) => Some(rx),
                        Err(crossbeam_channel::TrySendError::Full(_)) => None,
                        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                            let (_, closed) = crossbeam_channel::bounded(0);
                            Some(closed)
                        }
                    };
                    Some((id.clone(), s.generation, rx, thread))
                })
                .collect()
        };
        let deadline = Instant::now() + self.timing.faulted_release;
        let mut still_running = Vec::new();
        for (id, generation, rx, thread) in stopping {
            let ended = match rx.map(|rx| rx.recv_deadline(deadline)) {
                Some(Ok(r)) => {
                    match r {
                        Ok(()) => debug!(%id, "faulted device released before re-opening it"),
                        Err(e) => {
                            debug!(%id, error = %e, "release of the faulted device before re-opening it failed")
                        }
                    }
                    true
                }
                Some(Err(crossbeam_channel::RecvTimeoutError::Disconnected)) => true,
                Some(Err(crossbeam_channel::RecvTimeoutError::Timeout)) | None => thread.is_finished(),
            };
            if ended {
                // The worker ends right after it answered (or already has).
                if thread.join().is_err() {
                    warn!(%id, "device thread panicked");
                }
                continue;
            }
            warn!(%id, "faulted device's worker did not stop in time; not re-opening it in this pass");
            let mut st = self.state.lock();
            if let Some(slot) = st.devices.get_mut(&id).filter(|s| s.generation == generation) {
                slot.thread = Some(thread);
            }
            still_running.push(id);
        }
        still_running
    }

    /// Remembers what a newly opened controller reports as the device's own lighting (ADR 0001 #38), and clears the
    /// device's "unreleased" marker when the controller found (or restored) its own lighting.
    fn record_open(&self, id: &DeviceId, ctrl: &dyn Controller) {
        if let Some(bytes) = ctrl.own_lighting() {
            match self.hardware_state.lock().record_own_lighting(id, &bytes) {
                Ok(true) => info!(%id, "own lighting remembered"),
                Ok(false) => {}
                Err(e) => warn!(%id, error = %e, "cannot save the device's own lighting"),
            }
        }
        if ctrl.shows_own_lighting() {
            self.set_unreleased(id, false);
        }
    }

    /// Sets or clears the persisted "unreleased" marker of `id` (ADR 0001 #38): CaseControl wrote to the device and
    /// did not hand it back. Writes the machine state file only on a change; a write error is logged and the marker
    /// keeps its old value (the next call tries again).
    pub(crate) fn set_unreleased(&self, id: &DeviceId, unreleased: bool) {
        match self.hardware_state.lock().set_unreleased(id, unreleased) {
            Ok(true) => debug!(%id, unreleased, "unreleased marker changed"),
            Ok(false) => {}
            Err(e) => warn!(%id, unreleased, error = %e, "cannot save the device's unreleased marker"),
        }
    }

    fn add_device(this: &Arc<Self>, family: String, mut ctrl: Box<dyn Controller>) {
        let id = ctrl.description().info.id.clone();
        if let Err(e) = ctrl.description().validate() {
            warn!(%id, error = %e, "driver returned an invalid description; device ignored");
            this.record_failure(&family, ctrl.as_ref(), DeviceError::Protocol(e.to_string()));
            return;
        }
        let (zone_sizes, duplicate) = {
            let st = this.state.lock();
            (
                st.zone_sizes.get(&id).cloned().unwrap_or_default(),
                st.devices
                    .get(&id)
                    .is_some_and(|s| !matches!(s.status, DeviceStatus::Faulted { .. })),
            )
        };
        if duplicate {
            warn!(%id, "device detected twice; second instance ignored");
            return;
        }
        this.record_open(&id, ctrl.as_ref());
        for (name, len) in zone_sizes {
            let desc = ctrl.description();
            let Some(zone) = desc.zones.iter().position(|z| z.name == name) else {
                warn!(%id, zone = %name, "configured zone size for unknown zone");
                continue;
            };
            if desc.zones[zone].len == len {
                continue;
            }
            if let Err(e) = ctrl.resize_zone(zone, len) {
                warn!(%id, zone = %name, len, error = %e, "could not apply configured zone size");
            }
        }

        let description = ctrl.description().clone();
        let hardware = ctrl
            .hardware_mode()
            .filter(|h| description.validate_state(h).is_ok());
        let off_mode = ctrl.off_mode();
        // The colour buffer starts with what the device shows, where that is known per LED, so a first zone edit
        // keeps the other zones as they are.
        let colors = match &hardware {
            Some(h) if h.colors.len() == description.led_count() => h.colors.clone(),
            _ => vec![Rgb::BLACK; description.led_count()],
        };
        let generation = this.next_generation.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
        let frame: FrameSlot = Arc::default();
        let worker = Worker::new(
            id.clone(),
            generation,
            ctrl,
            rx,
            Arc::clone(&frame),
            Arc::clone(this),
        );
        let thread = std::thread::Builder::new()
            .name(format!("cc-dev-{id}"))
            .spawn(move || worker.run());
        let thread = match thread {
            Ok(t) => t,
            Err(e) => {
                // The worker (and with it the controller) was dropped inside the failed spawn.
                warn!(%id, error = %e, "could not start device thread");
                return;
            }
        };
        let info = description.info.clone();
        let slot = Slot {
            family,
            generation,
            colors,
            description,
            status: DeviceStatus::Released,
            setting: None,
            hardware,
            off_mode,
            want: None,
            resuming: false,
            frame,
            tx,
            thread: Some(thread),
        };
        let (pending, replaced) = {
            let mut st = this.state.lock();
            if this.is_closed() {
                // Shutdown started meanwhile; the new device was never driven, so it needs no release.
                let _ = slot.tx.try_send(Command::Stop {
                    release: false,
                    reply: None,
                });
                return;
            }
            let replaced = st.devices.insert(id.clone(), slot);
            st.removed.remove(&id);
            // A device that faulted after a resume comes back with what it was driven with, unless a profile wants
            // it otherwise.
            let carried = replaced
                .as_ref()
                .filter(|_| this.resume_active())
                .and_then(Slot::wanted_setting);
            (st.pending.remove(&id).or(carried), replaced)
        };
        if let Some(old) = replaced {
            info!(%id, "faulted device detected again; replacing it");
            // The faulted worker no longer talks to the device (detection already stopped it after a release if it
            // could); let it end on its own.
            let _ = old.tx.try_send(Command::Stop {
                release: false,
                reply: None,
            });
        }
        info!(%id, name = %info.name, "device added");
        this.emit(EngineEvent::DeviceAdded(info));
        if let Some(setting) = pending
            && let Err(e) = this.queue_setting(&id, setting)
        {
            warn!(%id, error = %e, "pending setting could not be applied");
        }
    }

    /// Applies `setting` without waiting for the device (used during detection, which must not block on acks).
    fn queue_setting(&self, id: &DeviceId, setting: DeviceSetting) -> Result<(), EngineError> {
        let mut st = self.state.lock();
        let slot = st
            .devices
            .get(id)
            .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
        match setting.fit_to(slot.description.led_count()) {
            DeviceSetting::Direct { colors } => {
                check_has_direct(&slot.description)?;
                self.drive_direct(&mut st, id, colors);
            }
            DeviceSetting::Mode { state } => {
                slot.description
                    .validate_state(&state)
                    .map_err(|e| EngineError::InvalidArgument(e.to_string()))?;
                let tx = slot.tx.clone();
                let (seq, out) = self
                    .want_mode(&mut st, id, &state)
                    .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
                let (reply, _) = crossbeam_channel::bounded(1);
                tx.try_send(Command::SetMode {
                    seq,
                    state,
                    out,
                    reply,
                })
                .map_err(|_| EngineError::Timeout)?;
            }
        }
        Ok(())
    }

    fn record_failure(&self, family: &str, ctrl: &dyn Controller, error: DeviceError) {
        let mut st = self.state.lock();
        st.failures.push(DetectFailure {
            family: family.to_string(),
            location: ctrl.description().info.location.clone(),
            error,
        });
        let excess = st.failures.len().saturating_sub(MAX_FAILURES);
        st.failures.drain(..excess);
    }

    /// Handles a HID removal: drops the device whose location equals `path` (case-insensitive). If none matches,
    /// falls back to the open HID devices with the event's vid/pid and drops each whose location is no longer among
    /// the paths `source` currently enumerates. Removed devices are not released: they are gone.
    pub(crate) fn handle_removal(&self, ev: &HotplugEvent, source: &dyn HotplugSource) {
        let (exact, candidates) = {
            let st = self.state.lock();
            let exact = st
                .devices
                .iter()
                .find(|(_, s)| s.description.info.location.eq_ignore_ascii_case(&ev.path))
                .map(|(id, _)| id.clone());
            let candidates: Vec<(DeviceId, String)> = st
                .devices
                .iter()
                .filter(|(id, s)| {
                    s.description.info.bus == Bus::Hid && hid_vid_pid(id) == Some((ev.vid, ev.pid))
                })
                .map(|(id, s)| (id.clone(), s.description.info.location.clone()))
                .collect();
            (exact, candidates)
        };
        if let Some(id) = exact {
            self.drop_removed(&id);
            return;
        }
        if candidates.is_empty() {
            debug!(path = %ev.path, "removal of a device we do not control");
            return;
        }
        let present = match source.present_paths() {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "could not enumerate HID devices to resolve a removal");
                return;
            }
        };
        for (id, location) in candidates {
            if !present.iter().any(|p| p.eq_ignore_ascii_case(&location)) {
                self.drop_removed(&id);
            }
        }
    }

    fn drop_removed(&self, id: &DeviceId) {
        let slot = {
            let mut st = self.state.lock();
            let Some(slot) = st.devices.remove(id) else {
                return;
            };
            if let Some(setting) = slot.wanted_setting() {
                // Lost after a resume: it gets what it was driven with when it comes back, unless a profile wants
                // otherwise. Outside a resume a re-plugged device starts undriven, as before.
                if self.resume_active() {
                    st.pending.entry(id.clone()).or_insert_with(|| setting.clone());
                }
                if st.removed.len() >= MAX_REMOVED
                    && let Some(oldest) = st
                        .removed
                        .iter()
                        .min_by_key(|(_, (t, _))| *t)
                        .map(|(k, _)| k.clone())
                {
                    st.removed.remove(&oldest);
                }
                st.removed.insert(id.clone(), (Instant::now(), setting));
            }
            slot
        };
        info!(%id, "device removed");
        // The worker may be blocked on the dead device; do not wait for it. Dropping the sender ends it.
        let _ = slot.tx.try_send(Command::Stop {
            release: false,
            reply: None,
        });
        drop(slot);
        self.emit(EngineEvent::DeviceRemoved(id.clone()));
    }

    fn slot_tx(&self, id: &DeviceId) -> Result<Sender<Command>, EngineError> {
        if self.is_closed() {
            return Err(EngineError::ShutDown);
        }
        let st = self.state.lock();
        let slot = st
            .devices
            .get(id)
            .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
        slot.check_usable()?;
        Ok(slot.tx.clone())
    }

    /// Sends a command built by `make` and waits for its reply within the ack timeout.
    fn acked(
        &self,
        id: &DeviceId,
        tx: Sender<Command>,
        make: impl FnOnce(Reply) -> Command,
    ) -> Result<(), EngineError> {
        self.acked_within(id, tx, make, self.timing.ack_timeout)
    }

    fn acked_within(
        &self,
        id: &DeviceId,
        tx: Sender<Command>,
        make: impl FnOnce(Reply) -> Command,
        timeout: Duration,
    ) -> Result<(), EngineError> {
        let deadline = Instant::now() + timeout;
        let (rtx, rrx) = crossbeam_channel::bounded(1);
        tx.send_deadline(make(rtx), deadline).map_err(|e| match e {
            crossbeam_channel::SendTimeoutError::Timeout(_) => EngineError::Timeout,
            crossbeam_channel::SendTimeoutError::Disconnected(_) => EngineError::UnknownDevice(id.clone()),
        })?;
        match rrx.recv_deadline(deadline) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(DeviceError::Faulted(error))) => Err(EngineError::Faulted {
                id: id.clone(),
                error,
            }),
            Ok(Err(e)) => Err(EngineError::Device(e)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Err(EngineError::Timeout),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                Err(EngineError::UnknownDevice(id.clone()))
            }
        }
    }

    fn set_direct(&self, id: &DeviceId, colors: Vec<Rgb>) -> Result<(), EngineError> {
        if self.is_closed() {
            return Err(EngineError::ShutDown);
        }
        let mut st = self.state.lock();
        let slot = st
            .devices
            .get_mut(id)
            .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
        slot.check_usable()?;
        check_has_direct(&slot.description)?;
        let expected = slot.description.led_count();
        if colors.len() != expected {
            return Err(EngineError::InvalidArgument(format!(
                "{} colors for {expected} LEDs",
                colors.len()
            )));
        }
        self.drive_direct(&mut st, id, colors);
        Ok(())
    }

    fn set_zone(&self, id: &DeviceId, zone: usize, colors: Vec<Rgb>) -> Result<(), EngineError> {
        if self.is_closed() {
            return Err(EngineError::ShutDown);
        }
        let mut st = self.state.lock();
        let slot = st
            .devices
            .get_mut(id)
            .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
        slot.check_usable()?;
        check_has_direct(&slot.description)?;
        let range = slot
            .description
            .zones
            .get(zone)
            .map(|z| z.range())
            .ok_or_else(|| EngineError::InvalidArgument(format!("no zone {zone}")))?;
        if colors.len() != range.len() {
            return Err(EngineError::InvalidArgument(format!(
                "{} colors for zone {zone} with {} LEDs",
                colors.len(),
                range.len()
            )));
        }
        let mut buffer = slot.colors.clone();
        buffer[range].copy_from_slice(&colors);
        self.drive_direct(&mut st, id, buffer);
        Ok(())
    }

    fn set_mode(&self, id: &DeviceId, state: ModeState) -> Result<(), EngineError> {
        let tx = self.slot_tx(id)?;
        let (seq, out) = {
            let mut st = self.state.lock();
            let slot = st
                .devices
                .get(id)
                .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
            slot.description
                .validate_state(&state)
                .map_err(|e| EngineError::InvalidArgument(e.to_string()))?;
            self.want_mode(&mut st, id, &state)
                .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?
        };
        // The worker records the setting and emits DeviceStateChanged when (and if) it applies the mode.
        self.acked(id, tx, move |reply| Command::SetMode {
            seq,
            state,
            out,
            reply,
        })
    }

    fn resize_zone(&self, id: &DeviceId, zone: usize, len: usize) -> Result<(), EngineError> {
        let tx = self.slot_tx(id)?;
        {
            let st = self.state.lock();
            let slot = st
                .devices
                .get(id)
                .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
            let z = slot
                .description
                .zones
                .get(zone)
                .ok_or_else(|| EngineError::InvalidArgument(format!("no zone {zone}")))?;
            match &z.resizable {
                Some(r) if r.contains(&len) => {}
                Some(r) => {
                    return Err(EngineError::InvalidArgument(format!(
                        "zone {zone} length must be in {}..={}",
                        r.start(),
                        r.end()
                    )));
                }
                None => {
                    return Err(EngineError::InvalidArgument(format!(
                        "zone {zone} is not resizable"
                    )));
                }
            }
        }
        // The worker updates description, buffer and remembered size when the resize actually happens.
        let seq = self.next_seq();
        self.acked(id, tx, move |reply| Command::Resize {
            seq,
            zone,
            len,
            reply,
        })
    }

    fn save_to_hardware(&self, id: &DeviceId) -> Result<(), EngineError> {
        let tx = self.slot_tx(id)?;
        self.acked(id, tx, Command::Save)
    }

    fn release(&self, id: &DeviceId) -> Result<(), EngineError> {
        let tx = self.slot_tx(id)?;
        // Stop driving it before the sequence number is taken, so no later brightness step takes it back.
        let seq = {
            let mut st = self.state.lock();
            if let Some(slot) = st.devices.get_mut(id) {
                slot.want = None;
            }
            self.next_seq()
        };
        self.acked(id, tx, move |reply| Command::Release { seq, reply })
    }

    fn set_device_brightness(&self, id: &DeviceId, value: u8) -> Result<(), EngineError> {
        if self.is_closed() {
            return Err(EngineError::ShutDown);
        }
        let mut st = self.state.lock();
        if !st.devices.contains_key(id) {
            return Err(EngineError::UnknownDevice(id.clone()));
        }
        let value = value.min(FULL);
        if value == FULL {
            st.brightness.remove(id);
        } else {
            st.brightness.insert(id.clone(), value);
        }
        st.take_over(id);
        self.refresh_locked(&mut st, Some(id));
        Ok(())
    }

    fn replace_brightness(&self, map: BTreeMap<DeviceId, u8>) {
        if self.is_closed() {
            return;
        }
        let mut st = self.state.lock();
        st.brightness = normalized_brightness(map);
        self.refresh_locked(&mut st, None);
    }

    fn set_master_brightness(&self, value: u8) {
        if self.is_closed() {
            return;
        }
        let mut st = self.state.lock();
        st.master = value.min(FULL);
        st.take_over_all();
        self.refresh_locked(&mut st, None);
    }

    /// Connect all: takes over every undriven device that is not excluded or faulted, as brightness does (last
    /// setting, else the hardware readback, else the default colour).
    fn connect_all(&self) {
        if self.is_closed() {
            return;
        }
        let mut st = self.state.lock();
        let taken = st.take_over_all();
        for id in &taken {
            self.refresh_locked(&mut st, Some(id));
        }
        info!(taken = taken.len(), "connect all");
    }

    fn set_excluded(&self, excluded: HashSet<DeviceId>) {
        self.state.lock().excluded = excluded;
    }

    /// Replaces the dark zones and re-sends every device driven with direct colours whose buffer they change, with
    /// those zones black. All under the state lock, so no colour edit slips in between with the old zones.
    fn set_dark_zones(&self, zones: BTreeMap<DeviceId, BTreeSet<String>>) {
        let mut st = self.state.lock();
        st.dark_zones = zones;
        let mut changed = Vec::new();
        let State {
            devices, dark_zones, ..
        } = &mut *st;
        for (id, slot) in devices.iter_mut() {
            if slot.is_faulted() || slot.want != Some(Want::Direct) {
                continue;
            }
            let Some(dark) = dark_zones.get(id) else {
                continue;
            };
            let mut colors = slot.colors.clone();
            mask_hidden_zones(&slot.description, dark, &mut colors);
            if colors != slot.colors {
                slot.colors = colors;
                changed.push(id.clone());
            }
        }
        if self.is_closed() {
            return;
        }
        for id in changed {
            debug!(%id, "dark zones changed; re-sending the device");
            self.refresh_locked(&mut st, Some(&id));
        }
    }

    /// Forgets the setting of every present device that has no entry in `keep` (ADR 0001 #40). A device the engine
    /// drives is taken over anew, now without a setting: with the mode it shows on its own, else the default colour
    /// (see [`Slot::take_over`]), and re-sent. Excluded and faulted devices only lose the setting. Returns the driven
    /// devices none of these fit; the caller releases them.
    fn reset_unlisted(&self, keep: &BTreeMap<DeviceId, DeviceSetting>) -> Vec<DeviceId> {
        if self.is_closed() {
            return Vec::new();
        }
        let mut st = self.state.lock();
        let ids: Vec<DeviceId> = st
            .devices
            .keys()
            .filter(|id| !keep.contains_key(*id))
            .cloned()
            .collect();
        let mut release = Vec::new();
        for id in ids {
            let excluded = st.excluded.contains(&id);
            let retake = match st.devices.get_mut(&id) {
                Some(slot) => {
                    slot.setting = None;
                    let retake = !excluded && !slot.is_faulted() && slot.want.is_some();
                    if retake {
                        slot.want = None;
                    }
                    retake
                }
                None => false,
            };
            if !retake {
                continue;
            }
            st.take_over(&id);
            if st.devices.get(&id).is_some_and(|s| s.want.is_some()) {
                debug!(%id, "device not in the profile; taken over with its own mode or the default colour");
                self.refresh_locked(&mut st, Some(&id));
            } else {
                release.push(id);
            }
        }
        release
    }

    /// Releases every device that is not faulted; counts the ones that were driven before.
    fn release_all(&self) -> ReleaseReport {
        let ids: Vec<(DeviceId, bool)> = self
            .state
            .lock()
            .devices
            .iter()
            .filter(|(_, s)| !s.is_faulted())
            .map(|(id, s)| (id.clone(), s.want.is_some() || s.status == DeviceStatus::Active))
            .collect();
        let mut report = ReleaseReport::default();
        for (id, driven) in ids {
            match self.release(&id) {
                Ok(()) if driven => report.released += 1,
                Ok(()) => {}
                Err(e) => report.failed.push(ApplyFailure {
                    id,
                    error: e.to_string(),
                }),
            }
        }
        report
    }

    fn apply_one(&self, id: &DeviceId, setting: DeviceSetting) -> Result<(), EngineError> {
        let led_count = {
            let st = self.state.lock();
            st.devices
                .get(id)
                .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?
                .description
                .led_count()
        };
        match setting.fit_to(led_count) {
            DeviceSetting::Direct { colors } => self.set_direct(id, colors),
            DeviceSetting::Mode { state } => self.set_mode(id, state),
        }
    }

    /// Stops a removed slot's worker within `timeout`; with `release`, the worker releases the device first if it
    /// considers it Active (its own state, not the engine's copy). Joins the thread only if it
    /// answered; a thread stuck in a driver call is left behind rather than blocking the caller.
    fn stop_slot(
        &self,
        id: &DeviceId,
        mut slot: Slot,
        release: bool,
        timeout: Duration,
    ) -> Result<(), EngineError> {
        if slot.thread.is_none() {
            // A faulted device whose worker already stopped (after releasing it) before a re-detection.
            return Ok(());
        }
        let r = self.acked_within(
            id,
            slot.tx.clone(),
            |reply| Command::Stop {
                release,
                reply: Some(reply),
            },
            timeout,
        );
        match &r {
            Ok(()) | Err(EngineError::Device(_) | EngineError::UnknownDevice(_)) => {
                if let Some(t) = slot.thread.take()
                    && t.join().is_err()
                {
                    warn!(%id, "device thread panicked");
                }
            }
            Err(e) => warn!(%id, error = %e, "device thread did not stop in time; leaving it behind"),
        }
        r
    }

    fn set_disabled_families(&self, families: HashSet<String>) {
        let removed: Vec<(DeviceId, Slot)> = {
            let mut st = self.state.lock();
            st.disabled_families = families.clone();
            let ids: Vec<DeviceId> = st
                .devices
                .iter()
                .filter(|(_, s)| families.contains(&s.family))
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| st.devices.remove(&id).map(|s| (id, s)))
                .collect()
        };
        for (id, slot) in removed {
            info!(%id, "family disabled; releasing device");
            if let Err(e) = self.stop_slot(&id, slot, true, self.timing.ack_timeout) {
                warn!(%id, error = %e, "release of disabled device failed");
            }
            self.emit(EngineEvent::DeviceRemoved(id));
        }
    }

    /// Starts a reassert unless one started within the debounce time; returns whether it started. The work runs on
    /// its own thread (`cc-resume`), so this returns at once.
    pub(crate) fn reassert(this: &Arc<Self>) -> bool {
        if this.is_closed() {
            return false;
        }
        {
            let mut last = this.last_reassert.lock();
            let now = Instant::now();
            if last.is_some_and(|t| now.duration_since(t) < this.timing.resume_debounce) {
                debug!("reassert already running; ignoring the repeated resume");
                return false;
            }
            *last = Some(now);
        }
        // What was there before the sleep is what should be there again: the devices present now and the driven ones
        // removed just before the resume, which also get what they were driven with when they come back.
        let expected: Vec<DeviceId> = {
            let mut st = this.state.lock();
            let recent: Vec<(DeviceId, DeviceSetting)> = st
                .removed
                .iter()
                .filter(|(_, (t, _))| t.elapsed() < RESUME_LOOKBACK)
                .map(|(id, (_, setting))| (id.clone(), setting.clone()))
                .collect();
            let mut expected: Vec<DeviceId> = st.devices.keys().cloned().collect();
            for (id, setting) in recent {
                st.pending.entry(id.clone()).or_insert(setting);
                expected.push(id);
            }
            expected
        };
        info!(devices = expected.len(), "resumed; re-asserting device state");
        let shared = Arc::clone(this);
        let spawned = std::thread::Builder::new()
            .name("cc-resume".into())
            .spawn(move || shared.run_reassert(&expected));
        if let Err(e) = spawned {
            warn!(error = %e, "could not start the resume thread");
            return false;
        }
        true
    }

    /// The reassert: waits for USB to settle, resets and re-sends every driven device, re-opens faulted and missing
    /// ones, and keeps re-opening (with growing pauses) until every device in `expected` is back and confirmed or
    /// the resume window is over.
    fn run_reassert(self: Arc<Self>, expected: &[DeviceId]) {
        let start = Instant::now();
        if !self.sleep_unless_closed(self.timing.resume_settle) {
            return;
        }
        self.reassert_driven();
        Self::detect_new(&self, None);
        let deadline = start + self.timing.resume_window;
        let mut pause = self.timing.resume_poll;
        loop {
            let (missing, waiting) = self.resume_progress(expected);
            if missing.is_empty() && waiting == 0 {
                info!(elapsed = ?start.elapsed(), "device state re-asserted after resume");
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                warn!(?missing, waiting, "some devices did not come back after resume");
                return;
            }
            if !self.sleep_unless_closed(pause.min(left)) {
                return;
            }
            pause = (pause * 2).min(Duration::from_secs(1));
            if !missing.is_empty() {
                debug!(?missing, "re-opening devices after resume");
                Self::detect_new(&self, None);
            }
        }
    }

    /// Whether a reassert started within the resume window.
    fn resume_active(&self) -> bool {
        self.last_reassert
            .lock()
            .is_some_and(|t| t.elapsed() < self.timing.resume_window)
    }

    /// Sleeps `d`; returns `false` if the engine is shutting down.
    fn sleep_unless_closed(&self, d: Duration) -> bool {
        std::thread::sleep(d);
        !self.is_closed()
    }

    /// Hands every driven, usable device a reassert of its current output.
    fn reassert_driven(&self) {
        let mut st = self.state.lock();
        let ids: Vec<DeviceId> = st
            .devices
            .iter()
            .filter(|(_, s)| s.want.is_some() && !matches!(s.status, DeviceStatus::Faulted { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let levels = st.levels(&id);
            let seq = self.next_seq();
            let Some(slot) = st.devices.get_mut(&id) else {
                continue;
            };
            let Some(output) = slot.output(levels) else {
                continue;
            };
            match slot.tx.try_send(Command::Reassert { seq, output }) {
                Ok(()) => slot.resuming = true,
                Err(e) => {
                    warn!(%id, "could not queue the reassert; sending the output without a reset");
                    if let Command::Reassert { seq, output } = e.into_inner() {
                        slot.push_output(seq, output);
                    }
                }
            }
        }
    }

    /// Devices of `expected` that are missing or faulted, and how many devices still wait for their reassert.
    fn resume_progress(&self, expected: &[DeviceId]) -> (Vec<DeviceId>, usize) {
        let st = self.state.lock();
        let missing = expected
            .iter()
            .filter(|id| {
                st.devices
                    .get(*id)
                    .is_none_or(|s| matches!(s.status, DeviceStatus::Faulted { .. }))
            })
            .cloned()
            .collect();
        let waiting = st.devices.values().filter(|s| s.resuming).count();
        (missing, waiting)
    }

    fn shutdown(&self, timeout: Duration) {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let start = Instant::now();
        let deadline = start + timeout;
        // From here on API calls fail, no events go out and an in-flight detection adds nothing.
        self.closed.store(true, Ordering::Release);
        let resume = self.resume.lock().take();
        if let Some(r) = resume {
            r.stop();
        }
        let hotplug = self.hotplug.lock().take();
        if let Some(h) = hotplug {
            h.stop_within(deadline.saturating_duration_since(Instant::now()));
        }
        // Wait for an in-flight detection pass to finish, but only within the budget: a backend stuck in `detect`
        // must not block quitting. Detection checks `closed` before adding anything.
        let guard = self.detect_lock.try_lock_until(deadline);
        if guard.is_none() {
            warn!("detection still running at shutdown; not waiting for it");
        }
        let slots: Vec<(DeviceId, Slot)> = std::mem::take(&mut self.state.lock().devices)
            .into_iter()
            .collect();
        let n = slots.len();
        for (i, (id, slot)) in slots.into_iter().enumerate() {
            let left = u32::try_from(n - i).unwrap_or(u32::MAX);
            let budget = deadline.saturating_duration_since(Instant::now()) / left;
            if let Err(e) = self.stop_slot(&id, slot, true, budget) {
                warn!(%id, error = %e, "release on shutdown failed");
            }
        }
        drop(guard);
        self.subscribers.lock().clear();
        info!(devices = n, elapsed = ?start.elapsed(), "engine shut down");
    }
}

/// Vendor and product id of a HID device id (`"hid:<vid>:<pid>..."`).
fn hid_vid_pid(id: &DeviceId) -> Option<(u16, u16)> {
    let mut parts = id.as_str().split(':');
    if parts.next()? != "hid" {
        return None;
    }
    let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
    Some((vid, pid))
}

fn check_has_direct(desc: &DeviceDescription) -> Result<(), EngineError> {
    if desc.direct_mode.is_none() {
        return Err(EngineError::InvalidArgument("device has no direct mode".into()));
    }
    Ok(())
}

/// Carries a colour buffer over a zone resize: each zone keeps its leading colours, new LEDs repeat the zone's last
/// colour (black if it had none).
fn remap_buffer(old: &DeviceDescription, new: &DeviceDescription, colors: &[Rgb]) -> Vec<Rgb> {
    let mut out = Vec::with_capacity(new.led_count());
    for (i, zone) in new.zones.iter().enumerate() {
        let prev: Vec<Rgb> = old
            .zones
            .get(i)
            .and_then(|z| colors.get(z.range()))
            .map(<[Rgb]>::to_vec)
            .unwrap_or_default();
        let fill = prev.last().copied().unwrap_or(Rgb::BLACK);
        out.extend(prev.iter().copied().take(zone.len));
        out.extend(std::iter::repeat_n(fill, zone.len.saturating_sub(prev.len())));
    }
    out.resize(new.led_count(), Rgb::BLACK);
    out
}

/// Keeps the engine alive; dropping the last one shuts it down.
struct Guard {
    shared: Arc<Shared>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.shared.shutdown(Duration::from_secs(2));
    }
}

/// Handle to a running engine. Cheap to clone; `Send + Sync`. Dropping the last clone shuts the engine down with a
/// 2 s budget unless [`shutdown`](EngineHandle::shutdown) was called.
#[derive(Clone)]
pub struct EngineHandle {
    guard: Arc<Guard>,
}

impl fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let devices = self.shared().state.lock().devices.len();
        f.debug_struct("EngineHandle")
            .field("devices", &devices)
            .field("closed", &self.shared().is_closed())
            .finish()
    }
}

/// Starts the engine: runs detection synchronously, starts one thread per device and, if `cfg.hotplug`, watches HID
/// arrivals and removals.
pub fn start(cfg: EngineConfig, backends: Vec<Box<dyn Backend>>) -> Result<EngineHandle, EngineError> {
    let source: Option<Box<dyn HotplugSource>> = if cfg.hotplug {
        Some(Box::new(crate::hotplug::HidHotplugSource))
    } else {
        None
    };
    let resume = cfg.resume;
    let handle = start_with(cfg, backends, source, Timing::default())?;
    if resume {
        let rt = ResumeRuntime::start(handle.shared());
        *handle.shared().resume.lock() = rt;
    }
    Ok(handle)
}

/// [`start`] with an explicit hot-plug source and timing (tests inject both). `cfg.hotplug` and `cfg.resume` are
/// ignored; tests call [`EngineHandle::reassert`] directly.
pub(crate) fn start_with(
    cfg: EngineConfig,
    backends: Vec<Box<dyn Backend>>,
    hotplug: Option<Box<dyn HotplugSource>>,
    timing: Timing,
) -> Result<EngineHandle, EngineError> {
    if !FPS_RANGE.contains(&cfg.frame_rate) {
        return Err(EngineError::InvalidArgument(format!(
            "frame rate {} outside 1..=60",
            cfg.frame_rate
        )));
    }
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            devices: BTreeMap::new(),
            failures: Vec::new(),
            pending: BTreeMap::new(),
            removed: BTreeMap::new(),
            disabled_families: cfg.disabled_families,
            zone_sizes: cfg.zone_sizes,
            master: cfg.master_brightness.min(FULL),
            brightness: normalized_brightness(cfg.brightness),
            excluded: cfg.excluded,
            default_color: cfg.default_color,
            dark_zones: cfg.dark_zones,
        }),
        subscribers: Mutex::new(Vec::new()),
        backends,
        frame_interval_ns: AtomicU64::new(interval_ns(cfg.frame_rate)),
        closed: AtomicBool::new(false),
        shutdown_started: AtomicBool::new(false),
        next_seq: AtomicU64::new(0),
        detect_lock: Mutex::new(()),
        hotplug: Mutex::new(None),
        resume: Mutex::new(None),
        last_reassert: Mutex::new(None),
        hardware_state: Mutex::new(cfg.hardware_state),
        next_generation: AtomicU64::new(0),
        timing,
    });
    Shared::detect_new(&shared, None);
    if let Some(source) = hotplug {
        let rt = HotplugRuntime::start(source, &shared);
        *shared.hotplug.lock() = rt;
    }
    info!(devices = shared.state.lock().devices.len(), "engine started");
    Ok(EngineHandle {
        guard: Arc::new(Guard { shared }),
    })
}

impl EngineHandle {
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.guard.shared
    }

    /// Every controlled device, sorted by kind, then name (then id).
    pub fn devices(&self) -> Vec<DeviceView> {
        let st = self.shared().state.lock();
        let mut out: Vec<DeviceView> = st.devices.iter().map(|(id, s)| st.view(id, s)).collect();
        out.sort_by(|a, b| {
            let (a, b) = (&a.description.info, &b.description.info);
            (a.kind as u32, &a.name, &a.id).cmp(&(b.kind as u32, &b.name, &b.id))
        });
        out
    }

    /// One device, if controlled.
    pub fn device(&self, id: &DeviceId) -> Option<DeviceView> {
        let st = self.shared().state.lock();
        st.devices.get(id).map(|s| st.view(id, s))
    }

    /// Devices backends found but could not bring up (most recent last, bounded).
    pub fn failures(&self) -> Vec<DetectFailure> {
        self.shared().state.lock().failures.clone()
    }

    /// Shows `colors` (one per LED) in direct mode. Coalesced: the device gets at most one frame per frame interval,
    /// always the latest. Re-takes control after a release.
    pub fn set_direct(&self, id: &DeviceId, colors: Vec<Rgb>) -> Result<(), EngineError> {
        self.shared().set_direct(id, colors)
    }

    /// Writes `colors` into zone `zone` of the device's buffer and shows the buffer; coalesced like
    /// [`set_direct`](Self::set_direct).
    pub fn set_zone(&self, id: &DeviceId, zone: usize, colors: Vec<Rgb>) -> Result<(), EngineError> {
        self.shared().set_zone(id, zone, colors)
    }

    /// Switches to a hardware mode; waits for the device (3 s timeout) and emits `DeviceStateChanged` with the
    /// state (a device that was not driven before also gets one for being taken).
    pub fn set_mode(&self, id: &DeviceId, state: ModeState) -> Result<(), EngineError> {
        self.shared().set_mode(id, state)
    }

    /// Changes a resizable zone's LED count; waits for the device. The new size is remembered for re-detection.
    pub fn resize_zone(&self, id: &DeviceId, zone: usize, len: usize) -> Result<(), EngineError> {
        self.shared().resize_zone(id, zone, len)
    }

    /// Persists the current mode in the device's flash; waits for the device. Only on explicit user request.
    pub fn save_to_hardware(&self, id: &DeviceId) -> Result<(), EngineError> {
        self.shared().save_to_hardware(id)
    }

    /// Hands the device back to its hardware mode; waits for the device.
    pub fn release(&self, id: &DeviceId) -> Result<(), EngineError> {
        self.shared().release(id)
    }

    /// Releases every device that is not faulted, in id order. The report counts the devices that were driven (or
    /// being taken over) before and are released now, and lists the releases that failed.
    pub fn release_all(&self) -> ReleaseReport {
        self.shared().release_all()
    }

    /// Applies a profile's settings. Each setting is fitted to the device's LED count; `Direct` goes out as a colour
    /// frame, `Mode` via [`set_mode`](Self::set_mode). Settings for absent devices replace the pending set and are
    /// applied when those devices appear.
    pub fn apply(&self, settings: &BTreeMap<DeviceId, DeviceSetting>) -> ApplyReport {
        let shared = self.shared();
        let mut report = ApplyReport::default();
        let mut pending = BTreeMap::new();
        for (id, setting) in settings {
            let present = shared.state.lock().devices.contains_key(id);
            if !present {
                pending.insert(id.clone(), setting.clone());
                report.missing.push(id.clone());
                continue;
            }
            match shared.apply_one(id, setting.clone()) {
                Ok(()) => report.applied.push(id.clone()),
                Err(e) => report.failed.push(ApplyFailure {
                    id: id.clone(),
                    error: e.to_string(),
                }),
            }
        }
        shared.state.lock().pending = pending;
        report
    }

    /// Applies a whole profile (ADR 0001 #40): like [`apply`](Self::apply), and every connected device without an
    /// entry in `settings` forgets its setting. Such a device that the engine drives is taken over anew with the mode
    /// it shows on its own (`DeviceView::hardware`), else the default colour (see
    /// [`set_default_color`](Self::set_default_color)), or released when neither fits it; excluded and faulted
    /// devices are left alone apart from the forgotten setting. So after the call the devices show the profile and,
    /// where it has nothing, their own lighting or the fallback, never a setting of an earlier profile.
    pub fn apply_replacing(&self, settings: &BTreeMap<DeviceId, DeviceSetting>) -> ApplyReport {
        for id in self.shared().reset_unlisted(settings) {
            if let Err(e) = self.shared().release(&id) {
                warn!(%id, error = %e, "device not in the profile could not be released");
            }
        }
        self.apply(settings)
    }

    /// The current setting of every device that has one, for saving as a profile: what the engine drives the device
    /// with (also when the frame has not gone out yet), else the setting it was last driven with.
    pub fn current_settings(&self) -> BTreeMap<DeviceId, DeviceSetting> {
        self.shared()
            .state
            .lock()
            .devices
            .iter()
            .filter_map(|(id, s)| {
                s.wanted_setting()
                    .or_else(|| s.setting.clone())
                    .map(|set| (id.clone(), set))
            })
            .collect()
    }

    /// Sets device `id`'s brightness in percent (clamped to 100) and sends the device at the new level (ADR 0001
    /// #33). Direct colours are scaled; in a hardware mode with a brightness parameter the mode's brightness is scaled
    /// inside its range; other hardware modes are not affected. The stored colours and mode stay unscaled. Coalesced
    /// like [`set_direct`](Self::set_direct).
    ///
    /// Takes over a device the engine does not drive (never set, or released) unless it is excluded: with its last
    /// setting, else with the mode it shows on its own (`DeviceView::hardware`), else with the default colour (see
    /// [`set_default_color`](Self::set_default_color)). A device none of these fit keeps its own lighting; the value
    /// is stored and applies once the device is driven.
    pub fn set_device_brightness(&self, id: &DeviceId, percent: u8) -> Result<(), EngineError> {
        self.shared().set_device_brightness(id, percent)
    }

    /// Replaces every device brightness (a device without an entry gets 100), as applying a profile does, and
    /// re-sends the driven devices. Values are clamped to 100. Call it before [`apply`](Self::apply) so the
    /// profile's colours go out at the profile's brightness right away.
    pub fn replace_brightness(&self, brightness: BTreeMap<DeviceId, u8>) {
        self.shared().replace_brightness(brightness);
    }

    /// Every device brightness below 100, also of devices that are not connected now (for saving as a profile).
    pub fn brightness(&self) -> BTreeMap<DeviceId, u8> {
        self.shared().state.lock().brightness.clone()
    }

    /// Sets the master brightness in percent (clamped to 100), multiplied into every device, and re-sends the driven
    /// devices. Undriven devices that are not excluded are taken over first, as by
    /// [`set_device_brightness`](Self::set_device_brightness).
    pub fn set_master_brightness(&self, percent: u8) {
        self.shared().set_master_brightness(percent);
    }

    /// Connect all, the counterpart of [`release_all`](Self::release_all): takes over every device the engine does
    /// not drive (never set, or released) that is neither excluded nor faulted, with its last setting, else the mode
    /// it shows on its own (`DeviceView::hardware`), else the default colour (see
    /// [`set_default_color`](Self::set_default_color)). Returns at once.
    pub fn connect_all(&self) {
        self.shared().connect_all();
    }

    /// Master brightness.
    pub fn lighting(&self) -> LightingView {
        let st = self.shared().state.lock();
        LightingView {
            master_brightness: st.master,
        }
    }

    /// Replaces the devices that brightness changes never take over (the app's hidden devices). Devices
    /// the engine already drives are not affected; release them separately.
    pub fn set_excluded(&self, excluded: HashSet<DeviceId>) {
        self.shared().set_excluded(excluded);
    }

    /// Sets the colour a takeover shows on a device that has neither a setting nor a hardware readback (ADR 0001
    /// #36): that colour on every LED in the device's direct mode, scaled by brightness as usual, or, without a
    /// direct mode, the device's first mode that takes a colour (per LED or mode-wide), set to it. Devices the
    /// engine already drives keep what they show.
    pub fn set_default_color(&self, color: Rgb) {
        self.shared().state.lock().default_color = color;
    }

    /// Replaces the zones (by name, per device) that the engine keeps black: the app's hidden zones (ADR 0001 #26).
    /// Every direct output has them black, whatever drives the device (colour edits, profiles, pending settings,
    /// takeovers); so do the per-LED colours of a default-colour takeover in a hardware mode. A device driven with
    /// direct colours is re-sent at once if the new zones black out something it shows; a zone that is no longer
    /// dark stays black until the next colour change. Other hardware modes act on the whole device and are left
    /// alone.
    pub fn set_dark_zones(&self, zones: BTreeMap<DeviceId, BTreeSet<String>>) {
        self.shared().set_dark_zones(zones);
    }

    /// Changes the frame rate; clamped to `1..=60`. Takes effect with the next frame.
    pub fn set_frame_rate(&self, fps: u32) {
        self.shared()
            .frame_interval_ns
            .store(interval_ns(fps), Ordering::Relaxed);
    }

    /// Replaces the set of disabled families. Devices of newly disabled families are released and dropped
    /// (`DeviceRemoved`); re-enabled families are picked up by the next [`rescan`](Self::rescan).
    pub fn set_disabled_families(&self, families: HashSet<String>) {
        self.shared().set_disabled_families(families);
    }

    /// Re-asserts what the engine drives after the system resumed from sleep or hibernation, when devices may show
    /// their firmware state again or have dropped off the bus. The engine calls it itself when
    /// [`EngineConfig::resume`] is on; it is public for other triggers.
    ///
    /// Returns at once; the work runs on its own thread. After a short settle time every driven device that is
    /// not faulted is reset (`release()`, which also makes the driver re-send its init) and sent its current output
    /// again (colours or mode with device and master brightness applied); released and undriven
    /// devices are left alone. Writes that fail are retried with back-off for a few seconds, after which the device
    /// is faulted. Devices that are faulted or missing are re-opened through detection, repeatedly during the
    /// resume window, and get back what they were driven with. Returns `false` (and does nothing) when a reassert
    /// started within the debounce time (5 s), since a wake usually reports itself twice, or after shutdown.
    pub fn reassert(&self) -> bool {
        Shared::reassert(self.shared())
    }

    /// Runs detection again on every backend; devices already under control are skipped and left alone.
    pub fn rescan(&self) {
        Shared::detect_new(self.shared(), None);
    }

    /// A read-only copy of the remembered machine state (own lighting and "unreleased" marker per device), as
    /// recorded so far.
    pub fn hardware_state(&self) -> HardwareSnapshot {
        self.shared().hardware_state.lock().snapshot()
    }

    /// A new receiver of every [`EngineEvent`] from now on. Receivers that are dropped are forgotten.
    pub fn subscribe(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = crossbeam_channel::unbounded();
        if !self.shared().is_closed() {
            self.shared().subscribers.lock().push(tx);
        }
        rx
    }

    /// Stops hot-plug, releases every Active device (each within its share of `timeout`), joins the device threads
    /// and closes the event stream. Idempotent; later calls return immediately and other methods fail with
    /// [`EngineError::ShutDown`].
    pub fn shutdown(&self, timeout: Duration) {
        self.shared().shutdown(timeout);
    }
}
