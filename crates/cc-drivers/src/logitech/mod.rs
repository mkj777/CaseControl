//! Logitech HID++ lighting: the G560 speaker (HID++ 2.0, feature 0x8070, wired) and Lightspeed wireless devices
//! (PRO Wireless) behind the Lightspeed receiver `046D:C539`.
//!
//! Ported from `reference/openrgb/LogitechController/` (`LogitechControllerDetect.cpp`, `LogitechProtocolCommon.*`,
//! `LogitechHIDPP20Controller/*` for the G560, `LogitechLightspeedController/*` for the receiver path). See the
//! submodules: [`hidpp`] (framing, reply matching, shared feature queries), [`g560`], [`lightspeed`].
//!
//! Detection, per the reference:
//! - G560: every `046D:0A78` collection on usage page 0xFF43 (the reference registers `0xFF43`, any usage), grouped
//!   into physical devices by path (`LogitechDevicePathKey`); collections of one device are tried in order until one
//!   answers the HID++ probe. Id `hid:046d:0a78` (its `00000000` serial counts as absent), ordinals for duplicates.
//! - Lightspeed: the receiver's short-report collection (interface 2, page 0xFF00, usage 1) plus its long-report
//!   sibling (usage 2) with the same path key (`BundleLogitechUsages`). The receiver is asked what is paired; each
//!   paired device with a wireless PID the legacy controller owns gets id `hid:046d:<wpid>` and is probed if it is
//!   connected. A device that is switched off yields nothing, not an error. The backend remembers which ids each
//!   receiver produced and skips a receiver without any I/O when all of them are already open (`ctx.skip`).
//!
//! What this module deliberately does not do: no DPI, buttons, onboard profiles or pairing changes; no feature
//! 0x8071 / 0x8081 / per-key paths (not needed by the two v1 devices); no threads, no power management, no
//! keepalive (neither device needs one on 0x8070). Writes to flash happen only in `save_to_hardware()` (G560).

pub mod g560;
pub mod hidpp;
pub mod lightspeed;

use std::collections::HashMap;
use std::sync::Mutex;

use cc_core::{
    Backend, Bus, DetectContext, Detected, DeviceError, DeviceId, HotplugHandle, HotplugSender, family,
};
use cc_hid::{HidDeviceInfo, HidError, HidTransport};

pub use g560::G560Controller;
pub use lightspeed::LightspeedController;

use hidpp::{DEFAULT_DEVICE_INDEX, LOGITECH_VID, Link};

/// The backend for this family.
pub fn backend() -> Box<dyn Backend> {
    Box::new(LogitechBackend::new())
}

/// One detection result with the concrete controller type (for tools and examples that want driver-specific state).
#[derive(Debug)]
pub enum Found {
    /// A G560 speaker.
    G560(G560Controller),
    /// A device behind a Lightspeed receiver.
    Lightspeed(LightspeedController),
    /// A matching device that could not be set up.
    Failed {
        /// HID path of the device.
        location: String,
        /// What went wrong.
        error: DeviceError,
    },
}

impl From<Found> for Detected {
    fn from(f: Found) -> Self {
        match f {
            Found::G560(c) => Detected::Device(Box::new(c)),
            Found::Lightspeed(c) => Detected::Device(Box::new(c)),
            Found::Failed { location, error } => Detected::Failed {
                family: family::LOGITECH.into(),
                location,
                error,
            },
        }
    }
}

/// Opens a HID path; [`cc_hid::open`] in production, a fake in tests.
pub type Opener<'a> = dyn FnMut(&str) -> Result<Box<dyn HidTransport>, HidError> + 'a;

/// The Logitech backend. Holds, per receiver (path key), the ids the last detection produced there.
#[derive(Debug, Default)]
pub struct LogitechBackend {
    receivers: Mutex<HashMap<String, Vec<DeviceId>>>,
}

/// `LogitechDevicePathKey`, lower-cased: strips the `&ColNN` collection token and the per-collection instance
/// suffix so all collections of one physical interface share a key. Paths without a collection token key as
/// themselves.
pub fn path_key(path: &str) -> String {
    let mut key = path.to_ascii_lowercase();
    let Some(pos) = key.find("&col") else {
        return key;
    };
    let end = key[pos + 4..]
        .find(|c: char| !c.is_ascii_hexdigit())
        .map_or(key.len(), |e| pos + 4 + e);
    key.replace_range(pos..end, "");
    if let Some(guid) = key.rfind('#')
        && let Some(last) = key[..guid].rfind('&')
    {
        key.replace_range(last..guid, "");
    }
    key
}

/// A serial that counts as present (same rule as [`DeviceId::hid`]).
fn present_serial(serial: Option<&str>) -> Option<String> {
    serial
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.chars().all(|c| c == '0'))
        .map(str::to_owned)
}

/// Assigns ids to `bases` in order, appending ordinals where a base occurs more than once.
fn with_ordinals(bases: Vec<DeviceId>) -> Vec<DeviceId> {
    bases
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if bases.iter().filter(|x| *x == b).count() > 1 {
                let ord = bases[..i].iter().filter(|x| *x == b).count();
                b.clone().with_ordinal(ord as u32)
            } else {
                b.clone()
            }
        })
        .collect()
}

impl LogitechBackend {
    /// A backend with no receiver history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Detection with concrete controller types; [`Backend::detect`] maps this into [`Detected`].
    pub fn detect_typed(&self, ctx: &DetectContext) -> Vec<Found> {
        if !ctx.is_enabled(family::LOGITECH) {
            return Vec::new();
        }
        let devices = match cc_hid::enumerate() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(target: "cc_drivers::logitech", error = %e, "HID enumeration failed");
                return Vec::new();
            }
        };
        self.detect_in(&devices, ctx, &mut |p| cc_hid::open(p))
    }

    /// Detection over an enumeration result with an injectable opener. The family toggle is the caller's business.
    pub fn detect_in(
        &self,
        devices: &[HidDeviceInfo],
        ctx: &DetectContext,
        open: &mut Opener<'_>,
    ) -> Vec<Found> {
        let mut out = Vec::new();
        detect_g560(devices, ctx, open, &mut out);
        self.detect_lightspeed(devices, ctx, open, &mut out);
        out
    }

    fn detect_lightspeed(
        &self,
        devices: &[HidDeviceInfo],
        ctx: &DetectContext,
        open: &mut Opener<'_>,
        out: &mut Vec<Found>,
    ) {
        use lightspeed::*;

        struct Scanned {
            key: String,
            short: Box<dyn HidTransport>,
            long_path: String,
            candidates: Vec<Paired>,
        }

        let is_rx = |d: &HidDeviceInfo, usage: u16| {
            d.vid == LOGITECH_VID
                && d.pid == RECEIVER_PID
                && d.interface == RECEIVER_INTERFACE
                && d.usage_page == RECEIVER_USAGE_PAGE
                && d.usage == usage
        };

        // Phase 1: find receivers, skip the ones whose devices are all open, scan the rest.
        let mut scanned: Vec<Scanned> = Vec::new();
        for short_info in devices.iter().filter(|d| is_rx(d, USAGE_SHORT)) {
            let key = path_key(&short_info.path);
            let Some(long_info) = devices
                .iter()
                .find(|d| is_rx(d, USAGE_LONG) && path_key(&d.path) == key)
            else {
                out.push(Found::Failed {
                    location: short_info.path.clone(),
                    error: DeviceError::Protocol(
                        "receiver long-report collection (usage 2) not found".into(),
                    ),
                });
                continue;
            };
            let all_open = self
                .receivers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&key)
                .is_some_and(|ids| !ids.is_empty() && ids.iter().all(|id| ctx.is_skipped(id)));
            if all_open {
                tracing::debug!(target: "cc_drivers::logitech", key, "receiver devices already open, skipped");
                continue;
            }
            let mut short = match open(&short_info.path) {
                Ok(t) => t,
                Err(e) => {
                    out.push(Found::Failed {
                        location: short_info.path.clone(),
                        error: e.into(),
                    });
                    continue;
                }
            };
            let paired = match scan_receiver(short.as_mut()) {
                Ok(p) => p,
                Err(error) => {
                    out.push(Found::Failed {
                        location: short_info.path.clone(),
                        error,
                    });
                    continue;
                }
            };
            let candidates = paired
                .into_iter()
                .filter(|p| {
                    let ours = legacy_name(p.wpid).is_some();
                    if !ours {
                        tracing::info!(target: "cc_drivers::logitech", wpid = format_args!("{:04X}", p.wpid), "paired device not handled by this driver, left alone");
                    }
                    ours
                })
                .collect();
            scanned.push(Scanned {
                key,
                short,
                long_path: long_info.path.clone(),
                candidates,
            });
        }

        // Phase 2: ids from the wireless PIDs, with ordinals for duplicates across receivers scanned in this pass.
        let bases = scanned
            .iter()
            .flat_map(|s| {
                s.candidates
                    .iter()
                    .map(|p| DeviceId::hid(LOGITECH_VID, p.wpid, None))
            })
            .collect();
        let mut ids = with_ordinals(bases).into_iter();

        // Phase 3: probe connected devices.
        for mut rx in scanned {
            let mut produced = Vec::new();
            for p in &rx.candidates {
                let Some(id) = ids.next() else { break };
                if ctx.is_skipped(&id) {
                    produced.push(id);
                    continue;
                }
                let connected = match is_connected(rx.short.as_mut(), p.index) {
                    Ok(Some(c)) => c,
                    Ok(None) => p.online,
                    Err(error) => {
                        out.push(Found::Failed {
                            location: rx.long_path.clone(),
                            error,
                        });
                        continue;
                    }
                };
                if !connected {
                    tracing::info!(target: "cc_drivers::logitech", %id, index = p.index, "paired device not connected");
                    continue;
                }
                let long = match open(&rx.long_path) {
                    Ok(t) => t,
                    Err(e) => {
                        out.push(Found::Failed {
                            location: rx.long_path.clone(),
                            error: e.into(),
                        });
                        continue;
                    }
                };
                let mut link = Link::new(long, p.index);
                match probe_with_retry(rx.short.as_mut(), &mut link) {
                    Ok(Some(probe)) => {
                        let desc = describe(&probe, p.wpid, id.clone(), rx.long_path.clone());
                        if let Err(e) = desc.validate() {
                            out.push(Found::Failed {
                                location: rx.long_path.clone(),
                                error: DeviceError::Protocol(e.to_string()),
                            });
                            continue;
                        }
                        produced.push(id);
                        out.push(Found::Lightspeed(LightspeedController::new(link, probe, desc)));
                    }
                    Ok(None) => out.push(Found::Failed {
                        location: rx.long_path.clone(),
                        error: DeviceError::Protocol("device never answered with a valid identity".into()),
                    }),
                    Err(error) => out.push(Found::Failed {
                        location: rx.long_path.clone(),
                        error,
                    }),
                }
            }
            self.receivers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(rx.key, produced);
        }
    }
}

fn detect_g560(devices: &[HidDeviceInfo], ctx: &DetectContext, open: &mut Opener<'_>, out: &mut Vec<Found>) {
    use g560::*;

    let mut groups: Vec<(String, Vec<&HidDeviceInfo>)> = Vec::new();
    for d in devices
        .iter()
        .filter(|d| d.vid == LOGITECH_VID && d.pid == G560_PID && d.usage_page == G560_USAGE_PAGE)
    {
        let key = path_key(&d.path);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(d),
            None => groups.push((key, vec![d])),
        }
    }
    let bases = groups
        .iter()
        .map(|(_, m)| DeviceId::hid(LOGITECH_VID, G560_PID, m[0].serial.as_deref()))
        .collect();
    for ((_, members), id) in groups.iter().zip(with_ordinals(bases)) {
        if ctx.is_skipped(&id) {
            continue;
        }
        let mut last_error = DeviceError::Unsupported;
        let mut found = false;
        for m in members {
            let transport = match open(&m.path) {
                Ok(t) => t,
                Err(e) => {
                    last_error = e.into();
                    continue;
                }
            };
            let mut link = Link::new(transport, DEFAULT_DEVICE_INDEX);
            match probe(&mut link) {
                Ok(p) => {
                    let desc = describe(
                        &p,
                        id.clone(),
                        m.path.clone(),
                        present_serial(m.serial.as_deref()),
                    );
                    if let Err(e) = desc.validate() {
                        last_error = DeviceError::Protocol(e.to_string());
                        continue;
                    }
                    out.push(Found::G560(G560Controller::new(link, p, desc)));
                    found = true;
                    break;
                }
                Err(e) => {
                    tracing::debug!(target: "cc_drivers::logitech", path = %m.path, error = %e, "G560 collection did not answer the HID++ probe");
                    last_error = e;
                }
            }
        }
        if !found {
            out.push(Found::Failed {
                location: members[0].path.clone(),
                error: last_error,
            });
        }
    }
}

impl Backend for LogitechBackend {
    fn name(&self) -> &'static str {
        family::LOGITECH
    }

    fn bus(&self) -> Bus {
        Bus::Hid
    }

    fn detect(&self, ctx: &DetectContext) -> Vec<Detected> {
        self.detect_typed(ctx).into_iter().map(Detected::from).collect()
    }

    fn subscribe_hotplug(&self, _tx: HotplugSender) -> Option<Box<dyn HotplugHandle>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_core::Controller;
    use cc_hid::{FakeTransport, ScriptBuilder};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    fn info(
        path: &str,
        pid: u16,
        interface: i32,
        usage_page: u16,
        usage: u16,
        serial: Option<&str>,
    ) -> HidDeviceInfo {
        HidDeviceInfo {
            path: path.into(),
            vid: LOGITECH_VID,
            pid,
            interface,
            usage_page,
            usage,
            serial: serial.map(Into::into),
            product: None,
            manufacturer: None,
            release: 0,
        }
    }

    const RX_SHORT: &str =
        r"\\?\HID#VID_046D&PID_C539&MI_02&Col01#8&b41415e&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
    const RX_LONG: &str =
        r"\\?\HID#VID_046D&PID_C539&MI_02&Col02#8&b41415e&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
    const G560_PATH: &str =
        r"\\?\HID#VID_046D&PID_0A78&MI_02#7&1a2b3c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";

    #[test]
    fn path_key_merges_collections() {
        assert_eq!(path_key(RX_SHORT), path_key(RX_LONG));
        assert_eq!(
            path_key(RX_SHORT),
            r"\\?\hid#vid_046d&pid_c539&mi_02#8&b41415e&0#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        );
        // No collection token: the path keys as itself (lower-cased).
        assert_eq!(path_key(G560_PATH), G560_PATH.to_ascii_lowercase());
        // A second receiver with another instance id keys apart.
        let other = RX_SHORT.replace("b41415e", "c51526f");
        assert_ne!(path_key(&other), path_key(RX_SHORT));
    }

    #[test]
    fn ordinals_only_for_duplicates() {
        let a = DeviceId::hid(LOGITECH_VID, 0x0A78, None);
        let b = DeviceId::hid(LOGITECH_VID, 0x4079, None);
        let ids = with_ordinals(vec![a.clone(), b.clone(), a.clone()]);
        assert_eq!(ids[0].as_str(), "hid:046d:0a78:#0");
        assert_eq!(ids[1].as_str(), "hid:046d:4079");
        assert_eq!(ids[2].as_str(), "hid:046d:0a78:#1");
    }

    /// An opener that hands out scripted fakes per path and records every open.
    struct Fakes {
        scripts: RefCell<HashMap<String, VecDeque<FakeTransport>>>,
        opened: RefCell<Vec<String>>,
    }

    impl Fakes {
        fn new(entries: Vec<(&str, FakeTransport)>) -> Self {
            let mut scripts: HashMap<String, VecDeque<FakeTransport>> = HashMap::new();
            for (p, f) in entries {
                scripts.entry(p.into()).or_default().push_back(f);
            }
            Self {
                scripts: RefCell::new(scripts),
                opened: RefCell::new(Vec::new()),
            }
        }

        fn open(&self, path: &str) -> Result<Box<dyn HidTransport>, HidError> {
            self.opened.borrow_mut().push(path.into());
            self.scripts
                .borrow_mut()
                .get_mut(path)
                .and_then(VecDeque::pop_front)
                .map(|f| Box::new(f) as Box<dyn HidTransport>)
                .ok_or_else(|| HidError::NotFound(path.into()))
        }
    }

    fn g560_devices() -> Vec<HidDeviceInfo> {
        vec![info(G560_PATH, 0x0A78, 2, 0xFF43, 0x0202, Some("00000000"))]
    }

    fn rx_devices() -> Vec<HidDeviceInfo> {
        vec![
            info(RX_SHORT, 0xC539, 2, 0xFF00, 1, None),
            info(RX_LONG, 0xC539, 2, 0xFF00, 2, None),
            // Other collections of the receiver and unrelated devices are ignored.
            info(r"\\?\HID#VID_046D&PID_C539&MI_00#x", 0xC539, 0, 0x0001, 6, None),
            info(r"\\?\HID#VID_31E3&PID_1310#y", 0x1310, 0, 0xFF55, 1, None),
        ]
    }

    #[test]
    fn g560_detection_and_skip() {
        let backend = LogitechBackend::new();
        let fake = FakeTransport::new(g560::tests::probe_script().into_steps());
        let fakes = Fakes::new(vec![(G560_PATH, fake.clone())]);
        let found = backend.detect_in(&g560_devices(), &DetectContext::default(), &mut |p| fakes.open(p));
        assert_eq!(found.len(), 1);
        let Found::G560(c) = &found[0] else {
            panic!("expected a G560, got {found:?}");
        };
        assert_eq!(c.description().info.id.as_str(), "hid:046d:0a78");
        assert_eq!(c.description().info.location, G560_PATH);
        assert_eq!(c.description().info.serial, None);
        fake.finish().expect("script consumed");

        // Same device already open: no open, no I/O.
        let ctx = DetectContext {
            skip: [DeviceId::hid(LOGITECH_VID, 0x0A78, None)].into(),
            ..Default::default()
        };
        let none = Fakes::new(vec![]);
        assert!(
            backend
                .detect_in(&g560_devices(), &ctx, &mut |p| none.open(p))
                .is_empty()
        );
        assert!(none.opened.borrow().is_empty());
    }

    #[test]
    fn g560_that_never_answers_is_reported() {
        let fake = FakeTransport::builder()
            .read_timeout()
            .write(hidpp::feature_lookup_request(0xFF, 0x0001).to_vec())
            .read_timeout()
            .build();
        let fakes = Fakes::new(vec![(G560_PATH, fake.clone())]);
        let found = LogitechBackend::new()
            .detect_in(&g560_devices(), &DetectContext::default(), &mut |p| fakes.open(p));
        assert!(matches!(
            &found[..],
            [Found::Failed {
                error: DeviceError::Timeout,
                ..
            }]
        ));
        fake.finish().expect("script consumed");
    }

    fn receiver_fakes() -> (FakeTransport, FakeTransport) {
        use lightspeed::tests::*;
        // Short collection: scan, connected check, then the probe's flush.
        let short = connected_script(scan_script(FakeTransport::builder()))
            .read_timeout()
            .build();
        // Long collection: the probe's flush, then the device conversation.
        let long = device_script(ScriptBuilder::new().read_timeout()).build();
        (short, long)
    }

    #[test]
    fn lightspeed_detection_then_skip_without_io() {
        let backend = LogitechBackend::new();
        let (short, long) = receiver_fakes();
        let fakes = Fakes::new(vec![(RX_SHORT, short.clone()), (RX_LONG, long.clone())]);
        let found = backend.detect_in(&rx_devices(), &DetectContext::default(), &mut |p| fakes.open(p));
        assert_eq!(found.len(), 1, "{found:?}");
        let Found::Lightspeed(c) = &found[0] else {
            panic!("expected a Lightspeed device, got {found:?}");
        };
        let id = c.description().info.id.clone();
        assert_eq!(id.as_str(), "hid:046d:4079");
        assert_eq!(c.description().info.location, RX_LONG);
        assert_eq!(c.device_index(), 1);
        short.finish().expect("short script consumed");
        long.finish().expect("long script consumed");

        // The engine now has the mouse open: the receiver is skipped before any I/O.
        let ctx = DetectContext {
            skip: [id].into(),
            ..Default::default()
        };
        let none = Fakes::new(vec![]);
        assert!(
            backend
                .detect_in(&rx_devices(), &ctx, &mut |p| none.open(p))
                .is_empty()
        );
        assert!(none.opened.borrow().is_empty());

        // Nothing open (e.g. the mouse was closed): the receiver is talked to again.
        let none = Fakes::new(vec![]);
        let found = backend.detect_in(&rx_devices(), &DetectContext::default(), &mut |p| none.open(p));
        assert_eq!(none.opened.borrow().as_slice(), [RX_SHORT]);
        assert!(matches!(&found[..], [Found::Failed { .. }]));
    }

    #[test]
    fn switched_off_mouse_yields_nothing() {
        use lightspeed::tests::*;
        let short = scan_script(FakeTransport::builder())
            .write(hidpp::short_register(IDX, 0x81, 0x02, [0; 3]).to_vec())
            .read(vec![0x10, IDX, 0x8F, 0x81, 0x02, 0x09, 0x00])
            .build();
        let fakes = Fakes::new(vec![(RX_SHORT, short.clone())]);
        let backend = LogitechBackend::new();
        let found = backend.detect_in(&rx_devices(), &DetectContext::default(), &mut |p| fakes.open(p));
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(fakes.opened.borrow().as_slice(), [RX_SHORT]);
        short.finish().expect("script consumed");
        // Nothing was produced, so the next pass talks to the receiver again even with a non-empty skip set.
        let ctx = DetectContext {
            skip: [DeviceId::hid(LOGITECH_VID, 0x4079, None)].into(),
            ..Default::default()
        };
        let none = Fakes::new(vec![]);
        let _ = backend.detect_in(&rx_devices(), &ctx, &mut |p| none.open(p));
        assert_eq!(none.opened.borrow().as_slice(), [RX_SHORT]);
    }

    #[test]
    fn disabled_family_detects_nothing() {
        let ctx = DetectContext {
            disabled_families: [family::LOGITECH.to_string()].into(),
            ..Default::default()
        };
        assert!(backend().detect(&ctx).is_empty());
        assert!(
            backend()
                .subscribe_hotplug(std::sync::mpsc::channel().0)
                .is_none()
        );
        assert_eq!(backend().name(), family::LOGITECH);
    }

    #[cfg(windows)]
    #[test]
    fn enumeration_smoke() {
        // Enumerates only; opens nothing.
        assert!(cc_hid::enumerate().is_ok());
    }
}
