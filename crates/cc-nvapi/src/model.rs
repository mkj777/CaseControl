//! Safe, owned representations of the NvAPI data this crate exchanges with the driver, and their
//! byte-faithful conversions to and from the [`raw`](crate::raw) structs.
//!
//! Everything here is pure data and compiles on every platform, so the conversions are unit-testable
//! without a GPU or driver.

use crate::error::NvApiError;
use crate::raw;

/// PCI identifiers of a GPU as returned by `NvAPI_GPU_GetPCIIdentifiers`.
///
/// NvAPI packs two 16-bit PCI ids into each `u32`: the *device* (or subsystem device) id in the high
/// 16 bits and the *vendor* (or subsystem vendor) id in the low 16 bits. The header itself is silent on
/// this; the split is taken from OpenRGB's `NVIDIAIlluminationControllerDetect`
/// (`pci_device = device_id >> 16; pci_vendor = device_id & 0xffff`, same for `sub_system_id`).
/// For the RTX 5080 FE that means `device_id == 0x2C02_10DE` and `subsystem_id == 0x2095_10DE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PciIds {
    /// Packed `device << 16 | vendor`.
    pub device_id: u32,
    /// Packed `subsystem_device << 16 | subsystem_vendor`.
    pub subsystem_id: u32,
    /// PCI revision id.
    pub revision_id: u32,
    /// Extended device id (unpacked, as returned).
    pub ext_device_id: u32,
}

impl PciIds {
    /// PCI vendor id (`0x10DE` for NVIDIA).
    #[must_use]
    pub const fn vendor(&self) -> u16 {
        low16(self.device_id)
    }

    /// PCI device id (e.g. `0x2C02`).
    #[must_use]
    pub const fn device(&self) -> u16 {
        high16(self.device_id)
    }

    /// PCI subsystem vendor id.
    #[must_use]
    pub const fn subsystem_vendor(&self) -> u16 {
        low16(self.subsystem_id)
    }

    /// PCI subsystem device id (e.g. `0x2095`).
    #[must_use]
    pub const fn subsystem_device(&self) -> u16 {
        high16(self.subsystem_id)
    }
}

const fn low16(v: u32) -> u16 {
    (v & 0xFFFF) as u16
}

const fn high16(v: u32) -> u16 {
    (v >> 16) as u16
}

/// Opaque handle of a physical GPU (`NV_PHYSICAL_GPU_HANDLE`).
///
/// It is a token handed out by [`NvApi::physical_gpus`](crate::NvApi::physical_gpus) and is only
/// meaningful to the `NvApi` instance that produced it; NvAPI validates handles and rejects stale ones
/// with a status error rather than dereferencing them in our process. Stored as an integer so the token
/// is `Send + Sync`; this crate never dereferences it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GpuHandle(pub(crate) usize);

impl GpuHandle {
    /// The raw handle value, for logging.
    #[must_use]
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

impl core::fmt::Debug for GpuHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "GpuHandle({:#x})", self.0)
    }
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_TYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZoneKind {
    /// `..._INVALID` (0).
    Invalid,
    /// `..._RGB` (1).
    Rgb,
    /// `..._COLOR_FIXED` (2): fixed color, only brightness is controllable.
    ColorFixed,
    /// `..._RGBW` (3).
    Rgbw,
    /// `..._SINGLE_COLOR` (4): single color, only brightness is controllable.
    SingleColor,
    /// A value the header does not define; kept verbatim.
    Other(u32),
}

impl ZoneKind {
    /// Decodes the raw `type` field.
    #[must_use]
    pub const fn from_raw(v: u32) -> Self {
        match v {
            raw::ZONE_TYPE_INVALID => Self::Invalid,
            raw::ZONE_TYPE_RGB => Self::Rgb,
            raw::ZONE_TYPE_COLOR_FIXED => Self::ColorFixed,
            raw::ZONE_TYPE_RGBW => Self::Rgbw,
            raw::ZONE_TYPE_SINGLE_COLOR => Self::SingleColor,
            other => Self::Other(other),
        }
    }

    /// Encodes to the raw `type` field.
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        match self {
            Self::Invalid => raw::ZONE_TYPE_INVALID,
            Self::Rgb => raw::ZONE_TYPE_RGB,
            Self::ColorFixed => raw::ZONE_TYPE_COLOR_FIXED,
            Self::Rgbw => raw::ZONE_TYPE_RGBW,
            Self::SingleColor => raw::ZONE_TYPE_SINGLE_COLOR,
            Self::Other(v) => v,
        }
    }
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_LOCATION`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZoneLocation {
    /// `..._GPU_TOP_0` (0x00).
    GpuTop0,
    /// `..._GPU_FRONT_0` (0x08).
    GpuFront0,
    /// `..._GPU_BACK_0` (0x0C).
    GpuBack0,
    /// `..._SLI_TOP_0` (0x20).
    SliTop0,
    /// `..._INVALID` (0xFFFFFFFF).
    Invalid,
    /// A value the header does not define; kept verbatim.
    Other(u32),
}

impl ZoneLocation {
    /// Decodes the raw `zoneLocation` field.
    #[must_use]
    pub const fn from_raw(v: u32) -> Self {
        match v {
            raw::ZONE_LOCATION_GPU_TOP_0 => Self::GpuTop0,
            raw::ZONE_LOCATION_GPU_FRONT_0 => Self::GpuFront0,
            raw::ZONE_LOCATION_GPU_BACK_0 => Self::GpuBack0,
            raw::ZONE_LOCATION_SLI_TOP_0 => Self::SliTop0,
            raw::ZONE_LOCATION_INVALID => Self::Invalid,
            other => Self::Other(other),
        }
    }
}

/// `NV_GPU_CLIENT_ILLUM_CTRL_MODE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CtrlMode {
    /// `..._MANUAL` (0, alias of the deprecated `..._MANUAL_RGB`): a static color.
    Manual,
    /// `..._PIECEWISE_LINEAR` (1): a two-endpoint animation; decoded only as raw bytes.
    PiecewiseLinear,
    /// `..._INVALID` (0xFF).
    Invalid,
    /// A value the header does not define; kept verbatim.
    Other(u32),
}

impl CtrlMode {
    /// Decodes the raw `ctrlMode` field.
    #[must_use]
    pub const fn from_raw(v: u32) -> Self {
        match v {
            raw::CTRL_MODE_MANUAL => Self::Manual,
            raw::CTRL_MODE_PIECEWISE_LINEAR => Self::PiecewiseLinear,
            raw::CTRL_MODE_INVALID => Self::Invalid,
            other => Self::Other(other),
        }
    }

    /// Encodes to the raw `ctrlMode` field.
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        match self {
            Self::Manual => raw::CTRL_MODE_MANUAL,
            Self::PiecewiseLinear => raw::CTRL_MODE_PIECEWISE_LINEAR,
            Self::Invalid => raw::CTRL_MODE_INVALID,
            Self::Other(v) => v,
        }
    }
}

/// Static information about one illumination zone.
///
/// `location`, `device_index` and `provider_index` come from `NvAPI_GPU_ClientIllumZonesGetInfo`, whose
/// function id is not in the reference sources; until it is, [`NvApi::illum_zones_info`](crate::NvApi::illum_zones_info)
/// derives the list from `GetControl` (as OpenRGB's `getInfo()` does) and leaves these `None`.
/// [`ZoneInfo::list_from_raw`] decodes a real `NV_GPU_CLIENT_ILLUM_ZONE_INFO_PARAMS_V1` and fills them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ZoneInfo {
    /// Index of the zone in the driver's zone array (`zones[index]`).
    pub index: usize,
    /// Zone type.
    pub kind: ZoneKind,
    /// Physical location on the board, if known.
    pub location: Option<ZoneLocation>,
    /// `illumDeviceIdx`, if known.
    pub device_index: Option<u8>,
    /// `provIdx`, if known.
    pub provider_index: Option<u8>,
}

impl ZoneInfo {
    /// Decodes the zone list of a raw `NV_GPU_CLIENT_ILLUM_ZONE_INFO_PARAMS_V1`.
    ///
    /// # Errors
    /// [`NvApiError::Unsupported`] if `numIllumZones` exceeds the 32-slot array.
    pub fn list_from_raw(params: &raw::ZoneInfoParamsV1) -> Result<Vec<Self>, NvApiError> {
        let count = zone_count(params.num_illum_zones)?;
        Ok(params.zones[..count]
            .iter()
            .enumerate()
            .map(|(index, z)| Self {
                index,
                kind: ZoneKind::from_raw(z.zone_type),
                location: Some(ZoneLocation::from_raw(z.zone_location)),
                device_index: Some(z.illum_device_idx),
                provider_index: Some(z.prov_idx),
            })
            .collect())
    }

    /// Derives type-and-index-only zone info from control params (what OpenRGB's `getInfo()` does).
    #[must_use]
    pub fn list_from_control(params: &ZoneControlParams) -> Vec<Self> {
        params
            .zones
            .iter()
            .enumerate()
            .map(|(index, z)| Self {
                index,
                kind: z.kind,
                location: None,
                device_index: None,
                provider_index: None,
            })
            .collect()
    }
}

fn zone_count(n: u32) -> Result<usize, NvApiError> {
    usize::try_from(n)
        .ok()
        .filter(|&n| n <= raw::NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX)
        .ok_or_else(|| {
            NvApiError::Unsupported(format!(
                "driver reported {n} illumination zones, more than the {} the V1 struct holds",
                raw::NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX
            ))
        })
}

/// The manual (static) color of a zone, shaped by the zone type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZoneColor {
    /// `MANUAL_RGB_PARAMS` of an RGB zone. `brightness` is a percentage (`brightnessPct`).
    Rgb {
        /// `colorR`.
        r: u8,
        /// `colorG`.
        g: u8,
        /// `colorB`.
        b: u8,
        /// `brightnessPct`.
        brightness: u8,
    },
    /// `MANUAL_RGBW_PARAMS` of an RGBW zone.
    Rgbw {
        /// `colorR`.
        r: u8,
        /// `colorG`.
        g: u8,
        /// `colorB`.
        b: u8,
        /// `colorW`.
        w: u8,
        /// `brightnessPct`.
        brightness: u8,
    },
    /// Brightness-only params of a `SINGLE_COLOR` or `COLOR_FIXED` zone.
    Single {
        /// `brightnessPct`.
        brightness: u8,
    },
    /// Not decoded: the zone is not in manual mode or has an unknown/invalid type. Its bytes are carried
    /// through unchanged.
    Other,
}

/// One zone's control state: the decoded fields plus the zone's original 200 raw bytes.
///
/// [`ZoneControlParams::to_raw`] starts from the original bytes and overwrites only `type`, `ctrlMode`
/// and, when [`manual`](Self::manual) is not [`ZoneColor::Other`], the manual parameter bytes for the
/// zone's type. Everything else (reserved bytes, piecewise-linear data, unknown fields) is sent back
/// exactly as the driver returned it.
///
/// Equality compares the decoded fields plus the preserved bytes (i.e. everything that ends up on the
/// wire), so values produced by `from_raw`/`to_raw` are equal exactly when they encode to the same bytes.
#[derive(Clone, Copy, Debug)]
pub struct ZoneControl {
    /// Zone type.
    pub kind: ZoneKind,
    /// Control mode. A decoded manual color is only written when this is [`CtrlMode::Manual`].
    pub mode: CtrlMode,
    /// Manual color, see [`ZoneColor`].
    pub manual: ZoneColor,
    original: raw::ZoneControlV1,
}

impl ZoneControl {
    /// A new zone in manual mode with all other bytes zero.
    #[must_use]
    pub const fn new(kind: ZoneKind, manual: ZoneColor) -> Self {
        Self {
            kind,
            mode: CtrlMode::Manual,
            manual,
            original: raw::ZoneControlV1::zeroed(),
        }
    }

    /// Decodes one raw zone, keeping its bytes for the round trip.
    #[must_use]
    pub fn from_raw(z: &raw::ZoneControlV1) -> Self {
        let kind = ZoneKind::from_raw(z.zone_type);
        let mode = CtrlMode::from_raw(z.ctrl_mode);
        let manual = if mode == CtrlMode::Manual {
            match kind {
                ZoneKind::Rgb => {
                    let p = z.manual_rgb();
                    ZoneColor::Rgb {
                        r: p.color_r,
                        g: p.color_g,
                        b: p.color_b,
                        brightness: p.brightness_pct,
                    }
                }
                ZoneKind::Rgbw => {
                    let p = z.manual_rgbw();
                    ZoneColor::Rgbw {
                        r: p.color_r,
                        g: p.color_g,
                        b: p.color_b,
                        w: p.color_w,
                        brightness: p.brightness_pct,
                    }
                }
                ZoneKind::SingleColor => ZoneColor::Single {
                    brightness: z.manual_single_color(),
                },
                ZoneKind::ColorFixed => ZoneColor::Single {
                    brightness: z.manual_color_fixed(),
                },
                ZoneKind::Invalid | ZoneKind::Other(_) => ZoneColor::Other,
            }
        } else {
            ZoneColor::Other
        };
        Self {
            kind,
            mode,
            manual,
            original: *z,
        }
    }

    /// Encodes this zone onto its original bytes.
    ///
    /// # Errors
    /// [`NvApiError::Unsupported`] if `manual` does not fit `kind` (e.g. an RGBW color on an RGB zone),
    /// or if a manual color is set while `mode` is not [`CtrlMode::Manual`].
    pub fn to_raw(&self) -> Result<raw::ZoneControlV1, NvApiError> {
        let mut z = self.original;
        z.zone_type = self.kind.to_raw();
        z.ctrl_mode = self.mode.to_raw();
        if self.manual != ZoneColor::Other && self.mode != CtrlMode::Manual {
            return Err(NvApiError::Unsupported(format!(
                "manual color {:?} on a zone in mode {:?}",
                self.manual, self.mode
            )));
        }
        match (self.kind, self.manual) {
            (_, ZoneColor::Other) => {}
            (ZoneKind::Rgb, ZoneColor::Rgb { r, g, b, brightness }) => {
                z.set_manual_rgb(raw::ManualRgbParams {
                    color_r: r,
                    color_g: g,
                    color_b: b,
                    brightness_pct: brightness,
                });
            }
            (
                ZoneKind::Rgbw,
                ZoneColor::Rgbw {
                    r,
                    g,
                    b,
                    w,
                    brightness,
                },
            ) => {
                z.set_manual_rgbw(raw::ManualRgbwParams {
                    color_r: r,
                    color_g: g,
                    color_b: b,
                    color_w: w,
                    brightness_pct: brightness,
                });
            }
            (ZoneKind::SingleColor, ZoneColor::Single { brightness }) => {
                z.set_manual_single_color(brightness);
            }
            (ZoneKind::ColorFixed, ZoneColor::Single { brightness }) => {
                z.set_manual_color_fixed(brightness);
            }
            (kind, color) => {
                return Err(NvApiError::Unsupported(format!(
                    "color {color:?} does not fit a zone of type {kind:?}"
                )));
            }
        }
        Ok(z)
    }

    /// The original bytes with every region this struct owns (type, mode, decoded manual params) zeroed.
    fn preserved_bytes(&self) -> [u8; raw::ZONE_CONTROL_V1_SIZE] {
        let mut b = self.original.to_bytes();
        b[..8].fill(0);
        let params_len = match self.manual {
            ZoneColor::Rgb { .. } => 4,
            ZoneColor::Rgbw { .. } => 5,
            ZoneColor::Single { .. } => 1,
            ZoneColor::Other => 0,
        };
        b[8..8 + params_len].fill(0);
        b
    }
}

impl PartialEq for ZoneControl {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.mode == other.mode
            && self.manual == other.manual
            && self.preserved_bytes() == other.preserved_bytes()
    }
}

impl Eq for ZoneControl {}

/// Owned, faithfully round-trippable form of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1`.
///
/// `from_raw` followed by `to_raw` with no changes reproduces all 6476 bytes, including the reserved
/// header bytes, the 31 reserved flag bits and the unused zone slots. This is what lets a driver write
/// the initial params back on release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneControlParams {
    /// `bDefault`: these are the default values (`true`) or the currently active ones (`false`).
    pub default_values: bool,
    /// The zones, `zones[..numIllumZonesControl]`.
    pub zones: Vec<ZoneControl>,
    /// `rsvdField` (bits 1..=31 of the bit field word), kept verbatim.
    reserved_flags: u32,
    /// Header `rsvd[64]`, kept verbatim.
    reserved: [u8; 64],
    /// Original bytes of every slot; only slots at index `>= zones.len()` are emitted from here.
    /// Boxed to keep the type small (6.4 KiB). `from_raw` zeroes the entries that `zones` represents, so
    /// if a caller shrinks `zones`, the freed slot positions are sent as zeros, not the original bytes.
    slots: Box<[raw::ZoneControlV1; raw::NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX]>,
}

impl ZoneControlParams {
    /// Params with the given zones and every other byte zero.
    #[must_use]
    pub fn new(default_values: bool, zones: Vec<ZoneControl>) -> Self {
        Self {
            default_values,
            zones,
            reserved_flags: 0,
            reserved: [0; 64],
            slots: Box::new([raw::ZoneControlV1::zeroed(); raw::NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX]),
        }
    }

    /// Decodes raw params as returned by `NvAPI_GPU_ClientIllumZonesGetControl`.
    ///
    /// # Errors
    /// [`NvApiError::Unsupported`] if `version` is not [`raw::ZONE_CONTROL_PARAMS_VER_1`] or the zone
    /// count exceeds 32.
    pub fn from_raw(p: &raw::ZoneControlParamsV1) -> Result<Self, NvApiError> {
        if p.version != raw::ZONE_CONTROL_PARAMS_VER_1 {
            return Err(NvApiError::Unsupported(format!(
                "control params version {:#x}, expected {:#x}",
                p.version,
                raw::ZONE_CONTROL_PARAMS_VER_1
            )));
        }
        let count = zone_count(p.num_illum_zones_control)?;
        let mut slots = Box::new(p.zones);
        // Slots that are represented by `zones` are not needed here; zero them so equality ignores them.
        for slot in &mut slots[..count] {
            *slot = raw::ZoneControlV1::zeroed();
        }
        Ok(Self {
            default_values: p.is_default(),
            zones: p.zones[..count].iter().map(ZoneControl::from_raw).collect(),
            reserved_flags: p.flags & !raw::CONTROL_FLAG_DEFAULT,
            reserved: p.rsvd,
            slots,
        })
    }

    /// Encodes to raw params ready for `NvAPI_GPU_ClientIllumZonesSetControl` (or, with
    /// `default_values` set as desired, `GetControl`).
    ///
    /// # Errors
    /// [`NvApiError::Unsupported`] if there are more than 32 zones or a zone fails
    /// [`ZoneControl::to_raw`].
    pub fn to_raw(&self) -> Result<raw::ZoneControlParamsV1, NvApiError> {
        let count = self.zones.len();
        let count_u32 = u32::try_from(count)
            .ok()
            .filter(|_| count <= raw::NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX)
            .ok_or_else(|| {
                NvApiError::Unsupported(format!(
                    "{count} zones, the V1 struct holds at most {}",
                    raw::NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX
                ))
            })?;
        let mut p = raw::ZoneControlParamsV1::zeroed();
        p.flags = self.reserved_flags & !raw::CONTROL_FLAG_DEFAULT
            | if self.default_values {
                raw::CONTROL_FLAG_DEFAULT
            } else {
                0
            };
        p.num_illum_zones_control = count_u32;
        p.rsvd = self.reserved;
        p.zones = *self.slots;
        for (slot, zone) in p.zones.iter_mut().zip(&self.zones) {
            *slot = zone.to_raw()?;
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pci_id_unpacking() {
        let ids = PciIds {
            device_id: 0x2C02_10DE,
            subsystem_id: 0x2095_10DE,
            revision_id: 0xA1,
            ext_device_id: 0x2C02,
        };
        assert_eq!(ids.vendor(), 0x10DE);
        assert_eq!(ids.device(), 0x2C02);
        assert_eq!(ids.subsystem_vendor(), 0x10DE);
        assert_eq!(ids.subsystem_device(), 0x2095);

        let edge = PciIds {
            device_id: 0xFFFF_0000,
            subsystem_id: 0x0000_FFFF,
            revision_id: 0,
            ext_device_id: 0,
        };
        assert_eq!((edge.device(), edge.vendor()), (0xFFFF, 0x0000));
        assert_eq!(
            (edge.subsystem_device(), edge.subsystem_vendor()),
            (0x0000, 0xFFFF)
        );
    }

    fn sample() -> ZoneControlParams {
        ZoneControlParams::new(
            false,
            vec![
                ZoneControl::new(
                    ZoneKind::Rgb,
                    ZoneColor::Rgb {
                        r: 0x12,
                        g: 0x34,
                        b: 0x56,
                        brightness: 80,
                    },
                ),
                ZoneControl::new(
                    ZoneKind::Rgbw,
                    ZoneColor::Rgbw {
                        r: 1,
                        g: 2,
                        b: 3,
                        w: 4,
                        brightness: 100,
                    },
                ),
            ],
        )
    }

    #[test]
    fn hand_built_params_round_trip() {
        let x = sample();
        let raw = x.to_raw().expect("encodes");
        let bytes = raw.to_bytes();
        assert_eq!(&bytes[0..4], &72012u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0], "bDefault = 0");
        assert_eq!(&bytes[8..12], &2u32.to_le_bytes(), "two zones");
        let z0 = raw::ZONE_CONTROL_PARAMS_V1_HEADER_SIZE;
        assert_eq!(
            &bytes[z0..z0 + 12],
            &[1, 0, 0, 0, 0, 0, 0, 0, 0x12, 0x34, 0x56, 80]
        );
        let z1 = z0 + raw::ZONE_CONTROL_V1_SIZE;
        assert_eq!(&bytes[z1..z1 + 13], &[3, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 100]);

        let y = ZoneControlParams::from_raw(&raw).expect("decodes");
        assert_eq!(y, x);
        assert_eq!(y.to_raw().expect("encodes").to_bytes(), bytes);
    }

    /// The invariant the driver's `release()` depends on: GetControl -> SetControl with no changes sends
    /// exactly the bytes the driver returned, junk in every reserved region included.
    #[test]
    fn unchanged_raw_params_reproduce_every_byte() {
        let mut bytes = [0u8; raw::ZONE_CONTROL_PARAMS_V1_SIZE];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from((i * 7 + 3) % 251).expect("below 256");
        }
        bytes[0..4].copy_from_slice(&raw::ZONE_CONTROL_PARAMS_VER_1.to_le_bytes());
        // bDefault = 1 plus junk in the 31 reserved bits.
        bytes[4..8].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        bytes[8..12].copy_from_slice(&5u32.to_le_bytes());
        let z = |i: usize| raw::ZONE_CONTROL_PARAMS_V1_HEADER_SIZE + i * raw::ZONE_CONTROL_V1_SIZE;
        // zone 0: RGB manual, zone 1: RGBW manual, zone 2: RGB piecewise, zone 3: color fixed manual,
        // zone 4: unknown type 9 in an unknown mode 7. Slots 5..32 keep their junk.
        let heads: [(u32, u32); 5] = [(1, 0), (3, 0), (1, 1), (2, 0), (9, 7)];
        for (i, (ty, mode)) in heads.into_iter().enumerate() {
            bytes[z(i)..z(i) + 4].copy_from_slice(&ty.to_le_bytes());
            bytes[z(i) + 4..z(i) + 8].copy_from_slice(&mode.to_le_bytes());
        }

        let p = raw::ZoneControlParamsV1::from_bytes(&bytes);
        let decoded = ZoneControlParams::from_raw(&p).expect("decodes");
        assert!(decoded.default_values);
        assert_eq!(decoded.zones.len(), 5);
        assert!(matches!(decoded.zones[0].manual, ZoneColor::Rgb { .. }));
        assert!(matches!(decoded.zones[1].manual, ZoneColor::Rgbw { .. }));
        assert_eq!(decoded.zones[2].manual, ZoneColor::Other);
        assert_eq!(decoded.zones[2].mode, CtrlMode::PiecewiseLinear);
        assert!(matches!(decoded.zones[3].manual, ZoneColor::Single { .. }));
        assert_eq!(decoded.zones[4].kind, ZoneKind::Other(9));
        assert_eq!(decoded.zones[4].mode, CtrlMode::Other(7));

        let again = decoded.to_raw().expect("encodes").to_bytes();
        assert_eq!(again, bytes, "every one of the 6476 bytes must survive");
        assert_eq!(
            ZoneControlParams::from_raw(&raw::ZoneControlParamsV1::from_bytes(&again)).expect("decodes"),
            decoded
        );
    }

    #[test]
    fn color_edit_touches_only_param_bytes() {
        let mut bytes = [0xA5u8; raw::ZONE_CONTROL_PARAMS_V1_SIZE];
        bytes[0..4].copy_from_slice(&raw::ZONE_CONTROL_PARAMS_VER_1.to_le_bytes());
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        let z0 = raw::ZONE_CONTROL_PARAMS_V1_HEADER_SIZE;
        bytes[z0..z0 + 8].copy_from_slice(&[3, 0, 0, 0, 0, 0, 0, 0]);
        let mut decoded =
            ZoneControlParams::from_raw(&raw::ZoneControlParamsV1::from_bytes(&bytes)).expect("decodes");
        decoded.zones[0].manual = ZoneColor::Rgbw {
            r: 10,
            g: 20,
            b: 30,
            w: 40,
            brightness: 50,
        };
        let out = decoded.to_raw().expect("encodes").to_bytes();
        let mut expected = bytes;
        expected[z0 + 8..z0 + 13].copy_from_slice(&[10, 20, 30, 40, 50]);
        assert_eq!(out, expected);
    }

    #[test]
    fn mismatches_are_rejected() {
        let wrong_shape = ZoneControl::new(ZoneKind::Rgb, ZoneColor::Single { brightness: 1 });
        assert!(matches!(wrong_shape.to_raw(), Err(NvApiError::Unsupported(_))));

        let mut wrong_mode = ZoneControl::new(
            ZoneKind::Rgb,
            ZoneColor::Rgb {
                r: 0,
                g: 0,
                b: 0,
                brightness: 0,
            },
        );
        wrong_mode.mode = CtrlMode::PiecewiseLinear;
        assert!(matches!(wrong_mode.to_raw(), Err(NvApiError::Unsupported(_))));

        let too_many = ZoneControlParams::new(
            false,
            vec![ZoneControl::new(ZoneKind::Invalid, ZoneColor::Other); 33],
        );
        assert!(matches!(too_many.to_raw(), Err(NvApiError::Unsupported(_))));

        let mut bad = raw::ZoneControlParamsV1::zeroed();
        bad.num_illum_zones_control = 33;
        assert!(matches!(
            ZoneControlParams::from_raw(&bad),
            Err(NvApiError::Unsupported(_))
        ));
        bad.num_illum_zones_control = 0;
        bad.version = 0x0001_0000;
        assert!(matches!(
            ZoneControlParams::from_raw(&bad),
            Err(NvApiError::Unsupported(_))
        ));
    }

    #[test]
    fn zone_info_decoding() {
        let mut p = raw::ZoneInfoParamsV1::zeroed();
        p.num_illum_zones = 2;
        p.zones[0].zone_type = raw::ZONE_TYPE_RGBW;
        p.zones[0].zone_location = raw::ZONE_LOCATION_GPU_TOP_0;
        p.zones[0].illum_device_idx = 0;
        p.zones[0].prov_idx = 1;
        p.zones[1].zone_type = raw::ZONE_TYPE_SINGLE_COLOR;
        p.zones[1].zone_location = 0x1234;
        let list = ZoneInfo::list_from_raw(&p).expect("decodes");
        assert_eq!(
            list,
            vec![
                ZoneInfo {
                    index: 0,
                    kind: ZoneKind::Rgbw,
                    location: Some(ZoneLocation::GpuTop0),
                    device_index: Some(0),
                    provider_index: Some(1),
                },
                ZoneInfo {
                    index: 1,
                    kind: ZoneKind::SingleColor,
                    location: Some(ZoneLocation::Other(0x1234)),
                    device_index: Some(0),
                    provider_index: Some(0),
                },
            ]
        );

        let from_control = ZoneInfo::list_from_control(&sample());
        assert_eq!(from_control.len(), 2);
        assert_eq!(from_control[1].kind, ZoneKind::Rgbw);
        assert_eq!(from_control[1].location, None);
    }
}
