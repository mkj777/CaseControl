# Brief: `crates/cc-core` (Phase 1)

You own `crates/cc-core/**`. The crate exists as a stub with its `Cargo.toml` (serde, serde_json, bitflags, thiserror).

## Goal

The device model and the traits everything else implements, exactly as sketched in `docs/architecture.md`
section "Core model". No I/O, no Windows dependencies, no `cfg(windows)`: this crate must compile and test on any OS.
Every public data type derives `Debug, Clone, PartialEq, Serialize, Deserialize` (plus `Eq, Hash` where sensible)
because the same types cross the Tauri IPC boundary as JSON and are stored in TOML profiles.

## Modules and contracts

- `color`: `Rgb { r, g, b }`. Serialize as the string `"#RRGGBB"` (upper-case hex), deserialize from `#RRGGBB`,
  `#RGB`, and `RRGGBB`; implement `FromStr`, `Display`, `From<[u8; 3]>`, `From<u32>` (0x00RRGGBB), `const fn new`,
  `BLACK`, `WHITE`. Keep the OpenRGB packing helpers out; drivers do their own byte order.
- `id`: `DeviceId(String)` with the rule from the architecture: `"<bus>:<vid>:<pid>"`, plus `":<serial>"` when the
  device reports a serial, plus `":#<n>"` ordinal only when two identical serial-less devices exist. Lower-case 4-digit
  hex for vid/pid. Constructors: `DeviceId::hid(vid, pid, serial: Option<&str>)`, `DeviceId::nvapi(device_id, subsys_id)`,
  `with_ordinal(self, n)`, `as_str()`, `Display`. Never derive an id from a device path.
- `device`: `Bus { Hid, NvApi, Smbus }`, `DeviceKind` (the 22 OpenRGB device types, see
  `reference/openrgb/RGBController/RGBController.h` `device_type`, mapped to PascalCase variants plus `Unknown`),
  `DeviceInfo`, `ZoneKind { Single, Linear, Matrix { width, height, map: Vec<Option<u32>> } }`, `Zone`, `Led`,
  `DeviceDescription { info, zones, leds, modes, direct_mode: Option<u32>, keepalive: Option<Duration> }` with
  `fn validate(&self) -> Result<(), ModelError>` enforcing: zones sorted by `start`, contiguous, non-overlapping and
  covering `0..leds.len()` exactly; every matrix map index `< zone.len`; `direct_mode` names an existing mode whose
  `color_mode == PerLed`; mode ids unique. `fn led_count(&self)`, `fn zone_range(&self, zone) -> Range<usize>`.
  Serialize `Duration` as milliseconds (`u64`).
- `mode`: `ModeCaps` bitflags (SPEED, DIRECTION_LR, DIRECTION_UD, DIRECTION_HV, BRIGHTNESS, PER_LED_COLOR,
  MODE_SPECIFIC_COLOR, RANDOM_COLOR, SAVE_MANUAL, SAVE_AUTOMATIC) with serde (bitflags `serde` feature is enabled),
  `ColorMode { None, PerLed, ModeSpecific, Random }`, `Direction { Left, Right, Up, Down, Horizontal, Vertical }`,
  `Mode { id, name, caps, color_mode, speed: Option<RangeInclusive<u32>>, brightness: Option<RangeInclusive<u32>>,
  colors: Option<RangeInclusive<usize>> }`, `ModeState { mode, speed, brightness, direction, colors: Vec<Rgb> }`.
  `Mode::validate_state(&self, state: &ModeState, led_count: usize) -> Result<(), ModelError>`: speed/brightness present
  only if the cap is set and then inside the range; direction only if a direction cap is set and consistent with it;
  colors length inside `colors` range for `ModeSpecific`, equal to `led_count` for `PerLed`, empty for `None`/`Random`.
  `DeviceDescription::validate_state(&self, state)` looks the mode up and delegates.
- `controller`: `trait Controller: Send` exactly as in the architecture (`description`, `set_direct`, `set_zone_direct`,
  `set_mode`, `resize_zone` default `Unsupported`, `save_to_hardware` default `Unsupported`, `keepalive` default `Ok`,
  `release` mandatory). Document the threading contract: one engine thread per controller; `Sync` not required.
- `backend`: `trait Backend: Send + Sync { fn name(&self) -> &'static str; fn bus(&self) -> Bus;
  fn detect(&self, ctx: &DetectContext) -> Vec<Detected>; fn subscribe_hotplug(&self, tx: HotplugSender) -> Option<Box<dyn HotplugHandle>>; }`.
  `DetectContext { disabled_families: HashSet<String> }` with `fn is_enabled(&self, family: &str) -> bool`.
  `enum Detected { Device(Box<dyn Controller>), Failed { family: String, location: String, error: DeviceError } }`
  (implement `Debug` by hand for the `Device` arm). `HotplugEvent { bus, vid: u16, pid: u16, path: String, action: HotplugAction { Arrived, Removed } }`.
  `HotplugSender` = `std::sync::mpsc::Sender<HotplugEvent>` (keep std to avoid a dependency here).
  `trait HotplugHandle: Send { fn stop(self: Box<Self>); }`.
- `family`: `pub const MSI_MYSTIC_LIGHT: &str = "msi_mystic_light"`, `WOOTING`, `LOGITECH`, `NVIDIA_ILLUMINATION`,
  and `pub const ALL: &[&str]`. Config, drivers and the conflict catalog all refer to these ids.
- `error`: `DeviceError` (thiserror): `Unsupported`, `InvalidArgument(String)`, `Transport(String)`, `Disconnected`,
  `Timeout`, `Protocol(String)`, `Faulted(String)`; `fn is_fatal(&self) -> bool` (Disconnected, Faulted).
  `ModelError` for validation failures with precise messages (which zone, which index).
- `event`: `EngineEvent { DeviceAdded(DeviceInfo), DeviceRemoved(DeviceId), DeviceFaulted { id, error: String },
  DeviceStateChanged { id, state: ModeState } }` (serde, used later by cc-engine and the UI).

## Tests (all in the crate, `cargo test -p cc-core`)

- `Rgb` parse/format round trips including `#RGB` short form and rejection of bad input.
- `DeviceId` formats for hid with/without serial, with ordinal, nvapi; lower-case hex, zero-padded.
- `DeviceDescription::validate` accepts a correct 3-zone description and rejects: gap between zones, overlap, zone past
  the LED count, matrix map index out of range, `direct_mode` pointing to a non-PerLed mode, duplicate mode ids.
- `validate_state` matrix across caps (speed without cap rejected, speed out of range rejected, PerLed length mismatch rejected,
  ModeSpecific color count outside range rejected, Random with colors rejected).
- JSON round trip of a full `DeviceDescription`, `ModeState`, `EngineEvent`; `Duration` as milliseconds.
- A `Box<dyn Controller>` can be created from a tiny test double and called through the trait (compile-level check of
  object safety); same for `Backend`.
