// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! The Windows implementation of [`NvApi`]: dynamic loading of `nvapi64.dll` and the safe calls.

use core::ffi::{c_char, c_void};
use core::mem::size_of;
use std::cell::Cell;
use std::time::Instant;

use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::core::{PCWSTR, s, w};

use crate::ILLUM_CALL_GAP;
use crate::error::{NvApiError, check};
use crate::model::{GpuHandle, PciIds, ZoneControlParams, ZoneInfo};
use crate::raw::{self, ids};

#[cfg(target_pointer_width = "64")]
const DLL_NAME: PCWSTR = w!("nvapi64.dll");
#[cfg(not(target_pointer_width = "64"))]
const DLL_NAME: PCWSTR = w!("nvapi.dll");

/// Owns the module handle and frees it on drop.
struct Library(HMODULE);

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: self.0 came from a successful LoadLibraryExW and is freed exactly once, here. No
        // function pointer resolved from it outlives the owning NvApi (fields drop after NvApi::drop).
        if let Err(e) = unsafe { FreeLibrary(self.0) } {
            tracing::warn!(error = %e, "FreeLibrary(nvapi64.dll) failed");
        }
    }
}

struct Fns {
    unload: raw::UnloadFn,
    enum_physical_gpus: raw::EnumPhysicalGpusFn,
    get_full_name: raw::GpuGetFullNameFn,
    get_pci_identifiers: raw::GpuGetPciIdentifiersFn,
    zones_get_control: raw::GpuClientIllumZonesGetControlFn,
    zones_set_control: raw::GpuClientIllumZonesSetControlFn,
}

/// Resolves one interface id to a function pointer of type `F`.
///
/// # Safety
/// `qi` must be the live `nvapi_QueryInterface` of a loaded `nvapi64.dll`, and `F` must be the
/// `extern "C"` function pointer type that matches the function behind `id`.
unsafe fn resolve<F: Copy>(qi: raw::QueryInterfaceFn, id: u32) -> Result<F, NvApiError> {
    const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
    // SAFETY: qi is a valid nvapi_QueryInterface (caller contract); it takes any u32 and returns null
    // for unknown ids.
    let ptr = unsafe { qi(id) };
    if ptr.is_null() {
        return Err(NvApiError::SymbolNotFound(id));
    }
    tracing::trace!(id = format_args!("{id:#010x}"), ?ptr, "resolved NvAPI interface");
    // SAFETY: ptr is a non-null code address for `id`; F is a pointer-sized fn pointer type matching it
    // (caller contract, size asserted above).
    Ok(unsafe { core::mem::transmute_copy::<*mut c_void, F>(&ptr) })
}

/// A loaded and initialised NvAPI.
///
/// Created by [`NvApi::load`]; dropping it calls `NvAPI_Unload` and frees the library.
///
/// # Threading
/// `NvApi` is `Send`: the module handle and the resolved function pointers are process-global and valid
/// on any thread, and NvAPI's entry points are plain free functions with no thread affinity. It is
/// deliberately **not** `Sync` (it holds a `Cell` for call pacing): nothing in the reference sources
/// shows the illumination calls are safe to run concurrently (OpenRGB serialises them and even
/// rate-limits them), so concurrent `&NvApi` use from several threads is ruled out by the type system.
/// Wrap it in a `Mutex` to share it.
///
/// # Pacing
/// Illumination calls (`illum_zones_*`) are spaced at least [`ILLUM_CALL_GAP`] apart, sleeping the
/// calling thread if needed, mirroring OpenRGB's 30 ms sleep after each such call.
pub struct NvApi {
    fns: Fns,
    last_illum_call: Cell<Option<Instant>>,
    lib: Library,
}

// SAFETY: see the "Threading" section above. HMODULE is a process-wide handle that may be used and
// freed from any thread, the fn pointers are plain code addresses, and Cell<Option<Instant>> is Send.
// The type stays !Sync through the Cell.
unsafe impl Send for NvApi {}

impl core::fmt::Debug for NvApi {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NvApi")
            .field("module", &self.lib.0.0)
            .finish_non_exhaustive()
    }
}

impl NvApi {
    /// Loads `nvapi64.dll` from `System32`, resolves every function this crate uses and calls
    /// `NvAPI_Initialize`.
    ///
    /// # Errors
    /// - [`NvApiError::LibraryNotFound`] if the DLL is absent (no NVIDIA driver).
    /// - [`NvApiError::Unsupported`] if the DLL does not export `nvapi_QueryInterface`.
    /// - [`NvApiError::SymbolNotFound`] if the driver lacks one of the needed functions (e.g. an old
    ///   driver without the illumination API).
    /// - [`NvApiError::Status`] if `NvAPI_Initialize` fails (e.g. no NVIDIA GPU present).
    pub fn load() -> Result<Self, NvApiError> {
        // SAFETY: DLL_NAME is a valid NUL-terminated wide string constant. LOAD_LIBRARY_SEARCH_SYSTEM32
        // restricts the search to System32, where the NVIDIA driver installs nvapi64.dll, so a planted
        // DLL in the working or application directory cannot be picked up.
        let module =
            unsafe { LoadLibraryExW(DLL_NAME, None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.map_err(|e| {
                tracing::debug!(error = %e, "LoadLibraryExW(nvapi64.dll) failed");
                NvApiError::LibraryNotFound
            })?;
        let lib = Library(module);

        // SAFETY: lib.0 is a valid module handle; the name is a NUL-terminated ANSI string constant.
        let proc = unsafe { GetProcAddress(lib.0, s!("nvapi_QueryInterface")) }.ok_or_else(|| {
            NvApiError::Unsupported("nvapi64.dll does not export nvapi_QueryInterface".into())
        })?;
        // SAFETY: nvapi_QueryInterface has the C signature `void* (*)(unsigned int)` (nvapi.cpp), which
        // QueryInterfaceFn spells out; both are plain code pointers and x64 has a single calling
        // convention, so reinterpreting the FARPROC is exact.
        let qi = unsafe {
            core::mem::transmute::<unsafe extern "system" fn() -> isize, raw::QueryInterfaceFn>(proc)
        };

        // SAFETY: (applies to every resolve call below) qi is the live nvapi_QueryInterface of `lib`, and each target type
        // is the fn pointer type declared for that id in raw.rs, copied from nvapi.h/nvapi.cpp.
        let initialize: raw::InitializeFn = unsafe { resolve(qi, ids::INITIALIZE) }?;
        let fns = Fns {
            // SAFETY: see above.
            unload: unsafe { resolve(qi, ids::UNLOAD) }?,
            // SAFETY: see above.
            enum_physical_gpus: unsafe { resolve(qi, ids::ENUM_PHYSICAL_GPUS) }?,
            // SAFETY: see above.
            get_full_name: unsafe { resolve(qi, ids::GPU_GET_FULL_NAME) }?,
            // SAFETY: see above.
            get_pci_identifiers: unsafe { resolve(qi, ids::GPU_GET_PCI_IDENTIFIERS) }?,
            // SAFETY: see above.
            zones_get_control: unsafe { resolve(qi, ids::GPU_CLIENT_ILLUM_ZONES_GET_CONTROL) }?,
            // SAFETY: see above.
            zones_set_control: unsafe { resolve(qi, ids::GPU_CLIENT_ILLUM_ZONES_SET_CONTROL) }?,
        };

        // SAFETY: initialize is NvAPI_Initialize from the loaded library; it takes no arguments.
        check(unsafe { initialize() }, "NvAPI_Initialize")?;
        tracing::debug!("NvAPI initialised");

        Ok(Self {
            fns,
            last_illum_call: Cell::new(None),
            lib,
        })
    }

    /// Enumerates the physical NVIDIA GPUs. An empty list (not an error) if the driver reports
    /// `NVAPI_NVIDIA_DEVICE_NOT_FOUND`.
    ///
    /// # Errors
    /// [`NvApiError::Status`] for any other failure of `NvAPI_EnumPhysicalGPUs`.
    pub fn physical_gpus(&self) -> Result<Vec<GpuHandle>, NvApiError> {
        let mut handles: [raw::NvPhysicalGpuHandle; raw::NVAPI_MAX_PHYSICAL_GPUS] =
            [core::ptr::null_mut(); raw::NVAPI_MAX_PHYSICAL_GPUS];
        let mut count: i32 = 0;
        // SAFETY: the driver writes at most NVAPI_MAX_PHYSICAL_GPUS (64) handles into the array (the size
        // OpenRGB passes) and one i32 into `count`; both live for the duration of the call.
        let status = unsafe { (self.fns.enum_physical_gpus)(handles.as_mut_ptr(), &raw mut count) };
        if status == raw::NvStatus::NVIDIA_DEVICE_NOT_FOUND {
            return Ok(Vec::new());
        }
        check(status, "NvAPI_EnumPhysicalGPUs")?;
        let count = usize::try_from(count)
            .unwrap_or(0)
            .min(raw::NVAPI_MAX_PHYSICAL_GPUS);
        Ok(handles[..count]
            .iter()
            .map(|h| GpuHandle(h.expose_provenance()))
            .collect())
    }

    /// PCI identifiers of a GPU, see [`PciIds`] for the packing.
    ///
    /// # Errors
    /// [`NvApiError::Status`] if `NvAPI_GPU_GetPCIIdentifiers` fails (e.g. a stale handle).
    pub fn pci_ids(&self, gpu: &GpuHandle) -> Result<PciIds, NvApiError> {
        let (mut device_id, mut subsystem_id, mut revision_id, mut ext_device_id) = (0, 0, 0, 0);
        // SAFETY: the handle is passed through opaquely (NvAPI validates it); the four out-pointers are
        // valid, distinct u32 locations for the duration of the call.
        let status = unsafe {
            (self.fns.get_pci_identifiers)(
                handle_ptr(gpu),
                &raw mut device_id,
                &raw mut subsystem_id,
                &raw mut revision_id,
                &raw mut ext_device_id,
            )
        };
        check(status, "NvAPI_GPU_GetPCIIdentifiers")?;
        Ok(PciIds {
            device_id,
            subsystem_id,
            revision_id,
            ext_device_id,
        })
    }

    /// Marketing name of a GPU, e.g. `"NVIDIA GeForce RTX 5080"`.
    ///
    /// # Errors
    /// [`NvApiError::Status`] if `NvAPI_GPU_GetFullName` fails.
    pub fn full_name(&self, gpu: &GpuHandle) -> Result<String, NvApiError> {
        let mut buf: raw::NvShortString = [0; raw::NV_SHORT_STRING_LEN];
        // SAFETY: buf is an NV_SHORT_STRING (64 chars), exactly what the function writes into.
        let status = unsafe { (self.fns.get_full_name)(handle_ptr(gpu), buf.as_mut_ptr()) };
        check(status, "NvAPI_GPU_GetFullName")?;
        Ok(short_string(&buf))
    }

    /// The illumination zones of a GPU.
    ///
    /// Documented deviation: derived from `ClientIllumZonesGetControl` (current values), exactly as
    /// OpenRGB's `getInfo()` does, because no function id for `NvAPI_GPU_ClientIllumZonesGetInfo` exists
    /// in the reference sources. Index and type are filled; `location` and device/provider index are
    /// therefore always `None`.
    ///
    /// # Errors
    /// As [`NvApi::illum_zones_get_control`].
    pub fn illum_zones_info(&self, gpu: &GpuHandle) -> Result<Vec<ZoneInfo>, NvApiError> {
        let params = self.illum_zones_get_control(gpu, false)?;
        Ok(ZoneInfo::list_from_control(&params))
    }

    /// Reads the zone control params: the currently active values (`default_values == false`) or the
    /// driver defaults (`true`). Always call this before any [`NvApi::illum_zones_set_control`] and keep
    /// the result to restore on release.
    ///
    /// # Errors
    /// [`NvApiError::Status`] if the call fails, [`NvApiError::Unsupported`] if the driver returns a
    /// struct this binding cannot represent.
    pub fn illum_zones_get_control(
        &self,
        gpu: &GpuHandle,
        default_values: bool,
    ) -> Result<ZoneControlParams, NvApiError> {
        let mut params = raw::ZoneControlParamsV1::zeroed();
        if default_values {
            params.flags = raw::CONTROL_FLAG_DEFAULT;
        }
        self.pace();
        // SAFETY: params is a zero-initialised NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1 (6476 bytes,
        // size asserted) with version 72012, which the driver fills in place.
        let status = unsafe { (self.fns.zones_get_control)(handle_ptr(gpu), &raw mut params) };
        self.mark_illum_call();
        check(status, "NvAPI_GPU_ClientIllumZonesGetControl")?;
        ZoneControlParams::from_raw(&params)
    }

    /// Writes zone control params. The bytes sent are exactly [`ZoneControlParams::to_raw`], including
    /// the `bDefault` bit as stored in `params.default_values` (params obtained with
    /// `default_values == false` are sent back as "active values").
    ///
    /// # Errors
    /// [`NvApiError::Unsupported`] if `params` cannot be encoded, [`NvApiError::Status`] if the call fails.
    pub fn illum_zones_set_control(
        &self,
        gpu: &GpuHandle,
        params: &ZoneControlParams,
    ) -> Result<(), NvApiError> {
        let mut raw_params = params.to_raw()?;
        self.pace();
        // SAFETY: raw_params is a fully initialised NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1 with the
        // correct version; the driver only reads it (the header takes a non-const pointer).
        let status = unsafe { (self.fns.zones_set_control)(handle_ptr(gpu), &raw mut raw_params) };
        self.mark_illum_call();
        check(status, "NvAPI_GPU_ClientIllumZonesSetControl")
    }

    fn pace(&self) {
        if let Some(last) = self.last_illum_call.get() {
            let elapsed = last.elapsed();
            if elapsed < ILLUM_CALL_GAP {
                std::thread::sleep(ILLUM_CALL_GAP - elapsed);
            }
        }
    }

    fn mark_illum_call(&self) {
        self.last_illum_call.set(Some(Instant::now()));
    }
}

impl Drop for NvApi {
    fn drop(&mut self) {
        // SAFETY: NvAPI_Unload from the still-loaded library (self.lib is freed after this body runs).
        let status = unsafe { (self.fns.unload)() };
        if !status.is_ok() {
            tracing::warn!(%status, "NvAPI_Unload failed");
        }
    }
}

fn handle_ptr(gpu: &GpuHandle) -> raw::NvPhysicalGpuHandle {
    core::ptr::with_exposed_provenance_mut(gpu.0)
}

/// Converts a NUL-terminated `NV_SHORT_STRING` to a `String` (lossy UTF-8, bounded at 64 chars).
fn short_string(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c.to_ne_bytes()[0])
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_stops_at_nul_and_bounds() {
        let mut buf: raw::NvShortString = [0; raw::NV_SHORT_STRING_LEN];
        for (dst, src) in buf.iter_mut().zip(b"NVIDIA GeForce RTX 5080") {
            *dst = c_char::from_ne_bytes([*src]);
        }
        assert_eq!(short_string(&buf), "NVIDIA GeForce RTX 5080");
        let full: raw::NvShortString = [c_char::from_ne_bytes(*b"x"); raw::NV_SHORT_STRING_LEN];
        assert_eq!(short_string(&full).len(), 64);
    }

    #[test]
    fn nvapi_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<NvApi>();
    }
}
