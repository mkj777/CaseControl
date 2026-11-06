#![allow(unsafe_code)]
//! CaseControl NvAPI binding: dynamic `nvapi64.dll` loading and the GPU illumination API.
//!
//! # What it does
//!
//! - Loads `nvapi64.dll` (shipped with the NVIDIA display driver) **at runtime** from `System32` only, resolves
//!   the needed functions through `nvapi_QueryInterface` and calls `NvAPI_Initialize`
//!   ([`NvApi::load`]). A missing DLL is an ordinary error ([`NvApiError::LibraryNotFound`]), so the app
//!   starts fine without an NVIDIA driver.
//! - Enumerates physical GPUs, reads their PCI ids and names, and reads/writes the undocumented
//!   "client illumination zones" control struct that OpenRGB's `NVIDIA_ILLUMINATION_V1` controller uses
//!   (RTX 2000 to 5000 Founders Editions and some partner cards).
//! - [`raw`] mirrors the C ABI byte for byte (`#[repr(C)]`, size/offset asserts, ids copied from OpenRGB's
//!   `NVFC/nvapi.h` and `nvapi.cpp`). [`ZoneControlParams`] is the safe owned form; it round-trips every
//!   byte it does not understand, so `GetControl -> SetControl` without changes sends identical bytes.
//!
//! # What it deliberately does not do
//!
//! - No device policy: no PCI-id matching, no RGBW-vs-RGB color heuristics, no effects. That is the
//!   `nvidia_illumination` driver in `cc-drivers`.
//! - No `SetControl` on its own initiative, and no tests that touch the DLL. Callers must `GetControl`
//!   before any `SetControl` and keep the initial params to write back on release.
//! - No GPU I2C (`NvAPI_I2C*`), clocks, fans or other NvAPI areas.
//! - Windows only. On other platforms [`NvApi::load`] returns [`NvApiError::Unsupported`].
//!
//! # Documented deviations
//!
//! - **Zone info from `GetControl`.** No function id for `NvAPI_GPU_ClientIllumZonesGetInfo` exists in the
//!   reference sources, so [`NvApi::illum_zones_info`] derives the zones from `ClientIllumZonesGetControl`,
//!   exactly as OpenRGB does. It fills index and type and always returns `location = None` (and no
//!   device/provider index).
//! - **`LoadLibraryExW` instead of `LoadLibraryW`.** The DLL is loaded with
//!   `LoadLibraryExW(.., LOAD_LIBRARY_SEARCH_SYSTEM32)` as hardening against DLL planting: a
//!   `nvapi64.dll` next to the executable or in the current directory is never picked up.
//! - **Cross-platform `raw` and model.** [`raw`] and the model types compile on every platform, and the
//!   non-Windows `NvApi` stub has the same API surface (including [`ILLUM_CALL_GAP`]), so the conversion
//!   tests run anywhere.
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> Result<(), cc_nvapi::NvApiError> {
//! let nvapi = cc_nvapi::NvApi::load()?;
//! for gpu in nvapi.physical_gpus()? {
//!     let ids = nvapi.pci_ids(&gpu)?;
//!     let initial = nvapi.illum_zones_get_control(&gpu, false)?;
//!     println!("{:04X}:{:04X} has {} zones", ids.vendor(), ids.device(), initial.zones.len());
//! }
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

mod error;
mod model;
pub mod raw;

#[cfg(not(windows))]
mod stub;
#[cfg(windows)]
mod win;

pub use error::NvApiError;
pub use model::{
    CtrlMode, GpuHandle, PciIds, ZoneColor, ZoneControl, ZoneControlParams, ZoneInfo, ZoneKind, ZoneLocation,
};
#[cfg(not(windows))]
pub use stub::NvApi;
#[cfg(windows)]
pub use win::NvApi;

/// Minimum gap between two illumination calls. OpenRGB's `nvapi_accessor` sleeps 30 ms after every
/// `ClientIllumZonesGetControl`/`SetControl` because "the NvAPI doesn't like to be spammed calls or else it
/// just ignores them". Defined on every platform so the API surface is identical.
pub const ILLUM_CALL_GAP: Duration = Duration::from_millis(30);
