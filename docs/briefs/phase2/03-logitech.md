# Brief: `cc-drivers::logitech` (Phase 2)

Read `00-common-rules.md` first (driver contract). Family id `cc_core::family::LOGITECH`.

## Targets

1. Logitech G560 speaker, `046D:0A78`, HID++ 2.0 on usage page 0xFF43, zone-only lighting (feature 0x8070 / RGB
   effects). Reports serial `00000000` (counts as absent; see `DeviceId` rules).
2. Logitech PRO Wireless mouse through the Lightspeed receiver `046D:C539` (wireless PID 0x4079). The receiver allows
   one connection; device index handling as upstream.

Reference: `reference/openrgb/LogitechController/LogitechControllerDetect.cpp`, `LogitechProtocolCommon.*`,
`LogitechHIDPP20Controller/*` (hidpp20 feature discovery, 0x8070/0x8071), `LogitechLightspeedController/*`.

## Requirements

- Detect both exactly as the reference does (which collections/usage pages are opened for short/long HID++ reports).
  Feature discovery (root feature 0x0000 lookups) is allowed during detection because the reference does it.
- Lightspeed: the mouse id must stay stable across runs. Use `DeviceId::hid(0x046D, <wireless pid>, <serial if the
  reference reads one>)`. Because that id is only known after talking to the receiver, keep a map inside the backend
  from receiver path to the ids it produced, and before any I/O on a receiver whose ids are all in `ctx.skip`, skip it.
  A mouse that is switched off (receiver present, no device) yields no device and no error.
- Zones: what the reference builds (G560: its lighting zones; PRO Wireless: logo/primary zones). Direct mode = the
  reference's static/direct colour per zone. Hardware modes the reference exposes (static, cycle, breathing) with their
  parameters.
- Keepalive: if the reference or the HID++ feature needs software control refreshed, set `keepalive` and implement it.
- `release()`: give lighting back to the device (onboard/firmware effect) the way the reference or HID++ 0x8070/0x8071
  allows (e.g. leave SW control / restore onboard mode). Report precisely what you send.
- Never touch DPI, buttons, onboard profiles, or pairing.
- G HUB and `logi_lamparray_service` may run concurrently on the owner's machine; read replies must be matched to
  your request (device index, feature index, function id, software id) and stray reports ignored, so another
  program's traffic cannot confuse the driver. Bounded reads with timeouts.

## Tests

Byte-exact for feature lookups, set-colour per zone, mode packets, release; reply-matching with interleaved foreign
reports in a FakeTransport script; Lightspeed skip logic.
