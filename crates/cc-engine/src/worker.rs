//! The per-device worker thread: owns one [`Controller`], paces colour frames, sends keepalives and applies the error
//! policy (exponential back-off, `Faulted` after too many consecutive failures or on a fatal error).
//!
//! Ordering: every colour frame, mode change, release and resize carries a sequence number taken when the caller
//! issued it. The worker applies them in sequence order and skips anything older than what it last applied, so the
//! device always ends up showing the most recently issued request. The worker, not the caller, records what was
//! actually applied (setting, `DeviceStateChanged`, resized description), so the engine's record matches the device.
//!
//! Brightness (ADR 0001 #33): what the worker sends ([`Out`]) can differ from the unscaled setting it records
//! ([`Output::stored`]). The latest-wins slot carries brightness steps for hardware modes too, paced like colour
//! frames; a mode output equal to the one the device already shows is not sent again, so a mode without a brightness
//! parameter does not restart its effect on every brightness step.
//!
//! Status: a device starts released (it shows its own lighting) and becomes active with the first output that goes
//! out; an explicit release makes it released again. The worker reports both changes to the engine, which emits
//! `DeviceStateChanged`.
//!
//! "Unreleased" marker (ADR 0001 #38): before the first write after it was opened or last released, the worker sets
//! the device's persisted marker (CaseControl wrote to it and did not hand it back); after a successful release it
//! clears it again, unless the driver says the release hands back a state CaseControl left
//! (`Controller::shows_own_lighting`). A device still marked at the next open was left in CaseControl's state.
//!
//! Resume (after sleep or hibernate): [`Command::Reassert`] makes the worker hand the device back once (`release()`,
//! which also clears the driver's "already sent" caches and makes it re-send its init) and then send the current
//! output again. Reset and send are retried together with the usual back-off; if the device still does not take
//! them when [`Timing::resume_retry`] has passed, the worker faults it so the engine re-opens it through detection.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cc_core::{Controller, DeviceError, DeviceId, ModeState};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::engine::Shared;
use crate::lighting::{Out, Output};
use crate::view::DeviceStatus;

/// An output (colour frame or brightness step of a mode) and the sequence number it was issued with.
pub(crate) type Frame = (u64, Output);

/// Where the latest coalesced output waits for the worker. Latest wins: a newer output replaces an unsent one.
pub(crate) type FrameSlot = Arc<Mutex<Option<Frame>>>;

/// Reply channel of an acknowledged command.
pub(crate) type Reply = Sender<Result<(), DeviceError>>;

/// Messages to a worker thread.
pub(crate) enum Command {
    /// A new frame is in the [`FrameSlot`].
    Wake,
    /// Switch to a hardware mode: send `out` (the mode at the current brightness, or the off output at brightness 0),
    /// record `state`.
    SetMode {
        seq: u64,
        state: ModeState,
        out: Out,
        reply: Reply,
    },
    /// Resize a zone.
    Resize {
        seq: u64,
        zone: usize,
        len: usize,
        reply: Reply,
    },
    /// Persist the current mode to flash.
    Save(Reply),
    /// Hand the device back to its hardware mode.
    Release { seq: u64, reply: Reply },
    /// Leave the thread, releasing first if `release` and the device was not handed back since it was opened or last
    /// written to. A faulted device is released too (best effort; ADR 0001 #38).
    Stop { release: bool, reply: Option<Reply> },
    /// The system resumed: reset the device (`release()`) and send `output` (the current output, issued with
    /// `seq`) again, retrying both with back-off for up to [`Timing::resume_retry`].
    Reassert { seq: u64, output: Output },
}

/// Timing knobs of the error policy and acknowledgements. Tests shorten them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// How long an acked command waits for the worker.
    pub ack_timeout: Duration,
    /// First back-off after a write error.
    pub backoff_base: Duration,
    /// Back-off cap.
    pub backoff_max: Duration,
    /// Consecutive failures that fault a device.
    pub max_failures: u32,
    /// How often the hot-plug thread checks for shutdown.
    pub hotplug_poll: Duration,
    /// Resume notifications closer together than this start only one reassert (a wake usually sends two).
    pub resume_debounce: Duration,
    /// Wait after a resume before the first write, so USB devices can come back.
    pub resume_settle: Duration,
    /// How long a worker keeps retrying the reset and re-send after a resume before it faults the device.
    pub resume_retry: Duration,
    /// How long after a resume the engine keeps re-opening devices that are faulted or missing.
    pub resume_window: Duration,
    /// First pause between those checks; it doubles up to one second.
    pub resume_poll: Duration,
    /// How long detection waits for the workers of faulted devices to release them before opening them again; a
    /// device whose worker has not ended by then is not opened in that pass.
    pub faulted_release: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ack_timeout: Duration::from_secs(3),
            backoff_base: Duration::from_millis(50),
            backoff_max: Duration::from_secs(5),
            max_failures: 10,
            hotplug_poll: Duration::from_millis(100),
            resume_debounce: Duration::from_secs(5),
            resume_settle: Duration::from_secs(1),
            resume_retry: Duration::from_secs(5),
            resume_window: Duration::from_secs(20),
            resume_poll: Duration::from_millis(250),
            faulted_release: Duration::from_millis(500),
        }
    }
}

impl Timing {
    /// Back-off after the `n`-th consecutive failure (`n >= 1`): base doubling per failure, capped.
    pub(crate) fn backoff(&self, n: u32) -> Duration {
        let shift = n.saturating_sub(1).min(16);
        self.backoff_base
            .saturating_mul(1u32 << shift)
            .min(self.backoff_max)
    }
}

/// The worker's own view of its device. A device starts `Released`: it shows its own lighting until the first
/// output goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Local {
    Active,
    Released,
    Faulted,
}

/// State owned by one worker thread.
pub(crate) struct Worker {
    id: DeviceId,
    generation: u64,
    ctrl: Box<dyn Controller>,
    rx: Receiver<Command>,
    frame: FrameSlot,
    shared: Arc<Shared>,
    local: Local,
    fault_reason: String,
    dirty: Option<Frame>,
    /// The mode the device was last sent and still shows (cleared by a colour frame or a release).
    shown_mode: Option<ModeState>,
    /// The device was not handed back since it was opened or last written to (also a write that failed, which may
    /// have taken effect), so stopping releases it. True from the start, as before the status became truthful:
    /// drivers make a release of an untouched device a no-op, except where the reference resets on purpose
    /// (Wooting cleans up after a crashed earlier run).
    needs_release: bool,
    /// The persisted "unreleased" marker was set by this worker and not cleared since (see the module docs).
    marked: bool,
    /// Sequence number of the last frame, mode or release that was applied (or superseded everything before it).
    last_seq: u64,
    last_frame: Option<Instant>,
    failures: u32,
    retry_at: Option<Instant>,
    next_keepalive: Option<Instant>,
    /// A resume asked for a reset (`release()`) before the next output.
    reset: bool,
    /// After a resume: until when failed writes are retried before the device is faulted. Cleared by the first
    /// successful output.
    resume_until: Option<Instant>,
}

impl Worker {
    pub(crate) fn new(
        id: DeviceId,
        generation: u64,
        ctrl: Box<dyn Controller>,
        rx: Receiver<Command>,
        frame: FrameSlot,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
            id,
            generation,
            ctrl,
            rx,
            frame,
            shared,
            local: Local::Released,
            fault_reason: String::new(),
            dirty: None,
            shown_mode: None,
            needs_release: true,
            marked: false,
            last_seq: 0,
            last_frame: None,
            failures: 0,
            retry_at: None,
            next_keepalive: None,
            reset: false,
            resume_until: None,
        }
    }

    /// The thread body. Returns when told to stop or when the engine dropped the command channel.
    pub(crate) fn run(mut self) {
        debug!(id = %self.id, "device thread started");
        'outer: loop {
            // Queued commands first, so a command issued before a frame is not overtaken by it.
            loop {
                match self.rx.try_recv() {
                    Ok(cmd) => {
                        if !self.handle(cmd) {
                            break 'outer;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'outer,
                }
            }
            self.take_frame();
            self.do_due_work(Instant::now());
            let msg = match self.next_deadline() {
                Some(deadline) => self.rx.recv_deadline(deadline),
                None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(cmd) => {
                    if !self.handle(cmd) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        debug!(id = %self.id, "device thread stopped");
    }

    fn take_frame(&mut self) {
        if let Some(f) = self.frame.lock().take()
            && self.dirty.as_ref().is_none_or(|(seq, _)| f.0 > *seq)
        {
            self.dirty = Some(f);
        }
    }

    /// Drops pending frames issued before `seq`; returns whether one was dropped.
    fn drop_frames_before(&mut self, seq: u64) -> bool {
        let mut dropped = false;
        if self.dirty.as_ref().is_some_and(|(s, _)| *s < seq) {
            self.dirty = None;
            dropped = true;
        }
        let mut slot = self.frame.lock();
        if slot.as_ref().is_some_and(|(s, _)| *s < seq) {
            *slot = None;
            dropped = true;
        }
        dropped
    }

    fn not_before(&self) -> Option<Instant> {
        self.retry_at
    }

    fn frame_due(&self) -> Option<Instant> {
        self.dirty.as_ref()?;
        let paced = self.last_frame.map(|t| t + self.shared.frame_interval());
        max_opt(paced, self.not_before()).or(Some(Instant::now()))
    }

    fn keepalive_due(&self) -> Option<Instant> {
        if self.local != Local::Active {
            return None;
        }
        max_opt(
            self.next_keepalive,
            self.not_before().filter(|_| self.next_keepalive.is_some()),
        )
    }

    fn next_deadline(&self) -> Option<Instant> {
        if self.local == Local::Faulted {
            return None;
        }
        min_opt(self.frame_due(), self.keepalive_due())
    }

    fn do_due_work(&mut self, now: Instant) {
        if self.local == Local::Faulted {
            self.dirty = None;
            return;
        }
        if self.frame_due().is_some_and(|due| now >= due)
            && let Some(frame) = self.dirty.take()
        {
            self.send_frame(frame, now);
        }
        if self.local == Local::Active
            && self.keepalive_due().is_some_and(|due| now >= due)
            && let Some(period) = self.ctrl.description().keepalive
        {
            let r = self.ctrl.keepalive();
            self.next_keepalive = Some(now + period);
            if let Err(e) = &r {
                warn!(id = %self.id, error = %e, "keepalive failed");
            }
            self.record(&r);
        }
    }

    fn send_frame(&mut self, (seq, output): Frame, now: Instant) {
        if seq < self.last_seq {
            debug!(id = %self.id, "dropping superseded frame");
            return;
        }
        if let Out::Direct(colors) = &output.out
            && colors.len() != self.ctrl.description().led_count()
        {
            debug!(id = %self.id, "dropping frame with stale length");
            return;
        }
        let unchanged = self.local == Local::Active
            && !self.reset
            && matches!(&output.out, Out::Mode(m) if self.shown_mode.as_ref() == Some(m));
        let r = if unchanged {
            Ok(())
        } else {
            self.last_frame = Some(now);
            self.send_out_reset(&output.out)
        };
        match &r {
            Ok(()) => {
                self.last_seq = seq;
                self.took_control(now);
                self.resumed();
                self.shared
                    .applied(&self.id, self.generation, output.stored, false);
            }
            Err(DeviceError::InvalidArgument(msg)) => {
                warn!(id = %self.id, %msg, "driver rejected frame");
            }
            Err(e) if !e.is_fatal() => {
                warn!(id = %self.id, error = %e, "frame write failed");
                // Retry after the back-off, with whatever frame is newest by then.
                if self.dirty.is_none() {
                    self.dirty = Some((seq, output));
                }
            }
            Err(_) => {}
        }
        self.record(&r);
    }

    /// [`send_out`](Self::send_out), preceded by the reset a resume asked for (once it succeeded, not again).
    fn send_out_reset(&mut self, out: &Out) -> Result<(), DeviceError> {
        if self.reset {
            self.ctrl.release()?;
            self.reset = false;
            self.shown_mode = None;
            debug!(id = %self.id, "device reset after resume");
        }
        self.send_out(out)
    }

    /// An output went out after a resume: the device is back.
    fn resumed(&mut self) {
        if self.resume_until.take().is_some() {
            debug!(id = %self.id, "device re-asserted after resume");
            self.shared.reasserted(&self.id, self.generation);
        }
    }

    /// Sets the persisted "unreleased" marker before a write, once until the next release.
    fn mark_written(&mut self) {
        if !self.marked {
            self.shared.set_unreleased(&self.id, true);
            self.marked = true;
        }
    }

    /// A release succeeded: clears the "unreleased" marker unless the driver handed back a state CaseControl left.
    fn released(&mut self) {
        self.needs_release = false;
        if self.ctrl.shows_own_lighting() {
            self.shared.set_unreleased(&self.id, false);
            self.marked = false;
        }
    }

    /// Sends one output to the device and remembers which mode it shows.
    fn send_out(&mut self, out: &Out) -> Result<(), DeviceError> {
        self.needs_release = true;
        self.mark_written();
        let r = match out {
            Out::Direct(colors) => self.ctrl.set_direct(colors),
            Out::Mode(state) => self.ctrl.set_mode(state),
        };
        if r.is_ok() {
            self.shown_mode = match out {
                Out::Direct(_) => None,
                Out::Mode(state) => Some(state.clone()),
            };
        }
        r
    }

    /// A write succeeded: the device is (again) under software control.
    fn took_control(&mut self, now: Instant) {
        if self.local != Local::Active {
            self.local = Local::Active;
            self.shared
                .set_status(&self.id, self.generation, DeviceStatus::Active);
        }
        if self.next_keepalive.is_none()
            && let Some(period) = self.ctrl.description().keepalive
        {
            self.next_keepalive = Some(now + period);
        }
    }

    /// Applies the error policy to the result of a device write.
    fn record(&mut self, r: &Result<(), DeviceError>) {
        match r {
            Ok(()) => {
                self.failures = 0;
                self.retry_at = None;
            }
            Err(e) if e.is_fatal() => self.fault(e.to_string()),
            Err(DeviceError::InvalidArgument(_) | DeviceError::Unsupported) => {}
            Err(e) => {
                self.failures += 1;
                let timing = self.shared.timing;
                if self.resume_until.is_some_and(|until| Instant::now() >= until) {
                    self.fault(format!("not reachable after resume, last error: {e}"));
                } else if self.failures >= timing.max_failures {
                    self.fault(format!("{} consecutive failures, last: {e}", self.failures));
                } else {
                    let wait = timing.backoff(self.failures);
                    debug!(id = %self.id, failures = self.failures, ?wait, "backing off");
                    self.retry_at = Some(Instant::now() + wait);
                }
            }
        }
    }

    fn fault(&mut self, reason: String) {
        warn!(id = %self.id, %reason, "device faulted");
        self.local = Local::Faulted;
        self.dirty = None;
        self.next_keepalive = None;
        self.reset = false;
        self.resume_until = None;
        self.fault_reason = reason.clone();
        self.shared.device_faulted(&self.id, self.generation, reason);
    }

    fn faulted_error(&self) -> DeviceError {
        DeviceError::Faulted(self.fault_reason.clone())
    }

    /// Handles one command; returns `false` when the thread must end.
    fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Wake => {}
            Command::SetMode {
                seq,
                state,
                out,
                reply,
            } => {
                let r = self.ordered(seq, |w| w.send_out_reset(&out));
                if r == Ok(true) {
                    self.took_control(Instant::now());
                    self.resumed();
                    self.shared.applied(
                        &self.id,
                        self.generation,
                        cc_config::DeviceSetting::Mode { state },
                        true,
                    );
                }
                let _ = reply.send(r.map(|_| ()));
            }
            Command::Release { seq, reply } => {
                let r = self.ordered(seq, |w| w.ctrl.release());
                if r == Ok(true) {
                    self.released();
                    self.mark_released();
                    self.reset = false;
                    self.resumed();
                }
                let _ = reply.send(r.map(|_| ()));
            }
            Command::Resize {
                seq,
                zone,
                len,
                reply,
            } => {
                let dropped = self.local != Local::Faulted && self.drop_frames_before(seq);
                let r = self.guarded(|w| w.ctrl.resize_zone(zone, len));
                if r.is_ok() {
                    let desc = self.ctrl.description().clone();
                    let hardware = self.ctrl.hardware_mode();
                    self.shared
                        .resized(&self.id, self.generation, zone, desc, hardware, dropped);
                }
                let _ = reply.send(r);
            }
            Command::Save(reply) => {
                self.needs_release = true;
                if self.local != Local::Faulted {
                    self.mark_written();
                }
                let r = self.guarded(|w| w.ctrl.save_to_hardware());
                let _ = reply.send(r);
            }
            Command::Reassert { seq, output } => self.reassert(seq, output),
            Command::Stop { release, reply } => {
                let r = if release && self.needs_release {
                    let r = self.ctrl.release();
                    match &r {
                        Err(e) if self.local == Local::Faulted => {
                            debug!(id = %self.id, error = %e, "release of the faulted device on stop failed");
                        }
                        Err(e) => warn!(id = %self.id, error = %e, "release on stop failed"),
                        Ok(()) => self.released(),
                    }
                    r
                } else {
                    Ok(())
                };
                if let Some(reply) = reply {
                    let _ = reply.send(r);
                }
                return false;
            }
        }
        true
    }

    /// Runs a sequenced call (mode or release): skipped with `Ok(false)` if something issued later was already
    /// applied, otherwise drops older pending frames, runs it and returns `Ok(true)` on success.
    fn ordered(
        &mut self,
        seq: u64,
        f: impl FnOnce(&mut Self) -> Result<(), DeviceError>,
    ) -> Result<bool, DeviceError> {
        if self.local == Local::Faulted {
            return Err(self.faulted_error());
        }
        if seq < self.last_seq {
            debug!(id = %self.id, "skipping superseded command");
            return Ok(false);
        }
        self.drop_frames_before(seq);
        self.guarded(f)?;
        self.last_seq = seq;
        Ok(true)
    }

    /// Starts the reset and re-send after a resume. Ignored while not driving the device (released or faulted) and
    /// when something issued later was already applied.
    fn reassert(&mut self, seq: u64, output: Output) {
        if self.local != Local::Active || seq < self.last_seq {
            debug!(id = %self.id, "skipping reassert");
            return;
        }
        self.drop_frames_before(seq);
        // A fresh start: the failures before the sleep say nothing about the device now.
        self.failures = 0;
        self.retry_at = None;
        self.reset = true;
        self.shown_mode = None;
        self.resume_until = Some(Instant::now() + self.shared.timing.resume_retry);
        if self.dirty.as_ref().is_none_or(|(s, _)| *s < seq) {
            self.dirty = Some((seq, output));
        }
    }

    fn mark_released(&mut self) {
        self.local = Local::Released;
        self.shown_mode = None;
        self.next_keepalive = None;
        self.shared
            .set_hardware(&self.id, self.generation, self.ctrl.hardware_mode());
        self.shared
            .set_status(&self.id, self.generation, DeviceStatus::Released);
    }

    /// Runs a user-requested device call unless faulted, and feeds its result into the error policy.
    fn guarded(&mut self, f: impl FnOnce(&mut Self) -> Result<(), DeviceError>) -> Result<(), DeviceError> {
        if self.local == Local::Faulted {
            return Err(self.faulted_error());
        }
        let r = f(self);
        self.record(&r);
        if self.local == Local::Faulted {
            return Err(self.faulted_error());
        }
        r
    }
}

fn max_opt(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn min_opt(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let t = Timing::default();
        assert_eq!(t.backoff(1), Duration::from_millis(50));
        assert_eq!(t.backoff(2), Duration::from_millis(100));
        assert_eq!(t.backoff(4), Duration::from_millis(400));
        assert_eq!(t.backoff(7), Duration::from_millis(3200));
        assert_eq!(t.backoff(8), Duration::from_secs(5));
        assert_eq!(t.backoff(40), Duration::from_secs(5));
    }
}
