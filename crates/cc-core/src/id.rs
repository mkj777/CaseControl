//! Stable device identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A device identifier that is stable across runs and USB ports.
///
/// Format: `"<bus>:<vid>:<pid>"`, plus `":<serial>"` when the device reports a serial, plus an ordinal `":#<n>"` only
/// when two identical serial-less devices exist (in enumeration order). Vendor and product ids are lower-case,
/// zero-padded 4-digit hex. An id is never derived from a device path: HID paths change with the USB port.
///
/// Serializes as a bare string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    /// Id of a HID device, e.g. `"hid:046d:c539"` or `"hid:31e3:1310:A1B2C3"`.
    ///
    /// A serial that is empty, whitespace-only, or consists only of `'0'` characters counts as no serial: HID stacks
    /// often report `""`, and some devices report a placeholder such as `"00000000"` (Logitech G560) that is not
    /// unique. A present serial is trimmed.
    pub fn hid(vid: u16, pid: u16, serial: Option<&str>) -> Self {
        let mut s = format!("hid:{vid:04x}:{pid:04x}");
        if let Some(serial) = serial
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.chars().all(|c| c == '0'))
        {
            s.push(':');
            s.push_str(serial);
        }
        Self(s)
    }

    /// Id of an NvAPI GPU from the raw words `NvAPI_GPU_GetPCIIdentifiers` returns.
    ///
    /// Both words carry the vendor in the low 16 bits and the device in the high 16 bits, so `device_id`
    /// `0x2C02_10DE` and `subsys_id` `0x2095_10DE` give `"nvapi:10de:2c02:10de:2095"` (vendor, device, subsystem
    /// vendor, subsystem device).
    pub fn nvapi(device_id: u32, subsys_id: u32) -> Self {
        let (vendor, device) = split_pci_word(device_id);
        let (sub_vendor, sub_device) = split_pci_word(subsys_id);
        Self(format!(
            "nvapi:{vendor:04x}:{device:04x}:{sub_vendor:04x}:{sub_device:04x}"
        ))
    }

    /// Appends the ordinal `":#<n>"` that disambiguates identical serial-less devices.
    pub fn with_ordinal(self, n: u32) -> Self {
        Self(format!("{}:#{n}", self.0))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn split_pci_word(word: u32) -> (u32, u32) {
    (word & 0xFFFF, word >> 16)
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_without_serial() {
        assert_eq!(DeviceId::hid(0x046D, 0x0A78, None).as_str(), "hid:046d:0a78");
        assert_eq!(DeviceId::hid(0x1, 0x2, None).as_str(), "hid:0001:0002");
    }

    #[test]
    fn hid_with_serial() {
        assert_eq!(
            DeviceId::hid(0x31E3, 0x1310, Some("A1B2c3")).as_str(),
            "hid:31e3:1310:A1B2c3"
        );
        assert_eq!(
            DeviceId::hid(0x31E3, 0x1310, Some("  X1 ")).as_str(),
            "hid:31e3:1310:X1"
        );
    }

    #[test]
    fn hid_empty_serial_counts_as_none() {
        assert_eq!(DeviceId::hid(0x1462, 0x7D75, Some("")).as_str(), "hid:1462:7d75");
        assert_eq!(
            DeviceId::hid(0x1462, 0x7D75, Some("   ")).as_str(),
            "hid:1462:7d75"
        );
    }

    #[test]
    fn hid_all_zero_serial_counts_as_none() {
        assert_eq!(
            DeviceId::hid(0x046D, 0x0A78, Some("00000000")).as_str(),
            "hid:046d:0a78"
        );
        assert_eq!(
            DeviceId::hid(0x046D, 0x0A78, Some(" 0 ")).as_str(),
            "hid:046d:0a78"
        );
        assert_eq!(
            DeviceId::hid(0x046D, 0x0A78, Some("00A0")).as_str(),
            "hid:046d:0a78:00A0"
        );
    }

    #[test]
    fn ordinal() {
        let id = DeviceId::hid(0x046D, 0xC539, None).with_ordinal(1);
        assert_eq!(id.as_str(), "hid:046d:c539:#1");
        assert_eq!(id.to_string(), "hid:046d:c539:#1");
    }

    #[test]
    fn nvapi() {
        let id = DeviceId::nvapi(0x2C02_10DE, 0x2095_10DE);
        assert_eq!(id.as_str(), "nvapi:10de:2c02:10de:2095");
        assert_eq!(
            DeviceId::nvapi(0x0001_0002, 0).as_str(),
            "nvapi:0002:0001:0000:0000"
        );
        assert_eq!(id.with_ordinal(0).as_str(), "nvapi:10de:2c02:10de:2095:#0");
    }

    #[test]
    fn serializes_as_bare_string() {
        let id = DeviceId::hid(0x046D, 0x0A78, None);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"hid:046d:0a78\"");
        let back: DeviceId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }
}
