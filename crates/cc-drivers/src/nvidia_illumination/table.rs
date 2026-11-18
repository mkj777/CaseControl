// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! The Illumination V1 detect table, ported from OpenRGB's `NVIDIAIlluminationControllerDetect_Windows_Linux.cpp`
//! (`device_list[]`) with the symbolic ids resolved from `reference/openrgb/pci_ids/pci_ids.h`.
//!
//! Every entry upstream is `NVIDIA_ILLUMINATION_V1`, so the version column is dropped. Only the RTX 5080 FE
//! (`10DE:2C02`, subsystem `10DE:2095`) has been run on this project's hardware; the other rows are kept because the
//! table is a plain list and the controller logic is identical for them.

use cc_nvapi::PciIds;

/// One GPU the reference drives through NvAPI Illumination V1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupportedGpu {
    /// PCI vendor id.
    pub vendor: u16,
    /// PCI device id.
    pub device: u16,
    /// PCI subsystem vendor id.
    pub subsystem_vendor: u16,
    /// PCI subsystem device id.
    pub subsystem_device: u16,
    /// Upstream `treats_rgbw_as_rgb`: the card ignores the white channel of RGBW zones, so colours are sent as
    /// plain RGB with white = 0.
    pub treats_rgbw_as_rgb: bool,
    /// Display name from the reference table.
    pub name: &'static str,
}

const NV: u16 = 0x10DE;
const PNY: u16 = 0x196E;
const PALIT: u16 = 0x1569;
const GAINWARD: u16 = 0x10B0;
/// Upstream `TREATS_RGBW_AS_RGB`.
const AS_RGB: bool = true;
/// Upstream `TREATS_RGBW_AS_RGBW`.
const AS_RGBW: bool = false;

const fn gpu(
    device: u16,
    subsystem_vendor: u16,
    subsystem_device: u16,
    treats_rgbw_as_rgb: bool,
    name: &'static str,
) -> SupportedGpu {
    SupportedGpu {
        vendor: NV,
        device,
        subsystem_vendor,
        subsystem_device,
        treats_rgbw_as_rgb,
        name,
    }
}

/// The reference's `device_list[]`, in upstream order.
pub const SUPPORTED: &[SupportedGpu] = &[
    gpu(0x1E89, NV, 0x1E89, AS_RGBW, "Palit GeForce RTX 2060"),
    gpu(0x1F08, NV, 0x12FB, AS_RGBW, "NVIDIA GeForce RTX 2060 FE"),
    gpu(0x1F06, NV, 0x13A2, AS_RGBW, "NVIDIA GeForce RTX 2060 SUPER FE"),
    gpu(0x1F07, NV, 0x12AD, AS_RGBW, "NVIDIA GeForce RTX 2070 FE"),
    gpu(0x1EC7, NV, 0x13AA, AS_RGBW, "NVIDIA GeForce RTX 2070 SUPER FE"),
    gpu(0x1E87, NV, 0x12A6, AS_RGBW, "NVIDIA GeForce RTX 2080 FE"),
    gpu(0x1E81, NV, 0x13A0, AS_RGBW, "NVIDIA GeForce RTX 2080 SUPER FE"),
    gpu(0x1E07, NV, 0x12A4, AS_RGBW, "NVIDIA GeForce RTX 2080 Ti FE"),
    gpu(
        0x1E07,
        NV,
        0x1435,
        AS_RGBW,
        "NVIDIA GeForce RTX 2080 Ti Cyberpunk 2077",
    ),
    gpu(0x1E02, NV, 0x12A3, AS_RGBW, "NVIDIA TITAN RTX"),
    gpu(0x2507, NV, 0x2507, AS_RGB, "Gainward GeForce RTX 3050 LHR"),
    gpu(0x2544, NV, 0x2544, AS_RGB, "Gainward GeForce RTX 3060 Pegasus"),
    gpu(0x2504, NV, 0x2504, AS_RGB, "Palit GeForce RTX 3060 LHR"),
    gpu(
        0x2504,
        PNY,
        0x138E,
        AS_RGB,
        "PNY GeForce RTX 3060 XLR8 REVEL EPIC-X",
    ),
    gpu(
        0x2504,
        PNY,
        0x138F,
        AS_RGB,
        "PNY GeForce RTX 3060 XLR8 REVEL EPIC-X LHR",
    ),
    gpu(
        0x2487,
        PNY,
        0x138E,
        AS_RGB,
        "PNY GeForce RTX 3060 XLR8 REVEL EPIC-X",
    ),
    gpu(0x2487, NV, 0x2487, AS_RGB, "Palit GeForce RTX 3060 LHR"),
    gpu(
        0x2489,
        PNY,
        0x1389,
        AS_RGB,
        "PNY GeForce RTX 3060 Ti XLR8 REVEL EPIC-X",
    ),
    gpu(
        0x2489,
        NV,
        0x1389,
        AS_RGB,
        "PNY GeForce RTX 3060 Ti XLR8 REVEL EPIC-X",
    ),
    gpu(0x2414, NV, 0x2414, AS_RGB, "NVIDIA GeForce RTX 3060 Ti V1 LHR"),
    gpu(0x2489, NV, 0x2489, AS_RGB, "NVIDIA GeForce RTX 3060 Ti LHR"),
    gpu(
        0x2482,
        PNY,
        0x138D,
        AS_RGBW,
        "PNY GeForce RTX 3070 Ti XLR8 Uprising EPIC-X",
    ),
    gpu(0x2206, NV, 0x1467, AS_RGBW, "NVIDIA GeForce RTX 3080 FE"),
    gpu(0x2208, NV, 0x1535, AS_RGBW, "NVIDIA GeForce RTX 3080 Ti FE"),
    gpu(0x2208, NV, 0x2612, AS_RGB, "MANLI GeForce RTX 3080 Ti GALLARDO"),
    gpu(0x2204, NV, 0x147D, AS_RGBW, "NVIDIA GeForce RTX 3090 FE"),
    gpu(0x2203, NV, 0x1618, AS_RGBW, "NVIDIA GeForce RTX 3090 Ti FE"),
    gpu(0x2882, PALIT, 0xF311, AS_RGB, "Palit GeForce RTX 4060 Dual"),
    gpu(0x2786, PALIT, 0xF302, AS_RGB, "Palit GeForce RTX 4070"),
    gpu(0x2783, PALIT, 0xF302, AS_RGB, "Palit GeForce RTX 4070 SUPER Dual"),
    gpu(
        0x2786,
        GAINWARD,
        0xF303,
        AS_RGBW,
        "Gainward GeForce RTX 4070 Ghost",
    ),
    gpu(
        0x2783,
        GAINWARD,
        0xF303,
        AS_RGBW,
        "Gainward GeForce RTX 4070 SUPER Ghost",
    ),
    gpu(0x2704, NV, 0x167A, AS_RGBW, "NVIDIA GeForce RTX 4080 FE"),
    gpu(0x2704, NV, 0x1794, AS_RGBW, "NVIDIA GeForce RTX 4080 FE"),
    gpu(0x2702, NV, 0x18CB, AS_RGBW, "NVIDIA GeForce RTX 4080 SUPER FE"),
    gpu(0x2684, NV, 0x165B, AS_RGBW, "NVIDIA GeForce RTX 4090 FE"),
    gpu(0x2684, NV, 0x16F4, AS_RGBW, "NVIDIA GeForce RTX 4090 FE"),
    gpu(0x2C02, NV, 0x2095, AS_RGBW, "NVIDIA GeForce RTX 5080 FE"),
    gpu(0x2B85, NV, 0x2057, AS_RGBW, "NVIDIA GeForce RTX 5090 FE"),
];

/// The table entry for a GPU's PCI ids, if the reference supports it.
///
/// Upstream loops over the whole table without `break` and would create one controller per matching row; no two rows
/// share all four ids, so the first match is the only one.
#[must_use]
pub fn lookup(ids: &PciIds) -> Option<&'static SupportedGpu> {
    SUPPORTED.iter().find(|g| {
        g.vendor == ids.vendor()
            && g.device == ids.device()
            && g.subsystem_vendor == ids.subsystem_vendor()
            && g.subsystem_device == ids.subsystem_device()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ids(device_id: u32, subsystem_id: u32) -> PciIds {
        PciIds {
            device_id,
            subsystem_id,
            revision_id: 0xA1,
            ext_device_id: 0,
        }
    }

    #[test]
    fn rtx_5080_fe_matches_as_rgbw() {
        // dump_illum on this machine: raw device_id 0x2C0210DE, subsystem_id 0x209510DE.
        let g = lookup(&ids(0x2C02_10DE, 0x2095_10DE)).expect("5080 FE is in the table");
        assert_eq!(g.name, "NVIDIA GeForce RTX 5080 FE");
        assert!(!g.treats_rgbw_as_rgb, "upstream: TREATS_RGBW_AS_RGBW");
    }

    #[test]
    fn subsystem_must_match() {
        // A partner 5080 (different subsystem) is not an Illumination V1 card.
        assert_eq!(lookup(&ids(0x2C02_10DE, 0x1234_1043)), None);
        // Vendor swapped into the wrong half does not match either.
        assert_eq!(lookup(&ids(0x10DE_2C02, 0x10DE_2095)), None);
    }

    #[test]
    fn table_has_no_duplicate_rows_and_all_v1_rows() {
        assert_eq!(SUPPORTED.len(), 39, "upstream device_list[] has 39 rows");
        let keys: HashSet<_> = SUPPORTED
            .iter()
            .map(|g| (g.vendor, g.device, g.subsystem_vendor, g.subsystem_device))
            .collect();
        assert_eq!(keys.len(), SUPPORTED.len());
        assert!(SUPPORTED.iter().all(|g| g.vendor == 0x10DE));
    }
}
