# Brief: `crates/cc-nvapi` (Phase 1)

You own `crates/cc-nvapi/**`. Stub `Cargo.toml` has `thiserror`, `tracing`, and on Windows `windows` with
`Win32_Foundation` and `Win32_System_LibraryLoader` (extend as needed). `#![allow(unsafe_code)]` is set.

## Goal

A safe Rust wrapper over the undocumented NvAPI functions OpenRGB uses for GPU illumination, loaded dynamically from
`nvapi64.dll` at runtime (the DLL ships with the NVIDIA driver; the app must still start when it is missing).
Target device on the development machine: GeForce RTX 5080 Founders Edition, PCI `10DE:2C02`, subsystem `10DE:2095`,
handled upstream by `NVIDIA_ILLUMINATION_V1`.

Primary sources, in this order:
1. `reference/openrgb/NVFC/nvapi.h` and `nvapi.cpp`: `nvapi_QueryInterface`, the function ids, the status enum, the
   `MAKE_NVAPI_VERSION` macro, every struct (`NV_GPU_CLIENT_ILLUM_ZONE_INFO_PARAMS_V1`, `NV_GPU_CLIENT_ILLUM_ZONE_CONTROL_PARAMS_V1`
   and the zone/data unions they contain).
2. `reference/openrgb/NVIDIAIlluminationController/*` for how the calls are sequenced and how RGB vs RGBW zones are treated.
Copy function ids, enum values and struct layouts from these files byte for byte. Do not type them from memory.

## Contracts

- `raw` module (`#[cfg(windows)]`): `#[repr(C)]` structs and unions mirroring the header exactly. Check the header for
  `#pragma pack` directives around these structs first; if a struct is packed, use `#[repr(C, packed(N))]` and say so in
  a comment, because a silent padding mismatch is the most likely way the size asserts fail. Add
  `const _: () = assert!(size_of::<T>() == N)` for every struct where the header fixes the size (derive `N` from the
  header's `MAKE_NVAPI_VERSION(T, v)` usage and padding rules; document each number). Function pointer types with the
  correct calling convention (`extern "C"` on x64, as the header's `NVAPI_INTERFACE` resolves). Status codes as an
  `i32` newtype with a `name()` table for the values the header defines.
- `NvApi` (safe API): `NvApi::load() -> Result<NvApi, NvApiError>` (LoadLibraryW `nvapi64.dll`, GetProcAddress
  `nvapi_QueryInterface`, resolve every needed function id eagerly, call `NvAPI_Initialize`), `Drop` calls `NvAPI_Unload`
  and frees the library. Methods: `physical_gpus() -> Result<Vec<GpuHandle>>`, `pci_ids(&GpuHandle) -> Result<PciIds { device_id: u32, subsystem_id: u32, revision_id: u32, ext_device_id: u32 }>`
  (document that NvAPI packs vendor in the high/low 16 bits exactly as the header says), `full_name(&GpuHandle) -> Result<String>`,
  `illum_zones_info(&GpuHandle) -> Result<Vec<ZoneInfo>>` (zone type RGB/RGBW/SingleColor/ColorFixed, location, index),
  `illum_zones_get_control(&GpuHandle, default_values: bool) -> Result<ZoneControlParams>` and
  `illum_zones_set_control(&GpuHandle, &ZoneControlParams) -> Result<()>`, where `ZoneControlParams` is a safe owned
  representation (`Vec<ZoneControl>` with `ZoneControl { kind, manual: ZoneColor { Rgb{r,g,b,brightness} | Rgbw{..} | Single{brightness} } }`)
  plus `to_raw()`/`from_raw()` conversions that keep the untouched raw bytes for zones we do not understand
  (round-trip safety: GetControl -> SetControl with no changes must send the same bytes).
- `NvApiError` (thiserror): `LibraryNotFound`, `SymbolNotFound(u32)`, `Status { code: i32, name: &'static str, call: &'static str }`,
  `Unsupported(String)`.
- `NvApi: Send` (document why; keep `!Sync` unless you can show NvAPI is thread-safe for these calls).
- On non-Windows compile to a stub whose `load()` returns `Unsupported`.

## Rules from the architecture

Always `GetControl` before any `SetControl`; `release()` in the driver (later phase) writes the initial params back,
so `ZoneControlParams` must be `Clone` and faithfully round-trippable. Never call `SetControl` from this crate's tests.

## Tests (no DLL calls)

- Struct size and `MAKE_NVAPI_VERSION` asserts (compile-time plus a unit test printing the numbers).
- Status code name table covers at least OK, ERROR, LIBRARY_NOT_FOUND, NO_IMPLEMENTATION, API_NOT_INITIALIZED,
  INVALID_ARGUMENT, NVIDIA_DEVICE_NOT_FOUND, INCOMPATIBLE_STRUCT_VERSION, NOT_SUPPORTED, and the illumination-specific ones
  if the header has them.
- `ZoneControlParams::from_raw(to_raw(x)) == x` on a hand-built params struct with one RGB and one RGBW zone.
- PCI id unpacking from the packed `u32`s.

## Example (not run in CI)

`examples/dump_illum.rs`: loads NvAPI, lists GPUs with PCI ids and names, prints zone info and the current and default
control params as a hex dump and decoded. Read-only; it must not call `SetControl`. The orchestrator runs it on the
RTX 5080 and the output becomes the first fixture for the driver in Phase 3.
