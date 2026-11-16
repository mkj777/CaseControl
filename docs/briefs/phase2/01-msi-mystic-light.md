# Brief: `cc-drivers::msi_mystic_light` (Phase 2)

Read `00-common-rules.md` first (driver contract). Family id `cc_core::family::MSI_MYSTIC_LIGHT`.

## Target

MSI MAG B650 TOMAHAWK WIFI (MS-7D75), Mystic Light HID controller `1462:7D75`. This is the first device that will ever
receive writes from CaseControl, and a motherboard controller: be conservative.

Reference: `reference/openrgb/MSIMotherboardController/*` (detect, 185/162/112/761 controllers, `MSIMotherboardCommon.h`).
Research: `docs/research/` (search for 7D75 / Mystic Light). Architecture table row `msi_mystic_light`.

## Requirements

- Detect on `1462:7D75` with the usage page / usage the reference detector uses. Keep the reference's supported-PID
  list for the class you port; never enable untested PIDs (no `ENABLE_UNTESTED` equivalent). Identify the controller
  class by `get_feature_report(0x52)` length exactly as the reference does; port the classes the reference supports
  for this board's answer, and fail detection with `Detected::Failed` for any other length.
- Port the firmware-version gate (`GetFirmwareVersionInvalid` or whatever the reference calls it) verbatim.
- Read the current configuration (feature report 0x52) on open and keep the bytes. `release()` sends that
  configuration back with the save-to-flash byte forced to zero. Document what happens if the stored config was itself
  a direct/per-LED state left by another program.
- Zones and LEDs as the reference builds them for this board (onboard zones, JRGB, JRAINBOW headers, and whatever the
  class exposes). If the reference allows resizing the ARGB header zones, implement `resize_zone` with its limits.
- Direct mode exactly as the reference's per-LED path (feature reports, packet layout, any required mode switch).
  Hardware modes (static, breathing, flashing, rainbow, ...) with speed/brightness/colors as the reference exposes them.
- `save_to_hardware()` is the only path that sets the save byte.

## Tests

Byte-exact packet tests for: initial mode switch to direct, one direct frame for every zone, a hardware mode packet,
release packet (save byte zero, bytes otherwise identical to the read config). Detection class selection from report
lengths. FakeTransport sequences per the common contract.
