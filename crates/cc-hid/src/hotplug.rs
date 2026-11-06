//! Windows HID hot-plug notifications.
//!
//! [`watch`] registers a `CM_Register_Notification` callback for device-interface arrival and
//! removal on `GUID_DEVINTERFACE_HID`. The callback does minimal work: it copies the symbolic
//! link and the action and sends them on an internal channel. A watcher thread parses the
//! vid/pid from the link, debounces per `(vid, pid)` for [`DEBOUNCE`] (a device with several
//! HID collections fires one notification per collection), and forwards one
//! [`HotplugEvent`] per burst carrying the last action and link seen.
//!
//! The link in [`HotplugEvent::path`] is lower-cased before it is sent. `HidDeviceInfo::path`
//! from the `hidapi` backend is *not* normalised and comes back in mixed case (for example
//! `\\?\HID#VID_046D&PID_0A78&MI_02&Col03#a&39103f03&0&0002#{4d1e55b2-...}`). Windows device
//! paths are case-insensitive, so compare the two with `eq_ignore_ascii_case`, never with `==`,
//! and pass `HidDeviceInfo::path` to [`open`](crate::open) unchanged. [`parse_symbolic_link`]
//! stays case-insensitive.
//!
//! `stop()` (or dropping the handle) wakes the watcher thread, which calls
//! `CM_Unregister_Notification` itself. That call waits for in-flight callbacks to finish, so
//! the callback context is freed only afterwards. It is never called from inside the callback.

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::offset_of;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cc_core::backend::{HotplugAction, HotplugEvent, HotplugHandle, HotplugSender};
use cc_core::device::Bus;
use crossbeam_channel::{Receiver, Sender, select};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL,
    CM_NOTIFY_EVENT_DATA, CM_NOTIFY_EVENT_DATA_0_0, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0,
    CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification,
    CM_Unregister_Notification, CR_SUCCESS, HCMNOTIFICATION,
};
use windows::Win32::Devices::HumanInterfaceDevice::GUID_DEVINTERFACE_HID;

use crate::transport::HidError;

/// Quiet period per `(vid, pid)` before one event is forwarded.
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// Extract `(vid, pid)` from a device-interface symbolic link such as
/// `\\?\HID#VID_1462&PID_7D75#...`. Case-insensitive; `None` if either id is missing or not hex.
pub fn parse_symbolic_link(link: &str) -> Option<(u16, u16)> {
    let upper = link.to_ascii_uppercase();
    let hex4 = |at: usize| -> Option<u16> {
        let digits = upper.get(at..at + 4)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u16::from_str_radix(digits, 16).ok()
    };
    let vid_at = upper.find("VID_")? + 4;
    let vid = hex4(vid_at)?;
    // The PID must directly follow the VID: `VID_xxxx&PID_xxxx`.
    let pid_key = vid_at + 4;
    if !upper.get(pid_key..)?.starts_with("&PID_") {
        return None;
    }
    let pid = hex4(pid_key + 5)?;
    Some((vid, pid))
}

/// What the OS callback forwards to the watcher thread.
struct RawEvent {
    action: HotplugAction,
    link: String,
}

/// Owned by the OS registration; freed only after `CM_Unregister_Notification` returned.
struct CallbackContext {
    tx: Sender<RawEvent>,
}

/// Offset of the symbolic link (a NUL-terminated UTF-16 string) inside the event data.
const LINK_OFFSET: usize =
    offset_of!(CM_NOTIFY_EVENT_DATA, u) + offset_of!(CM_NOTIFY_EVENT_DATA_0_0, SymbolicLink);

unsafe extern "system" fn on_notify(
    _hnotify: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    eventdata: *const CM_NOTIFY_EVENT_DATA,
    eventdatasize: u32,
) -> u32 {
    let action = match action {
        CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL => HotplugAction::Arrived,
        CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL => HotplugAction::Removed,
        _ => return 0,
    };
    let size = eventdatasize as usize;
    if context.is_null() || eventdata.is_null() || size <= LINK_OFFSET {
        return 0;
    }
    let max_chars = (size - LINK_OFFSET) / 2;
    // SAFETY: `eventdata` is non-null and the OS guarantees it points to `eventdatasize` readable
    // bytes for the duration of this callback. `LINK_OFFSET + 2 * max_chars <= size`, so the
    // slice stays inside that block. The link is a u16 array inside a #[repr(C)] struct, so the
    // address is 2-byte aligned.
    let wide: &[u16] = unsafe {
        let base = eventdata.cast::<u8>().add(LINK_OFFSET).cast::<u16>();
        std::slice::from_raw_parts(base, max_chars)
    };
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    let link = String::from_utf16_lossy(&wide[..len]);
    // SAFETY: `context` is the `Box<CallbackContext>` pointer passed to `CM_Register_Notification`.
    // It is freed only after `CM_Unregister_Notification` returned, which waits for running
    // callbacks, so it is valid here. Only shared access is taken.
    let ctx = unsafe { &*context.cast::<CallbackContext>() };
    // A closed channel means the watcher is shutting down; dropping the event is correct.
    let _ = ctx.tx.send(RawEvent { action, link });
    0 // ERROR_SUCCESS
}

/// Running watcher; stops on [`HotplugHandle::stop`] or drop.
#[derive(Debug)]
pub struct HidHotplugHandle {
    stop_tx: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl HotplugHandle for HidHotplugHandle {
    fn stop(self: Box<Self>) {
        // Drop does the work.
    }
}

impl Drop for HidHotplugHandle {
    fn drop(&mut self) {
        self.stop_tx.take();
        if let Some(t) = self.thread.take()
            && t.join().is_err()
        {
            tracing::error!(target: "cc_hid::hotplug", "hot-plug watcher thread panicked");
        }
    }
}

/// The registration and its context, moved into the watcher thread.
struct Registration {
    handle: HCMNOTIFICATION,
    context: *mut CallbackContext,
}

// SAFETY: `HCMNOTIFICATION` is an opaque OS handle usable from any thread, and the context
// pointer is only dereferenced by the OS callback (shared, `Sender` is `Sync`) and freed once
// by `Registration::drop` after the registration is gone.
unsafe impl Send for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: `self.handle` came from a successful `CM_Register_Notification` and is
        // unregistered exactly once (drop runs once). This runs on the watcher thread (or on the
        // caller thread if spawning it failed), never inside the callback.
        let cr = unsafe { CM_Unregister_Notification(self.handle) };
        if cr != CR_SUCCESS {
            tracing::warn!(target: "cc_hid::hotplug", code = cr.0, "CM_Unregister_Notification failed");
        }
        // SAFETY: `context` came from `Box::into_raw` in `watch` and is reclaimed exactly once,
        // here; after unregistration (which waits for in-flight callbacks) the OS no longer uses it.
        drop(unsafe { Box::from_raw(self.context) });
    }
}

/// Start watching HID arrivals and removals; events go to `tx`, debounced per `(vid, pid)`.
pub fn watch(tx: HotplugSender) -> Result<Box<dyn HotplugHandle>, HidError> {
    let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<RawEvent>();
    let context = Box::into_raw(Box::new(CallbackContext { tx: raw_tx }));

    let filter = CM_NOTIFY_FILTER {
        cbSize: u32::try_from(size_of::<CM_NOTIFY_FILTER>()).expect("filter size fits u32"),
        Flags: 0,
        FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        Reserved: 0,
        u: CM_NOTIFY_FILTER_0 {
            DeviceInterface: CM_NOTIFY_FILTER_0_0 {
                ClassGuid: GUID_DEVINTERFACE_HID,
            },
        },
    };
    let mut handle = HCMNOTIFICATION::default();
    // SAFETY: `filter` and `handle` are live locals for the duration of the call; `context` is a
    // valid heap pointer that stays alive until `Registration::drop` frees it (or below on
    // failure, when the OS never saw it registered).
    let cr = unsafe {
        CM_Register_Notification(
            &filter,
            Some(context.cast_const().cast::<c_void>()),
            Some(on_notify),
            &mut handle,
        )
    };
    if cr != CR_SUCCESS {
        // SAFETY: registration failed, so no callback holds `context`; reclaim the box once.
        drop(unsafe { Box::from_raw(context) });
        return Err(HidError::Io(format!(
            "CM_Register_Notification failed (CONFIGRET {})",
            cr.0
        )));
    }
    let registration = Registration { handle, context };

    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
    let thread = std::thread::Builder::new()
        .name("cc-hid-hotplug".into())
        .spawn(move || {
            run_watcher(&raw_rx, &stop_rx, &tx);
            drop(registration);
        })
        .map_err(|e| HidError::Io(format!("spawning hot-plug thread: {e}")))?;
    tracing::debug!(target: "cc_hid::hotplug", "HID hot-plug watcher registered");

    Ok(Box::new(HidHotplugHandle {
        stop_tx: Some(stop_tx),
        thread: Some(thread),
    }))
}

struct Pending {
    deadline: Instant,
    action: HotplugAction,
    link: String,
}

fn run_watcher(raw_rx: &Receiver<RawEvent>, stop_rx: &Receiver<()>, tx: &HotplugSender) {
    let mut pending: HashMap<(u16, u16), Pending> = HashMap::new();
    loop {
        let timeout = pending
            .values()
            .map(|p| p.deadline.saturating_duration_since(Instant::now()))
            .min()
            .unwrap_or(Duration::from_secs(3600));
        select! {
            recv(stop_rx) -> _ => return,
            recv(raw_rx) -> msg => match msg {
                Ok(ev) => match parse_symbolic_link(&ev.link) {
                    Some(key) => {
                        tracing::trace!(target: "cc_hid::hotplug", link = %ev.link, action = ?ev.action, "raw notification");
                        pending.insert(key, Pending {
                            deadline: Instant::now() + DEBOUNCE,
                            action: ev.action,
                            link: ev.link,
                        });
                    }
                    None => tracing::trace!(target: "cc_hid::hotplug", link = %ev.link, "ignoring link without VID/PID"),
                },
                Err(_) => return,
            },
            default(timeout) => {}
        }
        let now = Instant::now();
        let due: Vec<(u16, u16)> = pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(k, _)| *k)
            .collect();
        for key in due {
            if let Some(p) = pending.remove(&key) {
                let event = HotplugEvent {
                    bus: Bus::Hid,
                    vid: key.0,
                    pid: key.1,
                    path: p.link.to_lowercase(),
                    action: p.action,
                };
                tracing::debug!(target: "cc_hid::hotplug", ?event, "hot-plug event");
                if tx.send(event).is_err() {
                    // Nobody listens any more; shut down.
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_links() {
        assert_eq!(
            parse_symbolic_link(
                r"\\?\HID#VID_1462&PID_7D75#9&c9e9596&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"
            ),
            Some((0x1462, 0x7D75))
        );
        assert_eq!(
            parse_symbolic_link(
                r"\\?\HID#VID_046D&PID_C539&MI_02&Col02#b&a5fc8a&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}"
            ),
            Some((0x046D, 0xC539))
        );
        assert_eq!(
            parse_symbolic_link(
                r"\\?\HID#VID_31E3&PID_1310&MI_02#b&3ab1eb27&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"
            ),
            Some((0x31E3, 0x1310))
        );
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            parse_symbolic_link(r"\\?\hid#vid_046d&pid_0a78#x"),
            Some((0x046D, 0x0A78))
        );
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "garbage",
            r"\\?\HID#VID_14&PID_7D75#x",
            r"\\?\HID#VID_ZZZZ&PID_7D75#x",
            r"\\?\HID#VID_1462&PID_7D#",
            r"\\?\HID#VID_1462#x",
            r"\\?\HID#PID_7D75#x",
            r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002046d_PID&b023#x",
            "VID_é462&PID_7D75",
        ] {
            assert_eq!(parse_symbolic_link(s), None, "{s}");
        }
    }

    #[test]
    fn watch_registers_and_stops() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let handle = watch(tx).expect("registration succeeds");
        std::thread::sleep(Duration::from_millis(100));
        handle.stop();
    }

    #[test]
    fn watcher_debounces_per_device() {
        let (raw_tx, raw_rx) = crossbeam_channel::unbounded();
        let (stop_tx, stop_rx) = crossbeam_channel::bounded(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || run_watcher(&raw_rx, &stop_rx, &tx));
        let link = |col: u8| format!(r"\\?\HID#VID_046D&PID_C539&MI_02&Col0{col}#b&0#{{x}}");
        for col in 1..=3 {
            raw_tx
                .send(RawEvent {
                    action: HotplugAction::Arrived,
                    link: link(col),
                })
                .expect("send");
        }
        raw_tx
            .send(RawEvent {
                action: HotplugAction::Removed,
                link: r"\\?\HID#VID_1462&PID_7D75#9#{x}".into(),
            })
            .expect("send");
        let mut got = [
            rx.recv_timeout(Duration::from_secs(3)).expect("first event"),
            rx.recv_timeout(Duration::from_secs(3)).expect("second event"),
        ];
        assert!(
            rx.recv_timeout(DEBOUNCE + Duration::from_millis(200)).is_err(),
            "one per burst"
        );
        got.sort_by_key(|e| e.vid);
        assert_eq!(
            (got[0].vid, got[0].pid, got[0].action),
            (0x046D, 0xC539, HotplugAction::Arrived)
        );
        assert_eq!(got[0].path, link(3).to_lowercase(), "last link of the burst");
        assert_eq!(got[0].path, r"\\?\hid#vid_046d&pid_c539&mi_02&col03#b&0#{x}");
        assert_eq!(got[1].path, r"\\?\hid#vid_1462&pid_7d75#9#{x}");
        assert_eq!(
            (got[1].vid, got[1].pid, got[1].action),
            (0x1462, 0x7D75, HotplugAction::Removed)
        );
        drop(stop_tx);
        t.join().expect("watcher exits");
    }
}
