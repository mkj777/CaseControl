//! Named-mutex guard for buses shared with other hardware tools.
//!
//! Tools such as FanControl/LibreHardwareMonitor, HWiNFO and OpenRGB serialize access to SMBus,
//! ISA bus, PCI config space, the embedded controller and Corsair Link devices through well-known
//! named mutexes. [`NamedMutex`] opens or creates one of them; [`NamedMutex::lock`] waits for it.
//! An abandoned mutex (`WAIT_ABANDONED`, the previous owner died while holding it) counts as
//! acquired, the same policy as LibreHardwareMonitor's `Mutexes.cs`. Hold it as briefly as possible.
//!
//! Unused by the v1 drivers; present for the SMBus/Corsair tier.

use std::marker::PhantomData;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, HANDLE, WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{
    CreateMutexW, MUTEX_MODIFY_STATE, OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE,
    WaitForSingleObject,
};
use windows::core::HSTRING;

use crate::transport::HidError;

/// `Global\Access_SMBUS.HTP.Method`: SMBus access.
pub const GLOBAL_SMBUS: &str = r"Global\Access_SMBUS.HTP.Method";
/// `Global\Access_ISABUS.HTP.Method`: ISA bus / Super I/O port access.
pub const GLOBAL_ISABUS: &str = r"Global\Access_ISABUS.HTP.Method";
/// `Global\Access_PCI`: PCI configuration space access.
pub const GLOBAL_PCI: &str = r"Global\Access_PCI";
/// `Global\Access_EC`: embedded controller access.
pub const GLOBAL_EC: &str = r"Global\Access_EC";
/// `Global\CorsairLinkReadWriteGuardMutex`: Corsair Link / iCUE LINK device access.
pub const GLOBAL_CORSAIR_LINK: &str = r"Global\CorsairLinkReadWriteGuardMutex";

/// An opened named mutex. The handle is closed on drop.
#[derive(Debug)]
pub struct NamedMutex {
    handle: HANDLE,
    name: String,
}

// SAFETY: a mutex HANDLE is a process-wide kernel object reference that may be used and closed
// from any thread. Ownership of the *lock* is per thread, which `MutexGuard` enforces by being
// `!Send`.
unsafe impl Send for NamedMutex {}
// SAFETY: `WaitForSingleObject` on the same handle from several threads is allowed; the kernel
// serializes ownership. `&NamedMutex` exposes no other operation.
unsafe impl Sync for NamedMutex {}

impl NamedMutex {
    /// Open the named mutex, creating it if absent.
    ///
    /// If creating is denied (the mutex exists with a restrictive ACL), falls back to opening it
    /// with `SYNCHRONIZE | MUTEX_MODIFY_STATE`.
    pub fn open_or_create(name: &str) -> Result<NamedMutex, HidError> {
        let wide = HSTRING::from(name);
        // SAFETY: `wide` is a NUL-terminated UTF-16 string alive for the call; no security
        // attributes pointer is passed. The returned handle is owned by `NamedMutex`.
        let created = unsafe { CreateMutexW(None, false, &wide) };
        let handle = match created {
            Ok(h) => h,
            Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => {
                // SAFETY: as above; the returned handle is owned by `NamedMutex`.
                unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE, false, &wide) }
                    .map_err(|e| HidError::Io(format!("OpenMutexW({name}): {e}")))?
            }
            Err(e) => return Err(HidError::Io(format!("CreateMutexW({name}): {e}"))),
        };
        Ok(NamedMutex {
            handle,
            name: name.to_owned(),
        })
    }

    /// The name this mutex was opened with.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Wait up to `timeout` for the mutex. `WAIT_ABANDONED` counts as acquired.
    ///
    /// Durations beyond `u32::MAX - 1` milliseconds are capped (never an infinite wait).
    pub fn lock(&self, timeout: Duration) -> Result<MutexGuard<'_>, HidError> {
        let ms = u32::try_from(timeout.as_millis())
            .unwrap_or(u32::MAX)
            .min(u32::MAX - 1);
        // SAFETY: `self.handle` is a valid mutex handle owned by `self` until drop.
        let r = unsafe { WaitForSingleObject(self.handle, ms) };
        match r {
            WAIT_OBJECT_0 => {}
            WAIT_ABANDONED => {
                tracing::warn!(target: "cc_hid::mutex", name = %self.name, "acquired abandoned mutex");
            }
            WAIT_TIMEOUT => return Err(HidError::Timeout),
            WAIT_FAILED => {
                return Err(HidError::Io(format!(
                    "WaitForSingleObject({}): {}",
                    self.name,
                    windows::core::Error::from_thread()
                )));
            }
            other => {
                return Err(HidError::Io(format!(
                    "WaitForSingleObject({}): unexpected result {:#x}",
                    self.name, other.0
                )));
            }
        }
        Ok(MutexGuard {
            mutex: self,
            _not_send: PhantomData,
        })
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        // SAFETY: `self.handle` is owned and closed exactly once, here. Guards borrow `self`, so
        // none is alive.
        if let Err(e) = unsafe { CloseHandle(self.handle) } {
            tracing::warn!(target: "cc_hid::mutex", name = %self.name, "CloseHandle failed: {e}");
        }
    }
}

/// Holds the named mutex; releases it on drop. `!Send`: Windows mutex ownership belongs to
/// the acquiring thread, so the release must happen there.
#[derive(Debug)]
pub struct MutexGuard<'a> {
    mutex: &'a NamedMutex,
    _not_send: PhantomData<*const ()>,
}

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle is valid for `'a`, and this thread owns the mutex (acquired in
        // `lock` on this thread; the guard cannot leave it).
        if let Err(e) = unsafe { ReleaseMutex(self.mutex.handle) } {
            tracing::warn!(target: "cc_hid::mutex", name = %self.mutex.name, "ReleaseMutex failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_unlock_twice() {
        let m = NamedMutex::open_or_create(r"Local\CaseControlTest").expect("create");
        for _ in 0..2 {
            let g = m.lock(Duration::from_secs(1)).expect("lock");
            drop(g);
        }
        assert_eq!(m.name(), r"Local\CaseControlTest");
    }

    #[test]
    fn other_thread_times_out_while_held() {
        let m = NamedMutex::open_or_create(r"Local\CaseControlTestContention").expect("create");
        let g = m.lock(Duration::from_secs(1)).expect("lock");
        std::thread::scope(|s| {
            s.spawn(|| {
                let other =
                    NamedMutex::open_or_create(r"Local\CaseControlTestContention").expect("open existing");
                assert!(matches!(
                    other.lock(Duration::from_millis(50)),
                    Err(HidError::Timeout)
                ));
            });
        });
        drop(g);
    }
}
