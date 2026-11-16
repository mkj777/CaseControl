//! Resume from sleep: one suspend/resume subscription (`cc_hid::power`) and the thread that turns a resume into
//! [`EngineHandle::reassert`](crate::EngineHandle::reassert). The debounce and all device work live in the engine;
//! this module only forwards.

use std::sync::Arc;

use crate::engine::Shared;

/// A running resume subscription and its forwarding thread.
pub(crate) struct ResumeRuntime {
    #[cfg(windows)]
    watch: cc_hid::power::PowerWatch,
    stop: crossbeam_channel::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl ResumeRuntime {
    /// Subscribes to power broadcasts and starts the forwarding thread. Returns `None` (and logs) if either fails,
    /// and always off Windows.
    #[cfg(windows)]
    pub(crate) fn start(shared: &Arc<Shared>) -> Option<Self> {
        use cc_hid::power::PowerEvent;
        use tracing::{debug, info, warn};

        let (tx, rx) = crossbeam_channel::unbounded();
        let watch = match cc_hid::power::watch(tx) {
            Ok(w) => w,
            Err(e) => {
                warn!(error = %e, "resume notifications unavailable; devices are not re-asserted after sleep");
                return None;
            }
        };
        let (stop, stop_rx) = crossbeam_channel::bounded::<()>(0);
        let weak = Arc::downgrade(shared);
        let spawned = std::thread::Builder::new()
            .name("cc-power".into())
            .spawn(move || {
                loop {
                    crossbeam_channel::select! {
                        recv(stop_rx) -> _ => break,
                        recv(rx) -> ev => match ev {
                            Ok(PowerEvent::Suspend) => info!("system suspending"),
                            Ok(PowerEvent::Resume) => {
                                let Some(shared) = weak.upgrade() else { break };
                                debug!("resume broadcast");
                                Shared::reassert(&shared);
                            }
                            Err(_) => break,
                        },
                    }
                }
            });
        match spawned {
            Ok(thread) => Some(Self { watch, stop, thread }),
            Err(e) => {
                warn!(error = %e, "could not start the resume thread");
                None
            }
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn start(_shared: &Arc<Shared>) -> Option<Self> {
        None
    }

    /// Unsubscribes and joins the forwarding thread (it never blocks: a reassert runs on its own thread).
    pub(crate) fn stop(self) {
        #[cfg(windows)]
        drop(self.watch);
        drop(self.stop);
        if self.thread.join().is_err() {
            tracing::warn!("resume thread panicked");
        }
    }
}
