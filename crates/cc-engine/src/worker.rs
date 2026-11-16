//! The per-device worker thread: owns one [`Controller`], paces colour frames, sends keepalives and applies the error
//! policy (exponential back-off, `Faulted` after too many consecutive failures or on a fatal error).
//!
//! Ordering: every colour frame, mode change, release and resize carries a sequence number taken when the caller
//! issued it. The worker applies them in sequence order and skips anything older than what it last applied, so the
//! device always ends up showing the most recently issued request. The worker, not the caller, records what was
//! actually applied (setting, `DeviceStateChanged`, resized description), so the engine's record matches the device.
//!
//! Brightness (ADR 0001 #33): what the worker sends ([`Out`]) can differ from the unscaled setting it records
//! ([`Output::stored`]). The latest-wins slot carries brightness and fade steps for hardware modes too, paced like
//! colour frames; a mode output equal to the one the device already shows is not sent again, so a mode without a
//! brightness parameter does not restart its effect on every fade step.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cc_core::{Controller, DeviceError, DeviceId, ModeState};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::engine::Shared;
use crate::lighting::{Out, Output};
use crate::view::DeviceStatus;

/// An output (colour frame or brightness / fade step of a mode) and the sequence number it was issued with.
pub(crate) type Frame = (u64, Output);

/// Where the latest coalesced output waits for the worker. Latest wins: a newer output replaces an unsent one.
pub(crate) type FrameSlot = Arc<Mutex<Option<Frame>>>;

/// Reply channel of an acknowledged command.
pub(crate) type Reply = Sender<Result<(), DeviceError>>;

/// Messages to a worker thread.
pub(crate) enum Command {
    /// A new frame is in the [`FrameSlot`].
    Wake,
    /// Switch to a hardware mode: send `out` (the mode at the current brightness, or the off output while All off
    /// holds the device dark), record `state`.
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
    /// Leave the thread, releasing first if `release` and the device is Active.
    Stop { release: bool, reply: Option<Reply> },
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
    /// Length of a full All off / on fade.
    pub fade: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ack_timeout: Duration::from_secs(3),
            backoff_base: Duration::from_millis(50),
            backoff_max: Duration::from_secs(5),
            max_failures: 10,
            hotplug_poll: Duration::from_millis(100),
            fade: Duration::from_millis(500),
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
    /// Sequence number of the last frame, mode or release that was applied (or superseded everything before it).
    last_seq: u64,
    last_frame: Option<Instant>,
    failures: u32,
    retry_at: Option<Instant>,
    next_keepalive: Option<Instant>,
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
            local: Local::Active,
            fault_reason: String::new(),
            dirty: None,
            shown_mode: None,
            last_seq: 0,
            last_frame: None,
            failures: 0,
            retry_at: None,
            next_keepalive: None,
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
            && matches!(&output.out, Out::Mode(m) if self.shown_mode.as_ref() == Some(m));
        let r = if unchanged {
            Ok(())
        } else {
            self.last_frame = Some(now);
            self.send_out(&output.out)
        };
        match &r {
            Ok(()) => {
                self.last_seq = seq;
                self.took_control(now);
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

    /// Sends one output to the device and remembers which mode it shows.
    fn send_out(&mut self, out: &Out) -> Result<(), DeviceError> {
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
                if self.failures >= timing.max_failures {
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
                let r = self.ordered(seq, |w| w.send_out(&out));
                if r == Ok(true) {
                    self.took_control(Instant::now());
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
                    self.mark_released();
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
                    self.shared
                        .resized(&self.id, self.generation, zone, desc, dropped);
                }
                let _ = reply.send(r);
            }
            Command::Save(reply) => {
                let r = self.guarded(|w| w.ctrl.save_to_hardware());
                let _ = reply.send(r);
            }
            Command::Stop { release, reply } => {
                let r = if release && self.local == Local::Active {
                    let r = self.ctrl.release();
                    if let Err(e) = &r {
                        warn!(id = %self.id, error = %e, "release on stop failed");
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

    fn mark_released(&mut self) {
        self.local = Local::Released;
        self.shown_mode = None;
        self.next_keepalive = None;
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
