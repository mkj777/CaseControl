# Brief: `cc-drivers::wooting` (Phase 2)

Read `00-common-rules.md` first (driver contract). Family id `cc_core::family::WOOTING`.

## Target

Wooting 60HE (ARM), `31E3:1310`, V3 protocol. Reference: `reference/openrgb/WootingKeyboardController/*`, in
particular `WootingV3KeyboardController` and `RGBController_WootingKeyboard` (layout, key names, matrix size for the
60% board), `WootingKeyboardControllerDetect.cpp` (vid/pid, interface, usage page 0xFF55, how the 60HE ARM is typed).

## Requirements

- Detect exactly as the reference does for this PID. Serial from enumeration if present.
- Matrix zone with the 60% key map from the reference (`ZoneKind::Matrix` with `map` of `Option<u32>` LED indices),
  LED names = key names.
- Direct per-key RGB using the reference's V3 commands (including any init / "activate RGB control" and colour
  encoding the V3 controller uses). `set_direct` sends one full frame the way the reference does.
- `release()` hands control back to the keyboard's own profile using the reference's reset/restore command(s).
  If the reference has no explicit release, find the command it sends on close or for "reset to profile" and use it;
  report which one.
- The keyboard is the owner's daily keyboard: nothing in this driver may change key mappings, actuation, or profiles
  stored on the keyboard. Lighting commands only.
- No hardware modes unless the reference exposes them for V3; direct mode is enough.

## Tests

Byte-exact for init, one full frame (first and last key colours visible in the bytes), release. Key map shape (count of
mapped cells equals LED count; no index twice).
