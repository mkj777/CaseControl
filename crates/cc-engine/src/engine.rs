//! Shared engine state, detection, and the [`EngineHandle`] API.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cc_config::DeviceSetting;
use cc_core::{
    Backend, Bus, Controller, DetectContext, Detected, DeviceDescription, DeviceError, DeviceId, EngineEvent,
    HotplugEvent, ModeState, Rgb,
};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::error::EngineError;
use crate::hotplug::{HotplugRuntime, HotplugSource};
use crate::view::{ApplyFailure, ApplyReport, DetectFailure, DeviceStatus, DeviceView};
use crate::worker::{Command, FrameSlot, Reply, Timing, Worker};

/// Capacity of each device's command channel. Colour frames do not use it (they go through the frame slot), so a
/// small bound suffices.
const COMMAND_CAPACITY: usize = 16;
/// Detection failures kept for diagnostics; older ones are dropped.
const MAX_FAILURES: usize = 64;
/// Bounds of the frame rate.
const FPS_RANGE: std::ops::RangeInclusive<u32> = 1..=60;

/// Engine configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Maximum colour frames per second and device, `1..=60`.
    pub frame_rate: u32,
    /// Family ids (see `cc_core::family`) whose detectors must not run.
    pub disabled_families: HashSet<String>,
    /// Zone lengths by zone name, applied right after a device is detected.
    pub zone_sizes: BTreeMap<DeviceId, BTreeMap<String, usize>>,
    /// Whether to watch for HID arrivals and removals.
    pub hotplug: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            frame_rate: 30,
            disabled_families: HashSet::new(),
            zone_sizes: BTreeMap::new(),
            hotplug: true,
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
    frame: FrameSlot,
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Slot {
    fn view(&self) -> DeviceView {
        DeviceView {
            description: self.description.clone(),
            status: self.status.clone(),
            setting: self.setting.clone(),
            colors: self.colors.clone(),
            family: self.family.clone(),
        }
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

    /// Stores `colors` as the device's buffer and hands it to the worker as the latest frame (sequence `seq`). The
    /// setting is recorded by the worker once the frame has actually been sent.
    fn push_frame(&mut self, seq: u64, colors: Vec<Rgb>) {
        *self.frame.lock() = Some((seq, colors.clone()));
        self.colors = colors;
        // Full channel: the worker is busy and will pick the frame up from the slot anyway.
        let _ = self.tx.try_send(Command::Wake);
    }
}

struct State {
    devices: BTreeMap<DeviceId, Slot>,
    failures: Vec<DetectFailure>,
    pending: BTreeMap<DeviceId, DeviceSetting>,
    disabled_families: HashSet<String>,
    zone_sizes: BTreeMap<DeviceId, BTreeMap<String, usize>>,
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

    /// Called by a worker when its device's status changes. Ignored if the slot was replaced or removed meanwhile.
    pub(crate) fn set_status(&self, id: &DeviceId, generation: u64, status: DeviceStatus) {
        let mut st = self.state.lock();
        if let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation)
            && !matches!(slot.status, DeviceStatus::Faulted { .. })
        {
            slot.status = status;
        }
    }

    /// Called by a worker after a colour frame went out: the device now shows `colors`.
    pub(crate) fn direct_applied(&self, id: &DeviceId, generation: u64, colors: Vec<Rgb>) {
        let mut st = self.state.lock();
        if let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) {
            slot.setting = Some(DeviceSetting::Direct { colors });
        }
    }

    /// Called by a worker after a mode was applied: records it and emits `DeviceStateChanged`.
    pub(crate) fn mode_applied(&self, id: &DeviceId, generation: u64, state: ModeState) {
        {
            let mut st = self.state.lock();
            let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) else {
                return;
            };
            slot.setting = Some(DeviceSetting::Mode { state: state.clone() });
        }
        self.emit(EngineEvent::DeviceStateChanged {
            id: id.clone(),
            state,
        });
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
        dropped: bool,
    ) {
        let mut st = self.state.lock();
        let Some(slot) = st.devices.get_mut(id).filter(|s| s.generation == generation) else {
            return;
        };
        let colors = remap_buffer(&slot.description, &desc, &slot.colors);
        let direct = matches!(slot.setting, Some(DeviceSetting::Direct { .. }));
        if let Some(DeviceSetting::Direct { colors: c }) = &mut slot.setting {
            *c = remap_buffer(&slot.description, &desc, c);
        }
        let name = desc.zones.get(zone).map(|z| (z.name.clone(), z.len));
        slot.description = desc;
        slot.colors = colors.clone();
        if direct || dropped {
            let seq = self.next_seq();
            slot.push_frame(seq, colors);
        }
        if let Some((name, len)) = name {
            st.zone_sizes.entry(id.clone()).or_default().insert(name, len);
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
        let ctx = {
            let st = this.state.lock();
            DetectContext {
                disabled_families: st.disabled_families.clone(),
                // Faulted devices are retried: a fresh controller replaces the faulted one.
                skip: st
                    .devices
                    .iter()
                    .filter(|(_, s)| !matches!(s.status, DeviceStatus::Faulted { .. }))
                    .map(|(id, _)| id.clone())
                    .collect(),
            }
        };
        let mut found = Vec::new();
        let mut failures = Vec::new();
        for backend in &this.backends {
            if this.is_closed() {
                return;
            }
            if bus.is_some_and(|b| backend.bus() != b) || !ctx.is_enabled(backend.name()) {
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
            colors: vec![Rgb::BLACK; description.led_count()],
            description,
            status: DeviceStatus::Active,
            setting: None,
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
            (st.pending.remove(&id), replaced)
        };
        if let Some(old) = replaced {
            info!(%id, "faulted device detected again; replacing it");
            // The faulted worker no longer talks to the device; let it end on its own.
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
            .get_mut(id)
            .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
        match setting.fit_to(slot.description.led_count()) {
            DeviceSetting::Direct { colors } => {
                check_has_direct(&slot.description)?;
                let seq = self.next_seq();
                slot.push_frame(seq, colors);
            }
            DeviceSetting::Mode { state } => {
                slot.description
                    .validate_state(&state)
                    .map_err(|e| EngineError::InvalidArgument(e.to_string()))?;
                let (reply, _) = crossbeam_channel::bounded(1);
                let seq = self.next_seq();
                slot.tx
                    .try_send(Command::SetMode { seq, state, reply })
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
        let Some(slot) = self.state.lock().devices.remove(id) else {
            return;
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
        let seq = self.next_seq();
        slot.push_frame(seq, colors);
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
        let seq = self.next_seq();
        slot.push_frame(seq, buffer);
        Ok(())
    }

    fn set_mode(&self, id: &DeviceId, state: ModeState) -> Result<(), EngineError> {
        let tx = self.slot_tx(id)?;
        {
            let st = self.state.lock();
            let slot = st
                .devices
                .get(id)
                .ok_or_else(|| EngineError::UnknownDevice(id.clone()))?;
            slot.description
                .validate_state(&state)
                .map_err(|e| EngineError::InvalidArgument(e.to_string()))?;
        }
        // The worker records the setting and emits DeviceStateChanged when (and if) it applies the mode.
        let seq = self.next_seq();
        self.acked(id, tx, move |reply| Command::SetMode { seq, state, reply })
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
        let seq = self.next_seq();
        self.acked(id, tx, move |reply| Command::Release { seq, reply })
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

    fn shutdown(&self, timeout: Duration) {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let start = Instant::now();
        let deadline = start + timeout;
        // From here on API calls fail, no events go out and an in-flight detection adds nothing.
        self.closed.store(true, Ordering::Release);
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
    start_with(cfg, backends, source, Timing::default())
}

/// [`start`] with an explicit hot-plug source and timing (tests inject both). `cfg.hotplug` is ignored.
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
            disabled_families: cfg.disabled_families,
            zone_sizes: cfg.zone_sizes,
        }),
        subscribers: Mutex::new(Vec::new()),
        backends,
        frame_interval_ns: AtomicU64::new(interval_ns(cfg.frame_rate)),
        closed: AtomicBool::new(false),
        shutdown_started: AtomicBool::new(false),
        next_seq: AtomicU64::new(0),
        detect_lock: Mutex::new(()),
        hotplug: Mutex::new(None),
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
    fn shared(&self) -> &Arc<Shared> {
        &self.guard.shared
    }

    /// Every controlled device, sorted by kind, then name (then id).
    pub fn devices(&self) -> Vec<DeviceView> {
        let st = self.shared().state.lock();
        let mut out: Vec<DeviceView> = st.devices.values().map(Slot::view).collect();
        out.sort_by(|a, b| {
            let (a, b) = (&a.description.info, &b.description.info);
            (a.kind as u32, &a.name, &a.id).cmp(&(b.kind as u32, &b.name, &b.id))
        });
        out
    }

    /// One device, if controlled.
    pub fn device(&self, id: &DeviceId) -> Option<DeviceView> {
        self.shared().state.lock().devices.get(id).map(Slot::view)
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

    /// Switches to a hardware mode; waits for the device (3 s timeout) and emits `DeviceStateChanged`.
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

    /// Releases every device that is not faulted, in id order.
    pub fn release_all(&self) -> Vec<(DeviceId, Result<(), EngineError>)> {
        let ids: Vec<DeviceId> = self
            .shared()
            .state
            .lock()
            .devices
            .iter()
            .filter(|(_, s)| !matches!(s.status, DeviceStatus::Faulted { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .map(|id| {
                let r = self.release(&id);
                (id, r)
            })
            .collect()
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

    /// The current setting of every device that has one, for saving as a profile.
    pub fn current_settings(&self) -> BTreeMap<DeviceId, DeviceSetting> {
        self.shared()
            .state
            .lock()
            .devices
            .iter()
            .filter_map(|(id, s)| s.setting.clone().map(|set| (id.clone(), set)))
            .collect()
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

    /// Runs detection again on every backend; devices already under control are skipped and left alone.
    pub fn rescan(&self) {
        Shared::detect_new(self.shared(), None);
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
