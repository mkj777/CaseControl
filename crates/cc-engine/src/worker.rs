//! The per-device worker thread: owns one [`Controller`], paces colour frames, sends keepalives and applies the error
//! policy (exponential back-off, `Faulted` after too many consecutive failures or on a fatal error).

use std::sync::Arc;
use std::time::{Duration, Instant};

use cc_core::{Controller, DeviceDescription, DeviceError, DeviceId, ModeState, Rgb};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::engine::Shared;
use crate::view::DeviceStatus;

/// Where the latest coalesced colour frame waits for the worker. Latest wins: a newer frame replaces an unsent one.
pub(crate) type FrameSlot = Arc<Mutex<Option<Vec<Rgb>>>>;

/// Reply channel of an acknowledged command.
pub(crate) type Reply<T = ()> = Sender<Result<T, DeviceError>>;

/// Messages to a worker thread.
pub(crate) enum Command {
    /// A new frame is in the [`FrameSlot`].
    Wake,
    /// Switch to a hardware mode.
    SetMode(ModeState, Reply),
    /// Resize a zone; replies with the new description.
    Resize {
        zone: usize,
        len: usize,
        reply: Reply<DeviceDescription>,
    },
    /// Persist the current mode to flash.
    Save(Reply),
    /// Hand the device back to its hardware mode.
    Release(Reply),
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
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ack_timeout: Duration::from_secs(3),
            backoff_base: Duration::from_millis(50),
            backoff_max: Duration::from_secs(5),
            max_failures: 10,
            hotplug_poll: Duration::from_millis(100),
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
    pub id: DeviceId,
    pub generation: u64,
    pub ctrl: Box<dyn Controller>,
    pub rx: Receiver<Command>,
    pub frame: FrameSlot,
    pub shared: Arc<Shared>,
    local: Local,
    fault_reason: String,
    dirty: Option<Vec<Rgb>>,
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
            last_frame: None,
            failures: 0,
            retry_at: None,
            next_keepalive: None,
        }
    }

    /// The thread body. Returns when told to stop or when the engine dropped the command channel.
    pub(crate) fn run(mut self) {
        debug!(id = %self.id, "device thread started");
        loop {
            if let Some(f) = self.frame.lock().take() {
                self.dirty = Some(f);
            }
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

    fn frame_interval(&self) -> Duration {
        self.shared.frame_interval()
    }

    fn not_before(&self) -> Option<Instant> {
        self.retry_at
    }

    fn frame_due(&self) -> Option<Instant> {
        self.dirty.as_ref()?;
        let paced = self.last_frame.map(|t| t + self.frame_interval());
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
            && let Some(colors) = self.dirty.take()
        {
            self.send_frame(colors, now);
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

    fn send_frame(&mut self, colors: Vec<Rgb>, now: Instant) {
        if colors.len() != self.ctrl.description().led_count() {
            debug!(id = %self.id, "dropping frame with stale length");
            return;
        }
        self.last_frame = Some(now);
        let r = self.ctrl.set_direct(&colors);
        match &r {
            Ok(()) => self.took_control(now),
            Err(DeviceError::InvalidArgument(msg)) => {
                warn!(id = %self.id, %msg, "driver rejected frame");
            }
            Err(e) if !e.is_fatal() => {
                warn!(id = %self.id, error = %e, "frame write failed");
                // Retry after the back-off, with whatever frame is newest by then.
                if self.dirty.is_none() {
                    self.dirty = Some(colors);
                }
            }
            Err(_) => {}
        }
        self.record(&r);
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
            Command::SetMode(state, reply) => {
                let r = self.guarded(|w| {
                    // A mode supersedes any colour frame that has not gone out yet.
                    w.dirty = None;
                    w.ctrl.set_mode(&state)
                });
                if r.is_ok() {
                    self.took_control(Instant::now());
                }
                let _ = reply.send(r);
            }
            Command::Resize { zone, len, reply } => {
                let r = self
                    .guarded(|w| w.ctrl.resize_zone(zone, len))
                    .map(|()| self.ctrl.description().clone());
                let _ = reply.send(r);
            }
            Command::Save(reply) => {
                let r = self.guarded(|w| w.ctrl.save_to_hardware());
                let _ = reply.send(r);
            }
            Command::Release(reply) => {
                let r = self.guarded(|w| {
                    w.dirty = None;
                    w.ctrl.release()
                });
                if r.is_ok() {
                    self.mark_released();
                }
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

    fn mark_released(&mut self) {
        self.local = Local::Released;
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
