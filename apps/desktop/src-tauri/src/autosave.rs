//! Coalesced autosave of the active profile (ADR 0001 #40).
//!
//! Every change to what a device shows marks the active profile as changed ([`Autosave::mark`]). A background thread
//! ([`Autosave::wait_due`] in a loop) writes it at most once per interval (about 300 ms): the first change after a
//! quiet period is written at once, later ones in the same interval are folded into one write at its end. The write
//! takes the values at the time it runs, so the last value of a burst (a colour picker drag, a slider) is always what
//! ends up on disk. Profile operations and the exit path call [`Autosave::flush`] to write a pending change at once;
//! [`Autosave::stop`] ends the thread's loop.
//!
//! The type only does the timing; the write itself is a closure (the app's writes the engine's state into the
//! active profile file).

use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

/// The interval the app uses: at most one profile write per 300 ms.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_millis(300);

#[derive(Debug, Default)]
struct Inner {
    /// A change was marked and not written yet.
    dirty: bool,
    /// When the last write started.
    last_write: Option<Instant>,
    /// [`Autosave::stop`] was called.
    stopped: bool,
}

/// Change marker and write timing of the autosave. `Send + Sync`; shared by the commands and the writer thread.
#[derive(Debug)]
pub struct Autosave {
    inner: Mutex<Inner>,
    cv: Condvar,
    interval: Duration,
}

impl Autosave {
    /// An autosave that writes at most once per `interval`.
    pub fn new(interval: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
            interval,
        }
    }

    /// Records that the active profile changed; the writer thread writes it once it is due.
    pub fn mark(&self) {
        self.inner.lock().dirty = true;
        self.cv.notify_all();
    }

    /// Whether a change is waiting to be written.
    pub fn is_dirty(&self) -> bool {
        self.inner.lock().dirty
    }

    /// Runs `write` now if a change is pending and returns whether it did. The mark is cleared before `write` runs, so
    /// a change made during the write is written next time. Callers serialise flushes with their own lock (the app
    /// holds its profile lock), so a write never overlaps another.
    pub fn flush(&self, write: impl FnOnce()) -> bool {
        {
            let mut inner = self.inner.lock();
            if !inner.dirty {
                return false;
            }
            inner.dirty = false;
            inner.last_write = Some(Instant::now());
        }
        write();
        true
    }

    /// Blocks until a pending change is due (`interval` after the previous write started) and returns `true`, or
    /// returns `false` once [`stop`](Self::stop) was called. The caller then flushes.
    pub fn wait_due(&self) -> bool {
        let mut inner = self.inner.lock();
        loop {
            if inner.stopped {
                return false;
            }
            if inner.dirty {
                let due = inner.last_write.map_or_else(Instant::now, |t| t + self.interval);
                if Instant::now() >= due {
                    return true;
                }
                self.cv.wait_until(&mut inner, due);
            } else {
                self.cv.wait(&mut inner);
            }
        }
    }

    /// Ends [`wait_due`](Self::wait_due) loops. A pending change stays pending; flush it after stopping.
    pub fn stop(&self) {
        self.inner.lock().stopped = true;
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// The value a write stores (the "device state") and every value written, in order.
    #[derive(Default)]
    struct Target {
        value: AtomicU32,
        written: Mutex<Vec<u32>>,
    }

    impl Target {
        fn write(&self) {
            let v = self.value.load(Ordering::SeqCst);
            self.written.lock().push(v);
        }
    }

    /// Starts the writer loop the app runs: wait until due, then flush under the lock.
    fn writer(
        autosave: &Arc<Autosave>,
        target: &Arc<Target>,
        lock: &Arc<Mutex<()>>,
    ) -> std::thread::JoinHandle<()> {
        let (autosave, target, lock) = (Arc::clone(autosave), Arc::clone(target), Arc::clone(lock));
        std::thread::spawn(move || {
            while autosave.wait_due() {
                let _guard = lock.lock();
                autosave.flush(|| target.write());
            }
        })
    }

    #[test]
    fn a_burst_is_coalesced_and_the_last_value_wins() {
        let interval = Duration::from_millis(100);
        let autosave = Arc::new(Autosave::new(interval));
        let target = Arc::new(Target::default());
        let lock = Arc::new(Mutex::new(()));
        let thread = writer(&autosave, &target, &lock);

        let start = Instant::now();
        for v in 1..=200 {
            target.value.store(v, Ordering::SeqCst);
            autosave.mark();
            std::thread::sleep(Duration::from_millis(2));
        }
        let elapsed = start.elapsed();
        // Wait for the trailing write of the burst.
        let deadline = Instant::now() + Duration::from_secs(2);
        while autosave.is_dirty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(interval);
        autosave.stop();
        thread.join().expect("writer thread");

        let written = target.written.lock().clone();
        assert_eq!(
            written.last(),
            Some(&200),
            "the last value is written: {written:?}"
        );
        let most = u32::try_from(elapsed.as_millis() / interval.as_millis()).expect("small") + 2;
        assert!(
            u32::try_from(written.len()).expect("small") <= most,
            "{} writes for {elapsed:?}, at most {most} expected",
            written.len()
        );
        assert!(written.len() < 200, "writes are coalesced");
    }

    #[test]
    fn a_quiet_change_is_written_promptly() {
        let autosave = Arc::new(Autosave::new(Duration::from_millis(300)));
        let target = Arc::new(Target::default());
        let lock = Arc::new(Mutex::new(()));
        let thread = writer(&autosave, &target, &lock);
        target.value.store(7, Ordering::SeqCst);
        autosave.mark();
        let deadline = Instant::now() + Duration::from_millis(250);
        while target.written.lock().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            *target.written.lock(),
            [7],
            "the first change after a quiet time is not delayed"
        );
        autosave.stop();
        thread.join().expect("writer thread");
    }

    #[test]
    fn shutdown_flushes_a_pending_change() {
        // A long interval: the change made right after a write is not due yet when the app exits.
        let autosave = Arc::new(Autosave::new(Duration::from_secs(60)));
        let target = Arc::new(Target::default());
        let lock = Arc::new(Mutex::new(()));
        let thread = writer(&autosave, &target, &lock);
        target.value.store(1, Ordering::SeqCst);
        autosave.mark();
        let deadline = Instant::now() + Duration::from_secs(2);
        while target.written.lock().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        target.value.store(2, Ordering::SeqCst);
        autosave.mark();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            *target.written.lock(),
            [1],
            "the second change waits for its interval"
        );

        // The exit path: stop the loop, then flush under the lock.
        autosave.stop();
        thread.join().expect("writer thread");
        let _guard = lock.lock();
        assert!(autosave.flush(|| target.write()));
        assert_eq!(*target.written.lock(), [1, 2]);
        assert!(!autosave.flush(|| target.write()), "nothing left to write");
    }
}
