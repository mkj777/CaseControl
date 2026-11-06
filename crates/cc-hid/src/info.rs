//! Enumeration results and device matching.

/// One enumerated HID interface (a physical device can expose several).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HidDeviceInfo {
    /// OS device path, used with [`open`](crate::open). Never derive a device id from it.
    pub path: String,
    /// USB vendor id.
    pub vid: u16,
    /// USB product id.
    pub pid: u16,
    /// USB interface number, `-1` when unknown.
    pub interface: i32,
    /// HID usage page of the top-level collection.
    pub usage_page: u16,
    /// HID usage of the top-level collection.
    pub usage: u16,
    /// Serial number string, if the device reports a non-empty one.
    pub serial: Option<String>,
    /// Product string, if non-empty.
    pub product: Option<String>,
    /// Manufacturer string, if non-empty.
    pub manufacturer: Option<String>,
    /// Device release number (bcdDevice).
    pub release: u16,
}

/// Matcher for HID interfaces; `None` fields are wildcards, `vid` is always compared.
///
/// The constructors mirror OpenRGB's detector macros: `REGISTER_HID_DETECTOR` ([`new`](Self::new)),
/// `_I` ([`new_i`](Self::new_i)), `_P` ([`new_p`](Self::new_p)), `_PU` ([`new_pu`](Self::new_pu)),
/// `_IP` ([`new_ip`](Self::new_ip)) and `_IPU` ([`new_ipu`](Self::new_ipu)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HidMatch {
    /// Vendor id (never a wildcard).
    pub vid: u16,
    /// Product id, or any.
    pub pid: Option<u16>,
    /// Interface number, or any.
    pub interface: Option<i32>,
    /// Usage page, or any.
    pub usage_page: Option<u16>,
    /// Usage, or any.
    pub usage: Option<u16>,
}

impl HidMatch {
    /// Any product of one vendor.
    pub const fn any_pid(vid: u16) -> Self {
        Self {
            vid,
            pid: None,
            interface: None,
            usage_page: None,
            usage: None,
        }
    }

    /// `REGISTER_HID_DETECTOR`: vid and pid, any interface and usage.
    pub const fn new(vid: u16, pid: u16) -> Self {
        Self {
            vid,
            pid: Some(pid),
            interface: None,
            usage_page: None,
            usage: None,
        }
    }

    /// `REGISTER_HID_DETECTOR_I`: vid, pid and interface.
    pub const fn new_i(vid: u16, pid: u16, interface: i32) -> Self {
        Self {
            vid,
            pid: Some(pid),
            interface: Some(interface),
            usage_page: None,
            usage: None,
        }
    }

    /// `REGISTER_HID_DETECTOR_P`: vid, pid and usage page.
    pub const fn new_p(vid: u16, pid: u16, usage_page: u16) -> Self {
        Self {
            vid,
            pid: Some(pid),
            interface: None,
            usage_page: Some(usage_page),
            usage: None,
        }
    }

    /// `REGISTER_HID_DETECTOR_PU`: vid, pid, usage page and usage.
    pub const fn new_pu(vid: u16, pid: u16, usage_page: u16, usage: u16) -> Self {
        Self {
            vid,
            pid: Some(pid),
            interface: None,
            usage_page: Some(usage_page),
            usage: Some(usage),
        }
    }

    /// `REGISTER_HID_DETECTOR_IP`: vid, pid, interface and usage page.
    pub const fn new_ip(vid: u16, pid: u16, interface: i32, usage_page: u16) -> Self {
        Self {
            vid,
            pid: Some(pid),
            interface: Some(interface),
            usage_page: Some(usage_page),
            usage: None,
        }
    }

    /// `REGISTER_HID_DETECTOR_IPU`: vid, pid, interface, usage page and usage.
    pub const fn new_ipu(vid: u16, pid: u16, interface: i32, usage_page: u16, usage: u16) -> Self {
        Self {
            vid,
            pid: Some(pid),
            interface: Some(interface),
            usage_page: Some(usage_page),
            usage: Some(usage),
        }
    }

    /// Whether `info` satisfies every non-wildcard field.
    pub fn matches(&self, info: &HidDeviceInfo) -> bool {
        self.vid == info.vid
            && self.pid.is_none_or(|p| p == info.pid)
            && self.interface.is_none_or(|i| i == info.interface)
            && self.usage_page.is_none_or(|p| p == info.usage_page)
            && self.usage.is_none_or(|u| u == info.usage)
    }

    /// All entries of `devices` this matcher accepts, in order.
    pub fn filter<'a>(
        &'a self,
        devices: &'a [HidDeviceInfo],
    ) -> impl Iterator<Item = &'a HidDeviceInfo> + 'a {
        devices.iter().filter(move |d| self.matches(d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(vid: u16, pid: u16, interface: i32, usage_page: u16, usage: u16) -> HidDeviceInfo {
        HidDeviceInfo {
            path: String::new(),
            vid,
            pid,
            interface,
            usage_page,
            usage,
            serial: None,
            product: None,
            manufacturer: None,
            release: 0,
        }
    }

    #[test]
    fn matches_all_wildcard_combinations() {
        let d = dev(0x31E3, 0x1310, 2, 0xFF55, 0x0001);
        // Every combination of the four optional fields being the matching value or a wildcard.
        for mask in 0u8..16 {
            let m = HidMatch {
                vid: 0x31E3,
                pid: (mask & 1 != 0).then_some(0x1310),
                interface: (mask & 2 != 0).then_some(2),
                usage_page: (mask & 4 != 0).then_some(0xFF55),
                usage: (mask & 8 != 0).then_some(0x0001),
            };
            assert!(m.matches(&d), "mask {mask:04b} should match");
            // Any present field with a wrong value must reject.
            if mask & 1 != 0 {
                assert!(
                    !HidMatch {
                        pid: Some(0x1311),
                        ..m
                    }
                    .matches(&d)
                );
            }
            if mask & 2 != 0 {
                assert!(
                    !HidMatch {
                        interface: Some(0),
                        ..m
                    }
                    .matches(&d)
                );
            }
            if mask & 4 != 0 {
                assert!(
                    !HidMatch {
                        usage_page: Some(0x0001),
                        ..m
                    }
                    .matches(&d)
                );
            }
            if mask & 8 != 0 {
                assert!(
                    !HidMatch {
                        usage: Some(0x0002),
                        ..m
                    }
                    .matches(&d)
                );
            }
            assert!(
                !HidMatch { vid: 0x046D, ..m }.matches(&d),
                "vid is never a wildcard"
            );
        }
    }

    #[test]
    fn constructors_mirror_openrgb_macros() {
        assert_eq!(
            HidMatch::new(1, 2),
            HidMatch {
                vid: 1,
                pid: Some(2),
                interface: None,
                usage_page: None,
                usage: None
            }
        );
        let i = HidMatch::new_i(1, 2, 3);
        assert_eq!((i.interface, i.usage_page, i.usage), (Some(3), None, None));
        let p = HidMatch::new_p(1, 2, 0xFF55);
        assert_eq!((p.interface, p.usage_page, p.usage), (None, Some(0xFF55), None));
        let pu = HidMatch::new_pu(1, 2, 0xFF43, 0x0202);
        assert_eq!(
            (pu.interface, pu.usage_page, pu.usage),
            (None, Some(0xFF43), Some(0x0202))
        );
        let ip = HidMatch::new_ip(1, 2, 3, 4);
        assert_eq!((ip.interface, ip.usage_page, ip.usage), (Some(3), Some(4), None));
        let ipu = HidMatch::new_ipu(1, 2, 3, 4, 5);
        assert_eq!(
            (ipu.interface, ipu.usage_page, ipu.usage),
            (Some(3), Some(4), Some(5))
        );
        assert_eq!(HidMatch::any_pid(7).pid, None);
        assert!(HidMatch::any_pid(0x1462).matches(&dev(0x1462, 0x7D75, -1, 1, 0)));
    }

    #[test]
    fn filter_picks_the_right_collection() {
        const MSI: HidMatch = HidMatch::new_pu(0x1462, 0x7D75, 0x0001, 0x00);
        let devices = [
            dev(0x1462, 0x7D75, -1, 0x0001, 0x00),
            dev(0x1462, 0x7D75, -1, 0xFF00, 0x01),
        ];
        let found: Vec<_> = MSI.filter(&devices).collect();
        assert_eq!(found, vec![&devices[0]]);
    }
}
