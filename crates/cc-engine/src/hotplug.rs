//! Hot-plug: one notification source for the whole engine, and the thread that turns its events into targeted
//! re-detection (arrivals) and device removal.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use cc_core::{HotplugAction, HotplugHandle, HotplugSender};
use tracing::{debug, warn};

use crate::engine::Shared;

/// Something that delivers HID arrival/removal events and can list the HID paths present right now. The engine uses
/// `cc_hid::hotplug::watch` and `cc_hid::enumerate`; tests inject both.
pub(crate) trait HotplugSource: Send {
    /// Starts delivering events to `tx` until the returned handle is stopped.
    fn watch(&mut self, tx: HotplugSender) -> Result<Box<dyn HotplugHandle>, String>;

    /// Paths of every HID device currently present.
    fn present_paths(&self) -> Result<Vec<String>, String>;
}

/// The real source: `CM_Register_Notification` on the HID interface class.
#[derive(Debug)]
pub(crate) struct HidHotplugSource;

impl HotplugSource for HidHotplugSource {
    #[cfg(windows)]
    fn watch(&mut self, tx: HotplugSender) -> Result<Box<dyn HotplugHandle>, String> {
        cc_hid::hotplug::watch(tx).map_err(|e| e.to_string())
    }

    #[cfg(not(windows))]
    fn watch(&mut self, _tx: HotplugSender) -> Result<Box<dyn HotplugHandle>, String> {
        Err("hot-plug notifications are only implemented on Windows".into())
    }

    fn present_paths(&self) -> Result<Vec<String>, String> {
        cc_hid::enumerate()
            .map(|list| list.into_iter().map(|d| d.path).collect())
            .map_err(|e| e.to_string())
    }
}

/// A running hot-plug subscription and its event thread.
pub(crate) struct HotplugRuntime {
    handle: Box<dyn HotplugHandle>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    done: crossbeam_channel::Receiver<()>,
}

impl HotplugRuntime {
    /// Subscribes via `source` and starts the event thread. Returns `None` (and logs) if either fails.
    pub(crate) fn start(mut source: Box<dyn HotplugSource>, shared: &Arc<Shared>) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let handle = match source.watch(tx) {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, "hot-plug notifications unavailable");
                return None;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let weak = Arc::downgrade(shared);
        let poll = shared.timing.hotplug_poll;
        let (done_tx, done) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new()
            .name("cc-hotplug".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    let ev = match rx.recv_timeout(poll) {
                        Ok(ev) => ev,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    let Some(shared) = weak.upgrade() else { break };
                    debug!(?ev, "hot-plug event");
                    match ev.action {
                        HotplugAction::Arrived => Shared::detect_new(&shared, Some(ev.bus)),
                        HotplugAction::Removed => shared.handle_removal(&ev, source.as_ref()),
                    }
                }
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(thread) => Some(Self {
                handle,
                stop,
                thread,
                done,
            }),
            Err(e) => {
                warn!(error = %e, "could not start hot-plug thread");
                handle.stop();
                None
            }
        }
    }

    /// Stops the subscription and joins the event thread if it ends within `timeout`; otherwise (it is inside a
    /// detection pass) it is left to finish on its own.
    pub(crate) fn stop_within(self, timeout: Duration) {
        self.stop.store(true, Ordering::Release);
        self.handle.stop();
        // Disconnected means the thread ended without signalling (it panicked); joining is then immediate.
        if !matches!(
            self.done.recv_timeout(timeout),
            Err(crossbeam_channel::RecvTimeoutError::Timeout)
        ) {
            if self.thread.join().is_err() {
                warn!("hot-plug thread panicked");
            }
        } else {
            warn!("hot-plug thread did not stop in time; leaving it behind");
        }
    }
}
