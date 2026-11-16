//! Windows suspend and resume notifications.
//!
//! [`watch`] registers a `PowerRegisterSuspendResumeNotification` callback (`DEVICE_NOTIFY_CALLBACK`, so no window
//! and no message loop are needed) and forwards one [`PowerEvent`] per power broadcast it cares about:
//! `PBT_APMSUSPEND` as [`PowerEvent::Suspend`], `PBT_APMRESUMEAUTOMATIC` and `PBT_APMRESUMESUSPEND` as
//! [`PowerEvent::Resume`]. A wake by the user usually delivers both resume broadcasts, so a consumer must debounce;
//! this module forwards them as they come. The callback does nothing but a channel send.
//!
//! It lives in this crate because it is the crate that already holds the engine's other OS notification (HID
//! hot-plug) and is allowed `unsafe` FFI.

use std::ffi::c_void;

use crossbeam_channel::Sender;
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
    PowerUnregisterSuspendResumeNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
};

use crate::transport::HidError;

/// A power transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// The system is about to sleep or hibernate (`PBT_APMSUSPEND`).
    Suspend,
    /// The system woke up (`PBT_APMRESUMEAUTOMATIC` or `PBT_APMRESUMESUSPEND`; usually both arrive).
    Resume,
}

/// The power event a broadcast type stands for, if any.
pub fn classify(broadcast: u32) -> Option<PowerEvent> {
    match broadcast {
        PBT_APMSUSPEND => Some(PowerEvent::Suspend),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(PowerEvent::Resume),
        _ => None,
    }
}

unsafe extern "system" fn on_power(context: *const c_void, broadcast: u32, _setting: *const c_void) -> u32 {
    let Some(event) = classify(broadcast) else {
        return 0;
    };
    if context.is_null() {
        return 0;
    }
    // SAFETY: `context` is the `Box<Sender<PowerEvent>>` pointer registered in `watch`. That box is never freed
    // (see `PowerWatch::drop`), so it is valid for every callback, also one racing an unregistration. Only shared
    // access is taken; `Sender` is `Sync`.
    let tx = unsafe { &*context.cast::<Sender<PowerEvent>>() };
    // A closed channel means nobody listens any more; dropping the event is correct.
    let _ = tx.send(event);
    0 // ERROR_SUCCESS
}

/// A running registration; unregisters on drop.
#[derive(Debug)]
pub struct PowerWatch {
    handle: *mut c_void,
}

// SAFETY: the registration handle is an opaque OS handle usable from any thread; it is unregistered exactly once,
// in `drop`. The callback context it refers to is leaked, never freed, so moving the watch between threads cannot
// invalidate anything the OS still uses.
unsafe impl Send for PowerWatch {}

impl Drop for PowerWatch {
    fn drop(&mut self) {
        // SAFETY: `handle` came from a successful `PowerRegisterSuspendResumeNotification` and is unregistered
        // exactly once (drop runs once).
        let r = unsafe { PowerUnregisterSuspendResumeNotification(HPOWERNOTIFY(self.handle as isize)) };
        if r != ERROR_SUCCESS {
            tracing::warn!(target: "cc_hid::power", code = r.0, "PowerUnregisterSuspendResumeNotification failed");
        }
        // The callback context (one channel sender) and the subscribe parameters are leaked on purpose: Windows
        // does not document that unregistering waits for a callback in flight, and the watch lives as long as the
        // process anyway.
    }
}

/// Starts forwarding suspend and resume broadcasts to `tx` until the returned watch is dropped.
pub fn watch(tx: Sender<PowerEvent>) -> Result<PowerWatch, HidError> {
    let context: *mut Sender<PowerEvent> = Box::into_raw(Box::new(tx));
    let params: *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
        Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(on_power),
            Context: context.cast::<c_void>(),
        }));
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: with `DEVICE_NOTIFY_CALLBACK` the recipient is a pointer to a `DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS`;
    // `params` is a valid heap pointer that is never freed after a successful registration (leaked with the
    // context, see `PowerWatch::drop`). `handle` is a live local.
    let r = unsafe {
        PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(params.cast::<c_void>()),
            &mut handle,
        )
    };
    if r != ERROR_SUCCESS {
        // SAFETY: registration failed, so the OS holds neither pointer; both boxes are reclaimed exactly once.
        unsafe {
            drop(Box::from_raw(params));
            drop(Box::from_raw(context));
        }
        return Err(HidError::Io(format!(
            "PowerRegisterSuspendResumeNotification failed (error {})",
            r.0
        )));
    }
    tracing::debug!(target: "cc_hid::power", "suspend/resume notifications registered");
    Ok(PowerWatch { handle })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_broadcasts() {
        assert_eq!(classify(0x4), Some(PowerEvent::Suspend));
        assert_eq!(classify(0x7), Some(PowerEvent::Resume));
        assert_eq!(classify(0x12), Some(PowerEvent::Resume));
        // PBT_APMPOWERSTATUSCHANGE, PBT_POWERSETTINGCHANGE.
        assert_eq!(classify(0xA), None);
        assert_eq!(classify(0x8013), None);
    }

    #[test]
    fn callback_forwards_resume_only_for_known_broadcasts() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let tx = Box::new(tx);
        let ctx = std::ptr::from_ref::<Sender<PowerEvent>>(&tx).cast::<c_void>();
        // SAFETY: `ctx` points to a live `Sender` for the duration of the calls.
        unsafe {
            on_power(ctx, 0x12, std::ptr::null());
            on_power(ctx, 0xA, std::ptr::null());
            on_power(ctx, 0x4, std::ptr::null());
            on_power(std::ptr::null(), 0x7, std::ptr::null());
        }
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [PowerEvent::Resume, PowerEvent::Suspend]
        );
    }

    #[test]
    fn watch_registers_and_unregisters() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let watch = watch(tx).expect("registration succeeds");
        drop(watch);
    }
}
