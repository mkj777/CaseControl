//! The [`Backend`] trait (device discovery) and hot-plug notifications.

use std::collections::HashSet;
use std::fmt;
use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};

use crate::controller::Controller;
use crate::device::Bus;
use crate::error::DeviceError;

/// Inputs to a detection pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectContext {
    /// Family ids (see [`crate::family`]) the user turned off. Their detectors must not run.
    pub disabled_families: HashSet<String>,
}

impl DetectContext {
    /// Whether detectors of `family` may run.
    pub fn is_enabled(&self, family: &str) -> bool {
        !self.disabled_families.contains(family)
    }
}

/// One result of a detection pass.
pub enum Detected {
    /// A device was found and a controller is ready for it.
    Device(Box<dyn Controller>),
    /// A device of `family` was found at `location` but could not be brought up.
    Failed {
        /// Family id of the detector.
        family: String,
        /// Where the device was found (e.g. HID path), for diagnostics.
        location: String,
        /// Why it failed.
        error: DeviceError,
    },
}

impl fmt::Debug for Detected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(c) => f.debug_tuple("Device").field(&c.description().info.id).finish(),
            Self::Failed {
                family,
                location,
                error,
            } => f
                .debug_struct("Failed")
                .field("family", family)
                .field("location", location)
                .field("error", error)
                .finish(),
        }
    }
}

/// Whether a device appeared or disappeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HotplugAction {
    /// A device was connected.
    Arrived,
    /// A device was disconnected.
    Removed,
}

/// A device was connected or disconnected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HotplugEvent {
    /// Transport.
    pub bus: Bus,
    /// Vendor id.
    pub vid: u16,
    /// Product id.
    pub pid: u16,
    /// OS device path; used to match the event to an open device, never as identity.
    pub path: String,
    /// Arrival or removal.
    pub action: HotplugAction,
}

/// Where backends send hot-plug events.
pub type HotplugSender = Sender<HotplugEvent>;

/// A running hot-plug subscription.
pub trait HotplugHandle: Send {
    /// Stops the subscription; no events are sent afterwards.
    fn stop(self: Box<Self>);
}

/// A way to find devices. HID backends match on vid/pid/interface/usage page/usage; NvAPI on PCI ids.
pub trait Backend: Send + Sync {
    /// Short name for logs.
    fn name(&self) -> &'static str;

    /// The transport this backend searches.
    fn bus(&self) -> Bus;

    /// Finds devices. Must skip families `ctx` disables and do no probing writes beyond what the reference
    /// implementation does for the same device.
    fn detect(&self, ctx: &DetectContext) -> Vec<Detected>;

    /// Starts sending hot-plug events to `tx`, or returns `None` if the backend has no hot-plug support.
    fn subscribe_hotplug(&self, tx: HotplugSender) -> Option<Box<dyn HotplugHandle>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::tests::FakeController;
    use crate::family;
    use std::sync::mpsc;

    struct FakeBackend;

    #[derive(Debug)]
    struct FakeHandle(HotplugSender);

    impl HotplugHandle for FakeHandle {
        fn stop(self: Box<Self>) {
            drop(self.0);
        }
    }

    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn bus(&self) -> Bus {
            Bus::Hid
        }

        fn detect(&self, ctx: &DetectContext) -> Vec<Detected> {
            let mut out = Vec::new();
            if ctx.is_enabled(family::MSI_MYSTIC_LIGHT) {
                out.push(Detected::Device(Box::new(FakeController::new())));
            }
            if ctx.is_enabled(family::WOOTING) {
                out.push(Detected::Failed {
                    family: family::WOOTING.into(),
                    location: "HID: somewhere".into(),
                    error: DeviceError::Timeout,
                });
            }
            out
        }

        fn subscribe_hotplug(&self, tx: HotplugSender) -> Option<Box<dyn HotplugHandle>> {
            tx.send(HotplugEvent {
                bus: Bus::Hid,
                vid: 0x31E3,
                pid: 0x1310,
                path: "test".into(),
                action: HotplugAction::Arrived,
            })
            .expect("receiver alive");
            Some(Box::new(FakeHandle(tx)))
        }
    }

    #[test]
    fn usable_as_trait_object() {
        let b: Box<dyn Backend> = Box::new(FakeBackend);
        assert_eq!(b.name(), "fake");
        assert_eq!(b.bus(), Bus::Hid);

        let found = b.detect(&DetectContext::default());
        assert_eq!(found.len(), 2);
        assert_eq!(format!("{:?}", found[0]), "Device(DeviceId(\"hid:1462:7d75\"))");
        assert!(format!("{:?}", found[1]).contains("Timeout"));

        let ctx = DetectContext {
            disabled_families: [family::WOOTING.to_string()].into(),
        };
        assert!(!ctx.is_enabled(family::WOOTING));
        assert!(ctx.is_enabled(family::LOGITECH));
        assert_eq!(b.detect(&ctx).len(), 1);

        let (tx, rx) = mpsc::channel();
        let handle = b.subscribe_hotplug(tx).expect("hotplug supported");
        let ev = rx.recv().expect("event");
        assert_eq!(ev.action, HotplugAction::Arrived);
        handle.stop();
        assert!(rx.recv().is_err(), "sender dropped after stop");
    }

    #[test]
    fn backend_is_send_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<Box<dyn Backend>>();
    }

    #[test]
    fn hotplug_event_json_round_trip() {
        let ev = HotplugEvent {
            bus: Bus::Hid,
            vid: 0x046D,
            pid: 0xC539,
            path: r"\\?\hid#vid_046d".into(),
            action: HotplugAction::Removed,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: HotplugEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ev);
    }
}
