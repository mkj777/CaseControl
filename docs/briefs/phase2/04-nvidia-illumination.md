# Brief: `cc-drivers::nvidia_illumination` (Phase 2)

Read `00-common-rules.md` first (driver contract). Family id `cc_core::family::NVIDIA_ILLUMINATION`.

## Target

GeForce RTX 5080 Founders Edition: PCI `10DE:2C02`, subsystem `10DE:2095`. `dump_illum` on this machine found three
SingleColor zones (brightness only; reported brightness 50/60/60). Use `cc-nvapi` (`NvApi::load`, `physical_gpus`,
`pci_ids`, `illum_zones_info`, `illum_zones_get_control`, `illum_zones_set_control`, `ILLUM_CALL_GAP`). Read its rustdoc
and `crates/cc-nvapi/examples/dump_illum.rs` first.

Reference: `reference/openrgb/NVIDIAIlluminationController/*` (detect table, V1 controller, how SingleColor / RGB /
RGBW zones map to OpenRGB colours and brightness).

## Requirements

- `bus()` is `Bus::NvApi`; `DeviceId::nvapi(device_id, subsystem_id)`; location = PCI location if NvAPI gives one,
  else "NvAPI GPU <index>". If `nvapi64.dll` is missing or `load()` fails, detection returns nothing and logs at info.
- Match the reference's detect table entries for Illumination V1 that include `10DE:2C02/10DE:2095`; port the whole
  V1 table only if it is a plain list, otherwise just this card.
- On open: `GetControl` (current, not default) and keep it. `release()` writes that back. Never `SetControl` before a
  successful `GetControl`.
- Zones and colour mapping as the reference does: RGB zones take colours, RGBW as RGBW, SingleColor zones map the
  colour to brightness exactly as the reference computes it. Respect `ILLUM_CALL_GAP` between NvAPI calls (sleep only
  the remaining gap).
- Modes as the reference exposes (direct/static and any others it offers for V1).
- `set_direct` must skip the SetControl call when the resulting params equal the last params sent.
- Thread safety: `NvApi` is `Send`, `!Sync`; each controller owns its handle (load per controller or share through a
  mutex, your choice, documented).

## Tests (no DLL)

Colour-to-params mapping for each zone type, from-current-params round trip, "no change, no call" logic behind a small
trait so it can run with a fake. The demo example must first do a pure round-trip (GetControl, SetControl the same
params, GetControl, compare) and abort unless identical, before any colour change.
