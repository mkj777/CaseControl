//! `#[repr(C)]` mirror of the NvAPI illumination ABI.
//!
//! Every layout, enum value and function id here is copied from OpenRGB's
//! `dependencies/NVFC/nvapi.h` and `nvapi.cpp` (vendored at `reference/openrgb/NVFC/`), with the
//! exceptions called out explicitly (the status code table, see [`NvStatus`]).
//!
//! Layout notes that apply to the whole module:
//!
//! - The header has **no `#pragma pack`** directive anywhere, so every struct uses natural C alignment
//!   (`#[repr(C)]`, never `packed`).
//! - C enum fields (`NV_GPU_CLIENT_ILLUM_ZONE_TYPE`, `..._LOCATION`, `..._CTRL_MODE`,
//!   `..._PIECEWISE_LINEAR_CYCLE_TYPE`) are 4 bytes wide on MSVC and GCC and are stored as plain `u32`
//!   here, never as Rust enums: the driver may return values we do not know, and an unknown discriminant
//!   in a Rust enum would be undefined behaviour. The known values are the constants below.
//! - Padding the C compiler inserts implicitly is spelled out as explicit `_pad*` fields so that every
//!   byte of every struct is initialised and can be viewed as bytes soundly. The offset asserts at the
//!   bottom prove the explicit padding lands where the C compiler puts it.
//! - Union fields are crate-private. Outside code obtains union values only through
//!   [`ZoneControlParamsV1::zeroed`], [`ZoneControlV1::from_bytes`] and friends, which guarantees all
//!   union bytes are initialised (a Rust union literal naming a small member would leave the rest
//!   uninitialised and make the byte views unsound).
//! - The `version` field formula is `NV_STRUCT_VERSION(S, V) = (V << 16) | sizeof(S)`, see
//!   [`make_nvapi_version`].

use core::ffi::{c_char, c_void};
use core::fmt;
use core::mem::{offset_of, size_of};

// ---------------------------------------------------------------------------------------------------
// Scalars, handles, version macro
// ---------------------------------------------------------------------------------------------------

/// `NV_PHYSICAL_GPU_HANDLE` (`typedef NV_S32* NV_HANDLE`): an opaque pointer owned by the driver.
pub type NvPhysicalGpuHandle = *mut i32;

/// Length of `NV_SHORT_STRING` (`typedef char NV_SHORT_STRING[64]`).
pub const NV_SHORT_STRING_LEN: usize = 64;

/// `NV_SHORT_STRING`.
pub type NvShortString = [c_char; NV_SHORT_STRING_LEN];

/// Size of the handle array OpenRGB passes to `NvAPI_EnumPhysicalGPUs`
/// (`static NV_PHYSICAL_GPU_HANDLE gpu_handles[64]` in `NVIDIAIlluminationControllerDetect`).
pub const NVAPI_MAX_PHYSICAL_GPUS: usize = 64;

/// `NV_STRUCT_VERSION(STRUCT, VERSION) = ((VERSION) << 16) | sizeof(STRUCT)`.
///
/// The header *uses* `MAKE_NVAPI_VERSION` for the two illumination `*_VER_1` defines but never defines
/// it (those defines are dead code in OpenRGB, which is why the controller hard-codes `72012`). The only
/// version macro the header defines is `NV_STRUCT_VERSION`, the same formula. `72012 == (1 << 16) | 6476`
/// confirms it for the control params.
#[must_use]
pub const fn make_nvapi_version(size: usize, version: u32) -> u32 {
    // Struct sizes here are a few KiB; the size asserts below pin the exact values, so the cast is exact.
    #[allow(clippy::cast_possible_truncation)]
    let size = size as u32;
    (version << 16) | size
}

// ---------------------------------------------------------------------------------------------------
// Status codes
// ---------------------------------------------------------------------------------------------------

/// `NV_STATUS` (`typedef NV_S32 NV_STATUS`) as a newtype.
///
/// Source caveat: OpenRGB's `nvapi.h` defines no status enum at all; it only has `#define NVAPI_OK 0`
/// (in `NVIDIAIlluminationV1Controller_Windows_Linux.h`) and returns `-1` from its wrappers when a
/// function pointer is missing. The names and values in [`NvStatus::name`] are the `NvAPI_Status` enum
/// of NVIDIA's public NVAPI SDK. A wrong entry here can only mislabel an error message; it never changes
/// which function is called. The header defines no illumination-specific status codes.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NvStatus(pub i32);

impl NvStatus {
    /// `NVAPI_OK`.
    pub const OK: Self = Self(0);
    /// `NVAPI_ERROR` (also OpenRGB's "function not loaded" sentinel).
    pub const ERROR: Self = Self(-1);
    /// `NVAPI_LIBRARY_NOT_FOUND`.
    pub const LIBRARY_NOT_FOUND: Self = Self(-2);
    /// `NVAPI_NO_IMPLEMENTATION`.
    pub const NO_IMPLEMENTATION: Self = Self(-3);
    /// `NVAPI_API_NOT_INITIALIZED`.
    pub const API_NOT_INITIALIZED: Self = Self(-4);
    /// `NVAPI_INVALID_ARGUMENT`.
    pub const INVALID_ARGUMENT: Self = Self(-5);
    /// `NVAPI_NVIDIA_DEVICE_NOT_FOUND`.
    ///
    /// Unlike the other constants, this one is relied upon for control flow: `NvApi::physical_gpus`
    /// treats it as "no NVIDIA GPU present" and returns an empty list. Its value (-6) comes from
    /// NVIDIA's public NVAPI SDK, not from the OpenRGB reference.
    pub const NVIDIA_DEVICE_NOT_FOUND: Self = Self(-6);
    /// `NVAPI_END_ENUMERATION`.
    pub const END_ENUMERATION: Self = Self(-7);
    /// `NVAPI_INVALID_HANDLE`.
    pub const INVALID_HANDLE: Self = Self(-8);
    /// `NVAPI_INCOMPATIBLE_STRUCT_VERSION`.
    pub const INCOMPATIBLE_STRUCT_VERSION: Self = Self(-9);
    /// `NVAPI_NOT_SUPPORTED`.
    pub const NOT_SUPPORTED: Self = Self(-104);

    /// `(value, name)` pairs from the public NVAPI SDK. See the type docs for provenance.
    const TABLE: &'static [(i32, &'static str)] = &[
        (0, "NVAPI_OK"),
        (-1, "NVAPI_ERROR"),
        (-2, "NVAPI_LIBRARY_NOT_FOUND"),
        (-3, "NVAPI_NO_IMPLEMENTATION"),
        (-4, "NVAPI_API_NOT_INITIALIZED"),
        (-5, "NVAPI_INVALID_ARGUMENT"),
        (-6, "NVAPI_NVIDIA_DEVICE_NOT_FOUND"),
        (-7, "NVAPI_END_ENUMERATION"),
        (-8, "NVAPI_INVALID_HANDLE"),
        (-9, "NVAPI_INCOMPATIBLE_STRUCT_VERSION"),
        (-10, "NVAPI_HANDLE_INVALIDATED"),
        (-11, "NVAPI_OPENGL_CONTEXT_NOT_CURRENT"),
        (-12, "NVAPI_NO_GL_EXPERT"),
        (-13, "NVAPI_INSTRUMENTATION_DISABLED"),
        (-14, "NVAPI_INVALID_POINTER"),
        (-100, "NVAPI_EXPECTED_LOGICAL_GPU_HANDLE"),
        (-101, "NVAPI_EXPECTED_PHYSICAL_GPU_HANDLE"),
        (-102, "NVAPI_EXPECTED_DISPLAY_HANDLE"),
        (-103, "NVAPI_INVALID_COMBINATION"),
        (-104, "NVAPI_NOT_SUPPORTED"),
        (-105, "NVAPI_PORTID_NOT_FOUND"),
        (-106, "NVAPI_EXPECTED_UNATTACHED_DISPLAY_HANDLE"),
        (-107, "NVAPI_INVALID_PERF_LEVEL"),
        (-108, "NVAPI_DEVICE_BUSY"),
        (-109, "NVAPI_NV_PERSIST_FILE_NOT_FOUND"),
        (-110, "NVAPI_PERSIST_DATA_NOT_FOUND"),
    ];

    /// `true` for `NVAPI_OK`.
    #[must_use]
    pub const fn is_ok(self) -> bool {
        self.0 == 0
    }

    /// Symbolic name, or `"NVAPI_UNKNOWN_STATUS"` for values outside the table.
    #[must_use]
    pub fn name(self) -> &'static str {
        Self::TABLE
            .iter()
            .find(|(code, _)| *code == self.0)
            .map_or("NVAPI_UNKNOWN_STATUS", |(_, name)| name)
    }
}

impl fmt::Debug for NvStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.name(), self.0)
    }
}

impl fmt::Display for NvStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

// ---------------------------------------------------------------------------------------------------
// Function ids and pointer types
// ---------------------------------------------------------------------------------------------------

/// Interface ids passed to `nvapi_QueryInterface`, copied from `QueryInterfaces()` in `nvapi.cpp`.
///
/// For the two illumination calls the `// Interface:` comments in `nvapi.h` say `73C01D58` and
/// `57024C62`, which disagree with the ids `nvapi.cpp` actually resolves. The `.cpp` values are what
/// OpenRGB executes against real cards and are used here.
///
/// `NvAPI_GPU_ClientIllumZonesGetInfo` is deliberately absent: no id for it exists in the reference
/// sources, and resolving a guessed id would call an unknown function with a 4.5 KiB struct pointer.
pub mod ids {
    /// `NvAPI_Initialize`.
    pub const INITIALIZE: u32 = 0x0150_E828;
    /// `NvAPI_Unload`.
    pub const UNLOAD: u32 = 0xD22B_DD7E;
    /// `NvAPI_EnumPhysicalGPUs`.
    pub const ENUM_PHYSICAL_GPUS: u32 = 0xE5AC_921F;
    /// `NvAPI_GPU_GetFullName` (written `0x0CEEE8E9F` in the source).
    pub const GPU_GET_FULL_NAME: u32 = 0xCEEE_8E9F;
    /// `NvAPI_GPU_GetPCIIdentifiers`.
    pub const GPU_GET_PCI_IDENTIFIERS: u32 = 0x2DDF_B66E;
    /// `NvAPI_GPU_ClientIllumZonesGetControl`.
    pub const GPU_CLIENT_ILLUM_ZONES_GET_CONTROL: u32 = 0x3DBF_5764;
    /// `NvAPI_GPU_ClientIllumZonesSetControl`.
    pub const GPU_CLIENT_ILLUM_ZONES_SET_CONTROL: u32 = 0x197D_065E;
}

/// `nvapi_QueryInterface`, exported by name from `nvapi64.dll` (`typedef void * (*)(int)`; the `.cpp`
/// calls it with an `NV_U32`).
///
/// Calling convention: the header declares every entry point as a plain C function (no
/// `NVAPI_INTERFACE`/`__stdcall` decoration), i.e. `extern "C"`. On x86-64 Windows there is only one
/// calling convention, so this matches the DLL exactly.
pub type QueryInterfaceFn = unsafe extern "C" fn(id: u32) -> *mut c_void;
/// `NV_STATUS NvAPI_Initialize()`.
pub type InitializeFn = unsafe extern "C" fn() -> NvStatus;
/// `NV_STATUS NvAPI_Unload()`.
pub type UnloadFn = unsafe extern "C" fn() -> NvStatus;
/// `NV_STATUS NvAPI_EnumPhysicalGPUs(NV_PHYSICAL_GPU_HANDLE*, NV_S32*)`.
pub type EnumPhysicalGpusFn =
    unsafe extern "C" fn(handles: *mut NvPhysicalGpuHandle, count: *mut i32) -> NvStatus;
/// `NV_STATUS NvAPI_GPU_GetFullName(NV_PHYSICAL_GPU_HANDLE, NV_SHORT_STRING)`.
pub type GpuGetFullNameFn = unsafe extern "C" fn(gpu: NvPhysicalGpuHandle, name: *mut c_char) -> NvStatus;
/// `NV_STATUS NvAPI_GPU_GetPCIIdentifiers(handle, NV_U32* device_id, NV_U32* sub_system_id,
/// NV_U32* revision_id, NV_U32* ext_device_id)`.
pub type GpuGetPciIdentifiersFn = unsafe extern "C" fn(
    gpu: NvPhysicalGpuHandle,
    device_id: *mut u32,
    sub_system_id: *mut u32,
    revision_id: *mut u32,
    ext_device_id: *mut u32,
) -> NvStatus;
/// `NV_STATUS NvAPI_GPU_ClientIllumZonesGetControl(handle, NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS*)`.
pub type GpuClientIllumZonesGetControlFn =
    unsafe extern "C" fn(gpu: NvPhysicalGpuHandle, params: *mut ZoneControlParamsV1) -> NvStatus;
/// `NV_STATUS NvAPI_GPU_ClientIllumZonesSetControl(handle, NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS*)`.
pub type GpuClientIllumZonesSetControlFn =
    unsafe extern "C" fn(gpu: NvPhysicalGpuHandle, params: *mut ZoneControlParamsV1) -> NvStatus;
/// Shape of `NvAPI_GPU_ClientIllumZonesGetInfo`. Its *id* is not in the reference sources, so this crate
/// does not resolve it (see [`ids`]); the type exists so the slot is ready once the id is supplied.
pub type GpuClientIllumZonesGetInfoFn =
    unsafe extern "C" fn(gpu: NvPhysicalGpuHandle, params: *mut ZoneInfoParamsV1) -> NvStatus;

// ---------------------------------------------------------------------------------------------------
// Enum values
// ---------------------------------------------------------------------------------------------------

/// `NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX`.
pub const NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX: usize = 32;
/// `NV_GPU_CLIENT_ILLUM_CTRL_MODE_PIECEWISE_LINEAR_COLOR_ENDPOINTS`.
pub const NV_GPU_CLIENT_ILLUM_CTRL_MODE_PIECEWISE_LINEAR_COLOR_ENDPOINTS: usize = 2;

/// `NV_GPU_CLIENT_ILLUM_ZONE_TYPE_INVALID`.
pub const ZONE_TYPE_INVALID: u32 = 0;
/// `NV_GPU_CLIENT_ILLUM_ZONE_TYPE_RGB`.
pub const ZONE_TYPE_RGB: u32 = 1;
/// `NV_GPU_CLIENT_ILLUM_ZONE_TYPE_COLOR_FIXED`.
pub const ZONE_TYPE_COLOR_FIXED: u32 = 2;
/// `NV_GPU_CLIENT_ILLUM_ZONE_TYPE_RGBW`.
pub const ZONE_TYPE_RGBW: u32 = 3;
/// `NV_GPU_CLIENT_ILLUM_ZONE_TYPE_SINGLE_COLOR`.
pub const ZONE_TYPE_SINGLE_COLOR: u32 = 4;

/// `NV_GPU_CLIENT_ILLUM_ZONE_LOCATION_GPU_TOP_0`.
pub const ZONE_LOCATION_GPU_TOP_0: u32 = 0x00;
/// `NV_GPU_CLIENT_ILLUM_ZONE_LOCATION_GPU_FRONT_0`.
pub const ZONE_LOCATION_GPU_FRONT_0: u32 = 0x08;
/// `NV_GPU_CLIENT_ILLUM_ZONE_LOCATION_GPU_BACK_0`.
pub const ZONE_LOCATION_GPU_BACK_0: u32 = 0x0C;
/// `NV_GPU_CLIENT_ILLUM_ZONE_LOCATION_SLI_TOP_0`.
pub const ZONE_LOCATION_SLI_TOP_0: u32 = 0x20;
/// `NV_GPU_CLIENT_ILLUM_ZONE_LOCATION_INVALID`.
pub const ZONE_LOCATION_INVALID: u32 = 0xFFFF_FFFF;

/// `NV_GPU_CLIENT_ILLUM_PIECEWISE_LINEAR_CYCLE_HALF_HALT`.
pub const PIECEWISE_LINEAR_CYCLE_HALF_HALT: u32 = 0;
/// `NV_GPU_CLIENT_ILLUM_PIECEWISE_LINEAR_CYCLE_FULL_HALT`.
pub const PIECEWISE_LINEAR_CYCLE_FULL_HALT: u32 = 1;
/// `NV_GPU_CLIENT_ILLUM_PIECEWISE_LINEAR_CYCLE_FULL_REPEAT`.
pub const PIECEWISE_LINEAR_CYCLE_FULL_REPEAT: u32 = 2;
/// `NV_GPU_CLIENT_ILLUM_PIECEWISE_LINEAR_CYCLE_INVALID`.
pub const PIECEWISE_LINEAR_CYCLE_INVALID: u32 = 0xFF;

/// `NV_GPU_CLIENT_ILLUM_CTRL_MODE_MANUAL` (same value as the deprecated `..._MANUAL_RGB`).
pub const CTRL_MODE_MANUAL: u32 = 0;
/// `NV_GPU_CLIENT_ILLUM_CTRL_MODE_PIECEWISE_LINEAR` (same value as the deprecated `..._PIECEWISE_LINEAR_RGB`).
pub const CTRL_MODE_PIECEWISE_LINEAR: u32 = 1;
/// `NV_GPU_CLIENT_ILLUM_CTRL_MODE_INVALID`.
pub const CTRL_MODE_INVALID: u32 = 0xFF;

// ---------------------------------------------------------------------------------------------------
// Zone info (NV_GPU_CLIENT_ILLUM_ZONE_INFO_*)
// ---------------------------------------------------------------------------------------------------

/// `NV_GPU_CLIENT_ILLUM_ZONE_INFO_DATA_RGB`; `_RGBW` and `_SINGLE_COLOR` are the identical
/// `{ NV_U8 rsvd; }` struct and share this type.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneInfoDataReserved {
    /// `rsvd`.
    pub rsvd: u8,
}

/// The anonymous `data` union of `NV_GPU_CLIENT_ILLUM_ZONE_INFO_V1` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub union ZoneInfoData {
    pub(crate) rgb: ZoneInfoDataReserved,
    pub(crate) rgbw: ZoneInfoDataReserved,
    pub(crate) single_color: ZoneInfoDataReserved,
    pub(crate) rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_INFO_V1` (140 bytes): `type`@0, `illumDeviceIdx`@4, `provIdx`@5,
/// 2 padding bytes, `zoneLocation`@8, `data`@12 (64), `rsvd`@76 (64).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneInfoV1 {
    /// `type` (`NV_GPU_CLIENT_ILLUM_ZONE_TYPE`).
    pub zone_type: u32,
    /// `illumDeviceIdx`: index of the illumination device that controls this zone.
    pub illum_device_idx: u8,
    /// `provIdx`: provider index (logical to physical zone mapping).
    pub prov_idx: u8,
    /// Implicit C padding before the 4-byte aligned `zoneLocation`.
    pub _pad0: [u8; 2],
    /// `zoneLocation` (`NV_GPU_CLIENT_ILLUM_ZONE_LOCATION`).
    pub zone_location: u32,
    /// `data` union.
    pub data: ZoneInfoData,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_INFO_PARAMS_V1` (4552 bytes = 4 + 4 + 64 + 32 * 140).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneInfoParamsV1 {
    /// `version`, must be [`ZONE_INFO_PARAMS_VER_1`].
    pub version: u32,
    /// `numIllumZones`.
    pub num_illum_zones: u32,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
    /// `zones[NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX]`.
    pub zones: [ZoneInfoV1; NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX],
}

/// Size of `NV_GPU_CLIENT_ILLUM_ZONE_INFO_V1` in bytes.
pub const ZONE_INFO_V1_SIZE: usize = 140;
/// Size of `NV_GPU_CLIENT_ILLUM_ZONE_INFO_PARAMS_V1` in bytes.
pub const ZONE_INFO_PARAMS_V1_SIZE: usize = 4552;
/// `NV_GPU_CLIENT_ILLUM_ZONE_INFO_PARAMS_VER_1` = `(1 << 16) | 4552` = `0x0001_11C8` = 70088.
/// Unlike the control params (72012) there is no sniffed value to confirm this one.
pub const ZONE_INFO_PARAMS_VER_1: u32 = make_nvapi_version(size_of::<ZoneInfoParamsV1>(), 1);

// ---------------------------------------------------------------------------------------------------
// Zone control (NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_*)
// ---------------------------------------------------------------------------------------------------

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_RGB_PARAMS` (4 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualRgbParams {
    /// `colorR`.
    pub color_r: u8,
    /// `colorG`.
    pub color_g: u8,
    /// `colorB`.
    pub color_b: u8,
    /// `brightnessPct`.
    pub brightness_pct: u8,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_RGB` (4 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualRgb {
    /// `rgbParams`.
    pub rgb_params: ManualRgbParams,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_PIECEWISE_LINEAR` (20 bytes):
/// `cycleType`@0, `grpCount`@4, padding@5, six `NV_U16` at 6..18, 2 tail padding bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PiecewiseLinear {
    /// `cycleType` (`NV_GPU_CLIENT_ILLUM_PIECEWISE_LINEAR_CYCLE_TYPE`).
    pub cycle_type: u32,
    /// `grpCount`.
    pub grp_count: u8,
    /// Implicit C padding before the 2-byte aligned `riseTimems`.
    pub _pad0: u8,
    /// `riseTimems`.
    pub rise_time_ms: u16,
    /// `fallTimems`.
    pub fall_time_ms: u16,
    /// `ATimems`.
    pub a_time_ms: u16,
    /// `BTimems`.
    pub b_time_ms: u16,
    /// `grpIdleTimems`.
    pub grp_idle_time_ms: u16,
    /// `phaseOffsetms`.
    pub phase_offset_ms: u16,
    /// Implicit C tail padding to the struct's 4-byte alignment.
    pub _pad1: [u8; 2],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_PIECEWISE_LINEAR_RGB` (28 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PiecewiseLinearRgb {
    /// `rgbParams[2]`.
    pub rgb_params: [ManualRgbParams; NV_GPU_CLIENT_ILLUM_CTRL_MODE_PIECEWISE_LINEAR_COLOR_ENDPOINTS],
    /// `piecewiseLinearData`.
    pub piecewise_linear_data: PiecewiseLinear,
}

/// The `data` union of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_RGB` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub union ZoneControlDataRgbUnion {
    pub(crate) manual_rgb: ManualRgb,
    pub(crate) piecewise_linear_rgb: PiecewiseLinearRgb,
    pub(crate) rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_RGB` (128 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneControlDataRgb {
    /// `data` union, interpreted per `ctrlMode`.
    pub data: ZoneControlDataRgbUnion,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_COLOR_FIXED_PARAMS` (1 byte).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualColorFixedParams {
    /// `brightnessPct`.
    pub brightness_pct: u8,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_COLOR_FIXED` (1 byte).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualColorFixed {
    /// `colorFixedParams`.
    pub color_fixed_params: ManualColorFixedParams,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_PIECEWISE_LINEAR_COLOR_FIXED` (24 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PiecewiseLinearColorFixed {
    /// `colorFixedParams[2]`.
    pub color_fixed_params:
        [ManualColorFixedParams; NV_GPU_CLIENT_ILLUM_CTRL_MODE_PIECEWISE_LINEAR_COLOR_ENDPOINTS],
    /// Implicit C padding before the 4-byte aligned `piecewiseLinearData`.
    pub _pad0: [u8; 2],
    /// `piecewiseLinearData`.
    pub piecewise_linear_data: PiecewiseLinear,
}

/// The `data` union of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_COLOR_FIXED` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub union ZoneControlDataColorFixedUnion {
    pub(crate) manual_color_fixed: ManualColorFixed,
    pub(crate) piecewise_linear_color_fixed: PiecewiseLinearColorFixed,
    pub(crate) rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_COLOR_FIXED` (128 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneControlDataColorFixed {
    /// `data` union, interpreted per `ctrlMode`.
    pub data: ZoneControlDataColorFixedUnion,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_RGBW_PARAMS` (5 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualRgbwParams {
    /// `colorR`.
    pub color_r: u8,
    /// `colorG`.
    pub color_g: u8,
    /// `colorB`.
    pub color_b: u8,
    /// `colorW`.
    pub color_w: u8,
    /// `brightnessPct`.
    pub brightness_pct: u8,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_RGBW` (5 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualRgbw {
    /// `rgbwParams`.
    pub rgbw_params: ManualRgbwParams,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_PIECEWISE_LINEAR_RGBW` (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PiecewiseLinearRgbw {
    /// `rgbwParams[2]`.
    pub rgbw_params: [ManualRgbwParams; NV_GPU_CLIENT_ILLUM_CTRL_MODE_PIECEWISE_LINEAR_COLOR_ENDPOINTS],
    /// Implicit C padding before the 4-byte aligned `piecewiseLinearData`.
    pub _pad0: [u8; 2],
    /// `piecewiseLinearData`.
    pub piecewise_linear_data: PiecewiseLinear,
}

/// The `data` union of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_RGBW` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub union ZoneControlDataRgbwUnion {
    pub(crate) manual_rgbw: ManualRgbw,
    pub(crate) piecewise_linear_rgbw: PiecewiseLinearRgbw,
    pub(crate) rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_RGBW` (128 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneControlDataRgbw {
    /// `data` union, interpreted per `ctrlMode`.
    pub data: ZoneControlDataRgbwUnion,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_SINGLE_COLOR_PARAMS` (1 byte).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualSingleColorParams {
    /// `brightnessPct`.
    pub brightness_pct: u8,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_MANUAL_SINGLE_COLOR` (1 byte).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualSingleColor {
    /// `singleColorParams`.
    pub single_color_params: ManualSingleColorParams,
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_PIECEWISE_LINEAR_SINGLE_COLOR` (24 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PiecewiseLinearSingleColor {
    /// `singleColorParams[2]`.
    pub single_color_params:
        [ManualSingleColorParams; NV_GPU_CLIENT_ILLUM_CTRL_MODE_PIECEWISE_LINEAR_COLOR_ENDPOINTS],
    /// Implicit C padding before the 4-byte aligned `piecewiseLinearData`.
    pub _pad0: [u8; 2],
    /// `piecewiseLinearData`.
    pub piecewise_linear_data: PiecewiseLinear,
}

/// The `data` union of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_SINGLE_COLOR` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub union ZoneControlDataSingleColorUnion {
    pub(crate) manual_single_color: ManualSingleColor,
    pub(crate) piecewise_linear_single_color: PiecewiseLinearSingleColor,
    pub(crate) rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_DATA_SINGLE_COLOR` (128 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneControlDataSingleColor {
    /// `data` union, interpreted per `ctrlMode`.
    pub data: ZoneControlDataSingleColorUnion,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
}

/// The `data` union of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_V1` (128 bytes), interpreted per `type`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union ZoneControlData {
    pub(crate) rgb: ZoneControlDataRgb,
    pub(crate) color_fixed: ZoneControlDataColorFixed,
    pub(crate) rgbw: ZoneControlDataRgbw,
    pub(crate) single_color: ZoneControlDataSingleColor,
    pub(crate) rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_V1` (200 bytes): `type`@0, `ctrlMode`@4, `data`@8 (128),
/// `rsvd`@136 (64).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneControlV1 {
    /// `type` (`NV_GPU_CLIENT_ILLUM_ZONE_TYPE`).
    pub zone_type: u32,
    /// `ctrlMode` (`NV_GPU_CLIENT_ILLUM_CTRL_MODE`).
    pub ctrl_mode: u32,
    /// `data` union.
    pub data: ZoneControlData,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
}

/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1` (6476 bytes = 4 + 4 + 4 + 64 + 32 * 200).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ZoneControlParamsV1 {
    /// `version`, must be [`ZONE_CONTROL_PARAMS_VER_1`].
    pub version: u32,
    /// The bit field `NV_U32 bDefault : 1; NV_U32 rsvdField : 31;` as one `u32`. MSVC and GCC both
    /// allocate bit fields from the least significant bit, so `bDefault` is bit 0
    /// ([`CONTROL_FLAG_DEFAULT`]) and `rsvdField` is bits 1..=31.
    pub flags: u32,
    /// `numIllumZonesControl`.
    pub num_illum_zones_control: u32,
    /// `rsvd[64]`.
    pub rsvd: [u8; 64],
    /// `zones[NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX]`.
    pub zones: [ZoneControlV1; NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX],
}

/// `bDefault` bit of [`ZoneControlParamsV1::flags`]: `1` = default values, `0` = currently active.
pub const CONTROL_FLAG_DEFAULT: u32 = 1;

/// Size of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_V1` in bytes.
pub const ZONE_CONTROL_V1_SIZE: usize = 200;
/// Size of `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1` in bytes.
pub const ZONE_CONTROL_PARAMS_V1_SIZE: usize = 6476;
/// Byte offset of `zones[0]` in `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1`.
pub const ZONE_CONTROL_PARAMS_V1_HEADER_SIZE: usize = 76;
/// `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_VER_1` = `(1 << 16) | 6476` = `0x0001_194C` = 72012, the
/// value OpenRGB's `NVIDIAIlluminationV1Controller::getControl` hard-codes ("found via sniffing").
pub const ZONE_CONTROL_PARAMS_VER_1: u32 = make_nvapi_version(size_of::<ZoneControlParamsV1>(), 1);

// ---------------------------------------------------------------------------------------------------
// Constructors and byte views
// ---------------------------------------------------------------------------------------------------

impl ZoneControlV1 {
    /// All-zero zone.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self::from_bytes(&[0; ZONE_CONTROL_V1_SIZE])
    }

    /// Reinterprets 200 bytes (little-endian, as the driver sees them) as a zone.
    #[must_use]
    pub const fn from_bytes(bytes: &[u8; ZONE_CONTROL_V1_SIZE]) -> Self {
        // SAFETY: ZoneControlV1 is repr(C), exactly 200 bytes (asserted below), and consists only of
        // integers, integer arrays and unions of those, so every bit pattern is a valid value.
        unsafe { core::mem::transmute::<[u8; ZONE_CONTROL_V1_SIZE], Self>(*bytes) }
    }

    /// The 200 bytes of this zone.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; ZONE_CONTROL_V1_SIZE] {
        // SAFETY: Same size (asserted below). Every byte is initialised: there is no implicit padding
        // (4 + 4 + 128 + 64 = 200), and union values can only be created via `from_bytes`/`zeroed`
        // (union fields are crate-private) or by the driver writing into such memory, so all 128 union
        // bytes are initialised.
        unsafe { core::mem::transmute::<Self, [u8; ZONE_CONTROL_V1_SIZE]>(*self) }
    }
}

impl PartialEq for ZoneControlV1 {
    fn eq(&self, other: &Self) -> bool {
        self.to_bytes() == other.to_bytes()
    }
}
impl Eq for ZoneControlV1 {}

impl ZoneControlParamsV1 {
    /// All-zero params with `version` set to [`ZONE_CONTROL_PARAMS_VER_1`].
    #[must_use]
    pub fn zeroed() -> Self {
        let mut params = Self::from_bytes(&[0; ZONE_CONTROL_PARAMS_V1_SIZE]);
        params.version = ZONE_CONTROL_PARAMS_VER_1;
        params
    }

    /// Reinterprets 6476 bytes as params.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; ZONE_CONTROL_PARAMS_V1_SIZE]) -> Self {
        // SAFETY: repr(C), exactly 6476 bytes (asserted below), integers/unions of integers only, so
        // every bit pattern is valid. transmute_copy reads size_of::<Self>() bytes, which the array has
        // exactly, and performs an unaligned read, so the array's alignment of 1 is fine.
        unsafe { core::mem::transmute_copy::<[u8; ZONE_CONTROL_PARAMS_V1_SIZE], Self>(bytes) }
    }

    /// The 6476 bytes of these params, exactly as the driver sees them.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; ZONE_CONTROL_PARAMS_V1_SIZE] {
        // SAFETY: Same size (asserted below); no implicit padding (4+4+4+64+32*200 = 6476) and every
        // zone is fully initialised (see ZoneControlV1::to_bytes). [u8; N] has no alignment requirement.
        unsafe { core::mem::transmute_copy::<Self, [u8; ZONE_CONTROL_PARAMS_V1_SIZE]>(self) }
    }

    /// `bDefault` bit.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.flags & CONTROL_FLAG_DEFAULT != 0
    }
}

impl PartialEq for ZoneControlParamsV1 {
    fn eq(&self, other: &Self) -> bool {
        self.to_bytes() == other.to_bytes()
    }
}
impl Eq for ZoneControlParamsV1 {}

impl ZoneInfoV1 {
    /// All-zero zone info.
    #[must_use]
    pub const fn zeroed() -> Self {
        // SAFETY: repr(C), exactly 140 bytes (asserted below), integers/unions of integers only (the C
        // padding is the explicit `_pad0` field), so the all-zero pattern is a valid value.
        unsafe { core::mem::transmute::<[u8; ZONE_INFO_V1_SIZE], Self>([0; ZONE_INFO_V1_SIZE]) }
    }
}

impl ZoneInfoParamsV1 {
    /// All-zero params with `version` set to [`ZONE_INFO_PARAMS_VER_1`].
    #[must_use]
    pub fn zeroed() -> Self {
        Self {
            version: ZONE_INFO_PARAMS_VER_1,
            num_illum_zones: 0,
            rsvd: [0; 64],
            zones: [ZoneInfoV1::zeroed(); NV_GPU_CLIENT_ILLUM_ZONE_NUM_ZONES_MAX],
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// Union accessors (crate-internal). Reads are sound because every union member is plain integer data
// (any bit pattern valid) and union values are always fully initialised (see module docs).
// ---------------------------------------------------------------------------------------------------

impl ZoneControlV1 {
    pub(crate) fn manual_rgb(&self) -> ManualRgbParams {
        // SAFETY: integer-only member of a fully initialised union (module invariant).
        unsafe { self.data.rgb.data.manual_rgb.rgb_params }
    }

    pub(crate) fn set_manual_rgb(&mut self, p: ManualRgbParams) {
        self.data.rgb.data.manual_rgb.rgb_params = p;
    }

    pub(crate) fn manual_rgbw(&self) -> ManualRgbwParams {
        // SAFETY: integer-only member of a fully initialised union (module invariant).
        unsafe { self.data.rgbw.data.manual_rgbw.rgbw_params }
    }

    pub(crate) fn set_manual_rgbw(&mut self, p: ManualRgbwParams) {
        self.data.rgbw.data.manual_rgbw.rgbw_params = p;
    }

    pub(crate) fn manual_single_color(&self) -> u8 {
        // SAFETY: integer-only member of a fully initialised union (module invariant).
        unsafe {
            self.data
                .single_color
                .data
                .manual_single_color
                .single_color_params
                .brightness_pct
        }
    }

    pub(crate) fn set_manual_single_color(&mut self, brightness_pct: u8) {
        self.data
            .single_color
            .data
            .manual_single_color
            .single_color_params
            .brightness_pct = brightness_pct;
    }

    pub(crate) fn manual_color_fixed(&self) -> u8 {
        // SAFETY: integer-only member of a fully initialised union (module invariant).
        unsafe {
            self.data
                .color_fixed
                .data
                .manual_color_fixed
                .color_fixed_params
                .brightness_pct
        }
    }

    pub(crate) fn set_manual_color_fixed(&mut self, brightness_pct: u8) {
        self.data
            .color_fixed
            .data
            .manual_color_fixed
            .color_fixed_params
            .brightness_pct = brightness_pct;
    }
}

macro_rules! opaque_union_debug {
    ($($ty:ident),* $(,)?) => {$(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // SAFETY: `rsvd` is a byte array covering the start of a fully initialised union.
                let bytes = unsafe { self.rsvd };
                f.debug_struct(stringify!($ty)).field("rsvd", &bytes).finish_non_exhaustive()
            }
        }
    )*};
}

opaque_union_debug!(
    ZoneInfoData,
    ZoneControlDataRgbUnion,
    ZoneControlDataColorFixedUnion,
    ZoneControlDataRgbwUnion,
    ZoneControlDataSingleColorUnion,
    ZoneControlData,
);

// ---------------------------------------------------------------------------------------------------
// Compile-time layout checks. Numbers derived by hand from the header (no pragma pack, natural
// alignment) and cross-checked against OpenRGB's sniffed control version 72012.
// ---------------------------------------------------------------------------------------------------

const _: () = assert!(size_of::<ZoneInfoDataReserved>() == 1);
const _: () = assert!(size_of::<ZoneInfoData>() == 64);
const _: () = assert!(size_of::<ZoneInfoV1>() == ZONE_INFO_V1_SIZE);
const _: () = assert!(offset_of!(ZoneInfoV1, illum_device_idx) == 4);
const _: () = assert!(offset_of!(ZoneInfoV1, prov_idx) == 5);
const _: () = assert!(offset_of!(ZoneInfoV1, zone_location) == 8);
const _: () = assert!(offset_of!(ZoneInfoV1, data) == 12);
const _: () = assert!(offset_of!(ZoneInfoV1, rsvd) == 76);
const _: () = assert!(size_of::<ZoneInfoParamsV1>() == ZONE_INFO_PARAMS_V1_SIZE);
const _: () = assert!(offset_of!(ZoneInfoParamsV1, zones) == 72);
const _: () = assert!(ZONE_INFO_PARAMS_VER_1 == 0x0001_11C8);

const _: () = assert!(size_of::<ManualRgbParams>() == 4);
const _: () = assert!(size_of::<ManualRgb>() == 4);
const _: () = assert!(size_of::<PiecewiseLinear>() == 20);
const _: () = assert!(offset_of!(PiecewiseLinear, grp_count) == 4);
const _: () = assert!(offset_of!(PiecewiseLinear, rise_time_ms) == 6);
const _: () = assert!(offset_of!(PiecewiseLinear, phase_offset_ms) == 16);
const _: () = assert!(size_of::<PiecewiseLinearRgb>() == 28);
const _: () = assert!(size_of::<ZoneControlDataRgbUnion>() == 64);
const _: () = assert!(size_of::<ZoneControlDataRgb>() == 128);
const _: () = assert!(size_of::<ManualColorFixed>() == 1);
const _: () = assert!(size_of::<PiecewiseLinearColorFixed>() == 24);
const _: () = assert!(offset_of!(PiecewiseLinearColorFixed, piecewise_linear_data) == 4);
const _: () = assert!(size_of::<ZoneControlDataColorFixedUnion>() == 64);
const _: () = assert!(size_of::<ZoneControlDataColorFixed>() == 128);
const _: () = assert!(size_of::<ManualRgbwParams>() == 5);
const _: () = assert!(size_of::<PiecewiseLinearRgbw>() == 32);
const _: () = assert!(offset_of!(PiecewiseLinearRgbw, piecewise_linear_data) == 12);
const _: () = assert!(size_of::<ZoneControlDataRgbwUnion>() == 64);
const _: () = assert!(size_of::<ZoneControlDataRgbw>() == 128);
const _: () = assert!(size_of::<ManualSingleColor>() == 1);
const _: () = assert!(size_of::<PiecewiseLinearSingleColor>() == 24);
const _: () = assert!(size_of::<ZoneControlDataSingleColorUnion>() == 64);
const _: () = assert!(size_of::<ZoneControlDataSingleColor>() == 128);
const _: () = assert!(size_of::<ZoneControlData>() == 128);
const _: () = assert!(size_of::<ZoneControlV1>() == ZONE_CONTROL_V1_SIZE);
const _: () = assert!(offset_of!(ZoneControlV1, ctrl_mode) == 4);
const _: () = assert!(offset_of!(ZoneControlV1, data) == 8);
const _: () = assert!(offset_of!(ZoneControlV1, rsvd) == 136);
const _: () = assert!(size_of::<ZoneControlParamsV1>() == ZONE_CONTROL_PARAMS_V1_SIZE);
const _: () = assert!(offset_of!(ZoneControlParamsV1, flags) == 4);
const _: () = assert!(offset_of!(ZoneControlParamsV1, num_illum_zones_control) == 8);
const _: () = assert!(offset_of!(ZoneControlParamsV1, rsvd) == 12);
const _: () = assert!(offset_of!(ZoneControlParamsV1, zones) == ZONE_CONTROL_PARAMS_V1_HEADER_SIZE);
const _: () = assert!(ZONE_CONTROL_PARAMS_VER_1 == 72012);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_and_versions() {
        let rows = [
            ("NV_GPU_CLIENT_ILLUM_ZONE_INFO_V1", size_of::<ZoneInfoV1>()),
            (
                "NV_GPU_CLIENT_ILLUM_ZONE_INFO_PARAMS_V1",
                size_of::<ZoneInfoParamsV1>(),
            ),
            ("NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_V1", size_of::<ZoneControlV1>()),
            (
                "NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1",
                size_of::<ZoneControlParamsV1>(),
            ),
        ];
        for (name, size) in rows {
            println!("{name}: {size} bytes");
        }
        println!("ZONE_INFO_PARAMS_VER_1 = {ZONE_INFO_PARAMS_VER_1} (0x{ZONE_INFO_PARAMS_VER_1:08X})");
        println!(
            "ZONE_CONTROL_PARAMS_VER_1 = {ZONE_CONTROL_PARAMS_VER_1} (0x{ZONE_CONTROL_PARAMS_VER_1:08X})"
        );
        assert_eq!(rows[0].1, 140);
        assert_eq!(rows[1].1, 4552);
        assert_eq!(rows[2].1, 200);
        assert_eq!(rows[3].1, 6476);
        assert_eq!(ZONE_CONTROL_PARAMS_VER_1, 72012, "OpenRGB's sniffed version");
        assert_eq!(ZONE_CONTROL_PARAMS_VER_1, (1 << 16) | 6476);
        assert_eq!(ZONE_INFO_PARAMS_VER_1, (1 << 16) | 4552);
        assert_eq!(make_nvapi_version(0x194C, 2), 0x0002_194C);
        assert_eq!(ZoneControlParamsV1::zeroed().version, 72012);
        assert_eq!(ZoneInfoParamsV1::zeroed().version, 70088);
    }

    #[test]
    fn status_names() {
        let expected = [
            (0, "NVAPI_OK"),
            (-1, "NVAPI_ERROR"),
            (-2, "NVAPI_LIBRARY_NOT_FOUND"),
            (-3, "NVAPI_NO_IMPLEMENTATION"),
            (-4, "NVAPI_API_NOT_INITIALIZED"),
            (-5, "NVAPI_INVALID_ARGUMENT"),
            (-6, "NVAPI_NVIDIA_DEVICE_NOT_FOUND"),
            (-9, "NVAPI_INCOMPATIBLE_STRUCT_VERSION"),
            (-104, "NVAPI_NOT_SUPPORTED"),
        ];
        for (code, name) in expected {
            assert_eq!(NvStatus(code).name(), name);
        }
        assert_eq!(NvStatus(-9999).name(), "NVAPI_UNKNOWN_STATUS");
        assert!(NvStatus::OK.is_ok());
        assert!(!NvStatus::ERROR.is_ok());
        assert_eq!(NvStatus::NOT_SUPPORTED.to_string(), "NVAPI_NOT_SUPPORTED(-104)");
    }

    #[test]
    fn manual_param_offsets_inside_zone() {
        let mut z = ZoneControlV1::zeroed();
        z.zone_type = ZONE_TYPE_RGBW;
        z.set_manual_rgbw(ManualRgbwParams {
            color_r: 1,
            color_g: 2,
            color_b: 3,
            color_w: 4,
            brightness_pct: 5,
        });
        z.rsvd[0] = 0xEE;
        let b = z.to_bytes();
        assert_eq!(&b[0..4], &[3, 0, 0, 0]);
        assert_eq!(&b[8..13], &[1, 2, 3, 4, 5]);
        assert_eq!(b[136], 0xEE);
        assert_eq!(ZoneControlV1::from_bytes(&b), z);
        assert_eq!(z.manual_rgbw().color_w, 4);

        let mut z = ZoneControlV1::zeroed();
        z.set_manual_rgb(ManualRgbParams {
            color_r: 9,
            color_g: 8,
            color_b: 7,
            brightness_pct: 6,
        });
        assert_eq!(&z.to_bytes()[8..12], &[9, 8, 7, 6]);
        z.set_manual_single_color(0x42);
        assert_eq!(z.to_bytes()[8], 0x42);
        assert_eq!(
            z.manual_color_fixed(),
            0x42,
            "both brightness bytes alias offset 8"
        );
    }

    #[test]
    fn params_bytes_round_trip() {
        let mut bytes = [0u8; ZONE_CONTROL_PARAMS_V1_SIZE];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from((i * 31 + 7) % 256).expect("value below 256");
        }
        let p = ZoneControlParamsV1::from_bytes(&bytes);
        assert_eq!(p.to_bytes(), bytes);
        assert_eq!(p.version, u32::from_le_bytes([7, 38, 69, 100]));
        assert_eq!(
            p.zones[0].zone_type,
            u32::from_le_bytes([bytes[76], bytes[77], bytes[78], bytes[79]])
        );
    }
}
