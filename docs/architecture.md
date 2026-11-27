# Architecture (v1)

Target: Windows 11, x86_64. One process. Rust workspace plus a Tauri 2 desktop app with a Vue 3 frontend.
Design goals in priority order: never damage hardware, never interfere with FanControl or other tools, no
network except the signed auto-update, no admin, low idle cost, a backend boundary that admits SMBus/PawnIO later.

## Workspace layout

```
Cargo.toml                 workspace, shared [workspace.dependencies], lints
crates/
  cc-core/                 device model, traits, ids, errors, events (no I/O, no Windows deps)
  cc-hid/                  HidTransport trait; hidapi backend; fake/transcript transport; Windows hot-plug
  cc-nvapi/                dynamic NvAPI binding (nvapi64.dll via nvapi_QueryInterface), Illumination API
  cc-drivers/              device drivers behind feature flags: msi_mystic_light, wooting, logitech, nvidia_illumination
  cc-conflicts/            conflicting-software catalog (TOML, embedded), scanners, opt-in actions
  cc-system/               Windows glue: Task Scheduler tasks, session-end flags, cross-session lock, identity, ACLs
  cc-config/               settings and profiles (TOML in %APPDATA%\CaseControl), paths, migration
  cc-engine/               runtime: detection, per-device worker threads, frame scheduler, profiles, event bus
apps/
  desktop/                 Tauri 2 app
    src-tauri/             tauri commands, tray, logon task, session end, single instance, logging setup,
                           the --apply-at-boot and --uninstall-cleanup modes
    src/                   Vue 3 + TypeScript + Vite (pnpm)
docs/                      research, decisions, this file
reference/openrgb/         copied GPLv2 sources used as protocol reference
.github/workflows/ci.yml
```

Dependency direction: `cc-core` <- `cc-hid`, `cc-nvapi`, `cc-config`, `cc-conflicts` <- `cc-drivers` <- `cc-engine` <- `apps/desktop`;
`cc-system` (no internal dependencies) <- `cc-conflicts`, `apps/desktop`.
`cc-core` compiles on any OS so model tests run anywhere; everything Windows-specific sits behind `#[cfg(windows)]`.

## Core model (`cc-core`)

Mirrors OpenRGB's RGBController semantics (one flat color buffer, zones index into it, modes with capability flags,
"direct" mode = per-LED static colors with no hardware effect) in idiomatic Rust.

```rust
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(String);          // stable across runs and USB ports: "hid:<vid>:<pid>[:<serial>]" for HID,
                                      // "nvapi:<vendor>:<device>:<subvendor>:<subdevice>" for GPUs (lower-case 4-digit hex);
                                      // never a path hash (HID paths change with the port). If two identical
                                      // devices have no serial, append an ordinal ":#0", ":#1" in enumeration order.
                                      // Empty, whitespace-only and all-zero serials ("00000000", as the G560 reports) count as absent.

pub enum Bus { Hid, NvApi, Smbus /* reserved for the later tier */ }

pub enum DeviceKind { Motherboard, Dram, Gpu, Cooler, LedStrip, Keyboard, Mouse, Mousemat, Headset, HeadsetStand,
                      Gamepad, Light, Speaker, Virtual, Storage, Case, Microphone, Accessory, Keypad, Laptop, Monitor, Unknown }
                      // 22 variants in the order of OpenRGB's device_type enum

pub struct DeviceInfo { pub id: DeviceId, pub name: String, pub vendor: String, pub kind: DeviceKind,
                        pub bus: Bus, pub location: String, pub serial: Option<String>, pub firmware: Option<String> }

pub enum ZoneKind { Single, Linear, Matrix { width: u32, height: u32, map: Vec<Option<u32>> } }
pub struct Zone { pub name: String, pub kind: ZoneKind, pub start: usize, pub len: usize,
                  pub resizable: Option<std::ops::RangeInclusive<usize>>,
                  #[serde(default)] pub brightness_only: bool }   // fixed colour, takes only an intensity
pub struct Led { pub name: String }

bitflags! ModeCaps { SPEED, DIRECTION_LR, DIRECTION_UD, DIRECTION_HV, BRIGHTNESS, PER_LED_COLOR,
                     MODE_SPECIFIC_COLOR, RANDOM_COLOR, SAVE_MANUAL, SAVE_AUTOMATIC }
pub enum ColorMode { None, PerLed, ModeSpecific, Random }
pub enum Direction { Left, Right, Up, Down, Horizontal, Vertical }
pub struct Mode { pub id: u32, pub name: String, pub caps: ModeCaps, pub color_mode: ColorMode,
                  pub speed: Option<RangeInclusive<u32>>, pub brightness: Option<RangeInclusive<u32>>,
                  pub colors: Option<RangeInclusive<usize>> }
pub struct ModeState { pub mode: u32, pub speed: Option<u32>, pub brightness: Option<u32>,
                       pub direction: Option<Direction>, pub colors: Vec<Rgb> }

pub struct DeviceDescription { pub info: DeviceInfo, pub zones: Vec<Zone>, pub leds: Vec<Led>, pub modes: Vec<Mode>,
                               pub direct_mode: Option<u32>, pub keepalive: Option<Duration> }

/// One physical controller. Called only from its own engine thread; `Send` suffices, `Sync` is not required.
pub trait Controller: Send {
    fn description(&self) -> &DeviceDescription;
    fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError>;          // full buffer, len == leds.len()
    fn set_zone_direct(&mut self, zone: usize, colors: &[Rgb]) -> Result<(), DeviceError>;
    fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError>;
    fn resize_zone(&mut self, zone: usize, len: usize) -> Result<(), DeviceError> { Err(DeviceError::Unsupported) }
    fn save_to_hardware(&mut self) -> Result<(), DeviceError> { Err(DeviceError::Unsupported) }
    fn keepalive(&mut self) -> Result<(), DeviceError> { Ok(()) }
    /// Hand the device back to its firmware/hardware mode. Called on shutdown and on explicit "release".
    fn release(&mut self) -> Result<(), DeviceError>;
    /// The lighting the device shows on its own as read at open (no I/O), as a state of one of its modes.
    fn hardware_mode(&self) -> Option<ModeState> { None }
    /// Driver-opaque bytes of the device's own lighting read at open, to remember (ADR 0001 #38). No I/O.
    fn own_lighting(&self) -> Option<Vec<u8>> { None }
    /// Whether the device shows its own lighting (after open / after release), not a state CaseControl left.
    fn shows_own_lighting(&self) -> bool { true }
    /// A hardware mode that is dark at the bottom of its brightness range, used for brightness 0 (ADR 0001 #38).
    fn off_mode(&self) -> Option<u32> { None }
}

/// A way to find devices. HID detectors match on vid/pid/interface/usage page/usage; NvAPI on PCI ids.
pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn bus(&self) -> Bus;
    fn detect(&self, ctx: &DetectContext) -> Vec<Detected>;   // Detected = (DeviceInfo, Box<dyn Controller>) or an error record
    fn subscribe_hotplug(&self, tx: Sender<HotplugEvent>) -> Option<HotplugHandle>;
}
```

Rules encoded in the model, not in UI text:

- `release()` is mandatory. Every driver restores hardware mode / default effect so a crash-free quit leaves the
  device as the firmware would. Coexistence with iCUE-like tools depends on this.
- Detectors are toggleable per device family (`cc-config` `detectors` table, default on for all v1 families), the
  same escape hatch OpenRGB uses after its brick incidents.
- No probing writes during detection beyond what the reference implementation does for the same device.

## HID layer (`cc-hid`)

```rust
pub trait HidTransport: Send {
    fn write(&mut self, data: &[u8]) -> Result<usize, HidError>;                 // output report, data[0] = report id
    fn read_timeout(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, HidError>;
    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), HidError>;
    fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize, HidError>;  // buf[0] = report id on entry
}
pub struct HidMatch { vid: u16, pid: Option<u16>, interface: Option<i32>, usage_page: Option<u16>, usage: Option<u16> }
pub struct HidDeviceInfo { path, vid, pid, interface, usage_page, usage, serial, product, manufacturer }
pub fn enumerate() -> Vec<HidDeviceInfo>;                       // hidapi, one pass, cached per detection run
pub fn open(path) -> Result<Box<dyn HidTransport>>;             // hidapi default = FILE_SHARE_READ|WRITE, never exclusive
pub struct FakeTransport { script: Vec<Exchange> }               // asserts byte-exact writes, replays reads; for tests
pub fn watch_hotplug(tx) -> HotplugHandle;                      // windows: CM_Register_Notification(GUID_DEVINTERFACE_HID), debounced 500 ms
pub fn power::watch(tx) -> PowerWatch;                          // windows: PowerRegisterSuspendResumeNotification, Suspend / Resume events
pub struct NamedMutexGuard;                                     // Global\<name> acquire/release, for CorsairLink-style guards (unused in v1 drivers, present for the tier)
```

`hidapi` with the `windows-native` feature (pure Rust, no C toolchain; it lacks `get_input_report`, which none of the
four reference drivers use, verified by grep over `reference/openrgb/`). Reads block on the device thread only. Hot-plug triggers a rescan
of the affected vid/pid only, not a full re-detection, so devices already open are untouched.

## Drivers (`cc-drivers`)

One module per family, each exposing `pub fn backend() -> Box<dyn Backend>` plus pure packet builders that are
unit-tested against the FakeTransport. Ported from `reference/openrgb/`:

| Module | Reference | Notes |
|---|---|---|
| `msi_mystic_light` | `MSIMotherboardController/*` | Detect on `1462:7D75` usage page 0x0001 usage 0x00; class by `get_feature_report(0x52)` length (185/162/112, else 761 path). Direct mode per ARGB header via feature reports. Only PIDs in the upstream tested list; no `ENABLE_UNTESTED` set. Port the `GetFirmwareVersionInvalid` gate verbatim (it separates the tested path from the brick-era one). The save-to-flash byte inside the mode packet is always zero except in `save_to_hardware()`, `persist_look()` and `restore_saved_state()` (save to devices, see Engine). Last good own lighting (ADR 0001 #38): without the "unreleased" marker, a 185-byte config read at open that is not per-LED (no zone in effect 0x25) is reported as `own_lighting` (save byte 0), and a per-LED one belongs to another program (nothing stored or restored, an untouched release sends nothing). With the marker, the readback is CaseControl's: never stored; the stored state, if it passes `ConfigPacket::from_own_lighting` (185 bytes, report id 0x52, save byte 0, no zone in effect 0x25), is restored at open (so board and UI agree) and by `release()`, except when the engine is about to drive the board (`DetectContext::setting_pending`, ADR 0001 #42): then only `release()` restores it, so the board does not flash the old look at start; without one, `shows_own_lighting()` is false and the marker stays. The off output (Static 0) also darkens headers sized 0. `off_mode` is Static (brightness 0 = Static at brightness 0, no per-LED). Save to the board (ADR 0001 #42): `persist_look` builds one packet on the working configuration (never a direct-mode readback) with the `apply_mode`/`apply_zone_color` calls of a mode update (zone colour = Static at level 10, a mode exactly as `set_mode`, off = Static at brightness 0), refuses it unless every gate of research §1.4 passes (firmware gate, 185 bytes / report 0x52, no record in effect 0 or 0x25, LED counts unchanged and within the maxima, changes only in the saved zones' records plus the on-board sync/mirror bytes, `protocol::save_change_mask`; outside direct mode a fresh readback must equal the working configuration), sends it with save 0 (leaving direct mode) and checks the readback, then the two reports with save 1, then checks the readback again, and finally re-enters direct mode with the last frame. The fingerprint is the packet; afterwards `release()` hands back the saved packet and it is the own lighting. Backup = the own lighting seen before CaseControl's first write of the session (the readback, or the stored own lighting of a marked board) if `protocol::validate_backup` accepts it (also effects 1..=39 except 0x25, brightness <= 10); a restore validates the same way. | LibreHardwareMonitor's MSI HID controller only targets CoreLiquid AIOs (PIDs 0x6A04..0x6A16), so FanControl does not touch this device. |
| `wooting` | `WootingKeyboardController/*V3*` | `31E3:1310`, usage page 0xFF55 (V3). Direct per-key RGB, restore keyboard profile on release. |
| `logitech` | `LogitechController/LogitechHIDPP20Controller`, `LogitechLightspeedController`, `LogitechProtocolCommon` | G560 `046D:0A78` usage page 0xFF43 (zone-only HID++ 2.0, feature 0x8070). PRO Wireless through receiver `046D:C539` (Lightspeed, virtual PID 0x4079); the receiver accepts one connection, pair-index handling per upstream. G560 save (ADR 0001 #42): direct colours persisted as Static per zone (brightness baked in), modes as `set_mode` sends them, off as Static black, all with the 0x8070 persistence byte; `release()` afterwards replays the persisted parameters (volatile). The persisted state cannot be read, so the backup is the per-zone open-time GetEffect readback (`encode_backup`: version, zone count, 16 bytes per zone), only when every zone maps (`hidpp::restore_params`) and the device was not opened with the "unreleased" marker; otherwise the original is unknown, and without a backup stored in an earlier session the engine never writes the device. On the owner's G560 zone 1 reads back 16 zero bytes (log of 2026-11-17), so its backup is incomplete today; the cause is not clear from the code or the protocol notes (the reference never parses GetEffect) and is left for a hardware look. Lightspeed, Wooting and NVIDIA cannot save. |
| `nvidia_illumination` | `NVIDIAIlluminationController/*`, `dependencies/NVFC/nvapi.*` | Load `nvapi64.dll`, `NvAPI_GPU_ClientIllumZonesGetControl/SetControl`. Match `10DE:2C02` subsys `10DE:2095`. RGBW zones treated as RGBW. NvAPI Illumination is undocumented: function IDs and `#[repr(C)]` struct layouts/versions are copied byte-exact from `nvapi.h`. Always `GetControl` before any `SetControl`, keep the initial params, and `release()` writes them back. First hardware test is a round-trip (read, write the same values, read again) before any color change. |

Hardware checklist cases that every driver must pass on this machine (manual, documented in `docs/testing-hardware.md`):
FanControl running; `logi_lamparray_service` running and stopped (Logitech); G HUB running and closed (Logitech);
re-plug of the device while the app runs (hot-plug, profile re-association by `DeviceId`); app quit restores firmware mode.

## Engine (`cc-engine`)

- `Engine::start(config) -> EngineHandle`. Runs detection once, then owns one worker thread per controller with a
  bounded command channel (`SetDirect`, `SetZone`, `SetMode`, `Resize`, `Save`, `Release`, `Shutdown`, `Persist`,
  `Restore`).
- Early apply (ADR 0001 #42): `EngineConfig::initial_settings` (the visible active profile's device settings) are
  the pending settings before the first detection, and `EngineConfig::brightness` carries the profile's start
  brightness, so the first output already has it. Detection adds each backend's devices as soon as that backend is
  done (in `all_backends` order: MSI first), so the MSI board is driven while Wooting, Logitech and NVIDIA are still
  probed; detection order and fault handling are unchanged. `DetectContext::setting_pending` names the devices about
  to be driven, so a driver skips restoring an old look at open for them (Mystic Light).
- Frame scheduler: when a profile or live color edit is active, coalesces updates to at most `frame_rate` Hz
  (default 30) per device; keep-alive timer per device from `DeviceDescription.keepalive`.
- Hot-plug: `HotplugEvent` -> targeted re-detect -> `EngineEvent::DeviceAdded/Removed`.
- Brightness (ADR 0001 #33, #35): a device brightness per device and a master brightness (0-100 %) scale what goes
  to the device (direct colours, or a hardware mode's brightness parameter inside its range); the stored settings
  stay unscaled. Brightness 0 is the off state: black in direct colours, in a hardware mode the device's "Off" mode,
  else black in its direct mode. A driver that names an off mode (`Controller::off_mode`; Mystic Light: Static,
  ADR 0001 #38) gets that mode at the bottom of its brightness range (black colours) instead of black in the direct
  mode, for direct colours and hardware modes alike, so brightness 0 does not put the MSI board into per-LED mode;
  raising the brightness sends the colours (or mode) again. The device stays `Active` at 0.
- Brightness-only LEDs (`Zone::brightness_only`, set by `nvidia_illumination` for SINGLE_COLOR / COLOR_FIXED zones;
  the RTX 5080 FE is white only, ADR 0001 #37): a colour counts only as its intensity, the highest channel
  (`Rgb::intensity`). The engine reduces direct colours (and per-LED colours of a mode) for those LEDs to white at
  that intensity (`DeviceDescription::reduce_fixed_colors`) in the buffer, the recorded setting and the output, then
  brightness scales them as usual. A takeover with the default colour drives them at full white, so only brightness
  matters. The device view hides colour picker and LED selection for a device whose LEDs are all brightness-only
  (`DeviceDescription::is_brightness_only`) and draws them as white at the output intensity.
  Per-LED white levels (ADR 0001 #41): instead of colour controls, the device view shows one level slider (0-100 %)
  per visible brightness-only LED (labelled with the zone name, plus the LED name in a zone with several LEDs), also
  for the brightness-only zones of a mixed device. A level is stored as a grey in the direct colours
  (`Rgb::white_level(p)` = `round(p * 255 / 100)` per channel; read back with `Rgb::level_percent`; the frontend
  mirrors both in `lib/color.ts`), so it goes through `set_direct` into the `DeviceSetting::Direct` of the active
  profile like any colour edit and survives restarts. The device brightness (and master brightness) scale these greys
  as usual. The NVIDIA Illumination driver writes each zone's own brightness from its LED's intensity
  (`round(intensity * 100 / 255)`: direct colours always use the full mode brightness; only a "Direct" mode state
  sent with `set_mode` carries its own brightness, for that write only, so a takeover with the readback at 60 % does
  not cap later levels), so a level of p % reaches that zone as exactly p % at full device brightness; zones are
  independent in the `SetControl` struct (tested through the mock `IllumApi`).
- Takeover (ADR 0001 #33, #35, #36, #37): brightness changes (device and master) and Take over all (`connect_all`)
  take over a released, non-hidden, non-faulted device in this order: its stored setting, else the mode its driver
  read back (`DeviceView::hardware`), else the default colour (`Settings::default_color`, "Fallback colour" in the
  UI, white by default; `EngineConfig::default_color`, `EngineHandle::set_default_color`) as a static colour on
  every LED in its direct mode, scaled by brightness as usual. A device without a direct mode gets its first mode that takes a colour (per
  LED or mode-wide), set to the default colour with speed in the middle and brightness at the top of their ranges;
  only a device without either stays released (none of the current drivers: every MSI, Lightspeed, G560, Wooting and
  NVIDIA description has a direct mode or a per-LED mode). A mode-wide colour counts only with room for at least one
  colour. Changing the default colour does not repaint driven devices. `DeviceView::takeover` (`{ mode_id } | null`)
  reports what a takeover uses (what the device is driven with now, else the choice above; `mode_id` null for direct
  colours, the whole field null when nothing fits or the device is faulted), so the frontend does not repeat the
  choice. `release_all` returns a `ReleaseReport { released, failed }`: `released` counts the devices that
  were driven (or being taken over) before.
- Dark zones (ADR 0001 #26): the app's hidden zones (`EngineConfig::dark_zones`, `EngineHandle::set_dark_zones`)
  are black in every direct output, whatever drives the device: colour edits, profiles, pending settings applied on
  arrival, takeovers of a stored setting and the default colour (also its per-LED colours in a hardware mode).
  `set_dark_zones` re-sends a device driven with direct colours at once when the new zones black out something it
  shows. There is one masking function, `cc_engine::mask_hidden_zones`, which the app also uses for the buffers it
  builds.
- Last good own lighting (ADR 0001 #38). The seam: `DetectContext::own_lighting` (`BTreeMap<DeviceId, Vec<u8>>`)
  carries what is remembered into every detection, and `Controller::own_lighting()` reports what a newly opened
  controller read as the device's own lighting; both are driver-opaque bytes, so any driver can use them. The engine
  holds a `cc_config::HardwareState` (`EngineConfig::hardware_state`; the app loads it from
  `%LOCALAPPDATA%\CaseControl\hardware-state.toml` and `device-memory.toml`, in memory only without storage), hands
  its map to every detection
  and records each new controller's bytes in it. The driver validates stored bytes before it uses them (Mystic
  Light: see the driver table). `EngineHandle::hardware_state()` returns a read-only `HardwareSnapshot`.
- "Unreleased" marker (ADR 0001 #38), in the same file: CaseControl wrote to the device and did not hand it back.
  The worker sets it before its first write after open or a release and clears it after a successful release;
  `DetectContext::unreleased` hands it to every detection. A driver that finds the marker knows the readback is a
  state CaseControl left (crash, sleep, re-enumeration) and never reports it as own lighting (Mystic Light restores
  the stored own lighting at open instead). `Controller::shows_own_lighting()` (default `true`) tells the engine,
  right after open and after each successful release, whether the device shows its own lighting; only then is the
  marker cleared. Format:

  ```toml
  # CaseControl machine state (ADR 0001 #38, #42). Written by CaseControl; not meant to be edited.
  unreleased = ["hid:1462:7d75:7D7522120106"]
  pending_restore = ["hid:046d:0a78"]                 # backups to write back when the device next connects

  [own_lighting]
  "hid:1462:7d75:7D7522120106" = "5201ff0000..."   # lower-case hex ([0-9a-f]), no separators

  [saved_backup]                                      # original saved state, before the first save (driver-opaque)
  "hid:1462:7d75:7D7522120106" = "52190a..."

  [saved_fingerprint]                                 # what was saved last (driver-opaque)
  "hid:1462:7d75:7D7522120106" = "52010a..."

  [saved_writes]                                      # writes to the device's memory on the last day it was written
  "hid:1462:7d75:7D7522120106" = { day = 20419, count = 2 }   # day = days since 1970-01-01 (UTC)
  ```

  The file is rewritten atomically, only when a value changed, and the in-memory state changes only after the write
  succeeded. Content that is not TOML, tables and arrays of the wrong type and invalid entries are ignored with a
  warning (and dropped on the next write); top-level keys this version does not know are kept verbatim, so a newer
  version's records (a backup above all) survive a rewrite by an older one. A file that exists but cannot be read
  (I/O error, not UTF-8) makes the state read-only for the session: it is never overwritten.
- Error policy: write errors back off exponentially (50 ms .. 5 s), after 10 consecutive failures the device is marked
  `Faulted` and shown as such; never retried in a tight loop. A faulted device is released once, best effort, when
  its worker stops: on shutdown, and when detection is about to open it again. Detection stops the faulted workers
  of the backends it runs (all stops sent first, then one deadline, `Timing::faulted_release`, 500 ms); a device
  whose old worker has not ended by then is skipped in that pass and re-opened by a later one, so an old worker never
  writes after a new controller opened the device. A removed device is not released: its handle is gone; the
  re-opened controller finds the board's state itself (with the "unreleased" marker telling it the state is ours).
- Shutdown: `EngineHandle::shutdown_with(mode, timeout)`, `shutdown(timeout)` = `ShutdownMode::Release`: release
  every device (also faulted ones, best effort), join threads with timeout, then exit. Ctrl-C/WM_CLOSE/tray Quit all
  funnel here. `ShutdownMode::Keep` (ADR 0001 #42: Windows shutdown, restart, sign-out) sends each device its pending
  or paced output once (so it shows the latest one) and stops its thread without `release()`; the "unreleased"
  markers stay set (the devices still show CaseControl's output) and the own lighting is untouched, so the next start
  treats the readback as CaseControl's. Each device gets its share of the timeout; dropping the last handle always
  releases.
- Save to devices (ADR 0001 #34, #42), only on the setting's triggers, never on live edits:
  `EngineHandle::persist_all(timeout) -> PersistReport` saves every visible, capable (`Controller::persist_caps`),
  driven, connected device from its current output (`Slot::output(levels).out`): direct colours as one colour per
  zone averaged in linear light (`cc_core::average_linear`; hidden and empty zones `None` = left as stored), a
  hardware mode exactly (its brightness parameter already scaled), brightness 0 as `PersistLook::Off`. Per device, in
  the worker: no stored backup yet -> the driver's `saved_state_backup()` is stored first (none: skipped
  "original_unknown"; store failed: nothing written); at most 20 writes per device and day (`DAILY_SAVE_CAP`);
  `persist_look(look, last_fingerprint)` returns `Written { fingerprint }` (recorded with the day's count and the
  new own lighting, logged) or `Unchanged`. Devices are done one after another, each with an equal share of what is
  left of the timeout (the rest are skipped "time_budget"), so the call fits Windows' 5 s session-end limit together
  with `shutdown_with`. `persist_all` drops pending restores. `EngineHandle::restore_saved(timeout)` writes every
  stored backup back (`restore_saved_state`, which validates first), drops it on success and re-sends a driven
  device's current output; an absent or faulted device is marked `pending_restore` and restored when it next connects
  (before its pending setting). The report lists per device `written`, `unchanged`, `skipped { reason }` (`hidden`,
  `faulted`, `not_capable`, `not_driven`, `nothing_to_save`, `original_unknown`, `daily_cap`, `no_backup`, `absent`,
  `time_budget`) or `failed { error }`.
- Event bus (`tokio::sync::broadcast` or crossbeam) consumed by the Tauri layer and re-emitted as Tauri events.

### Sleep and resume

In sleep or hibernation the devices fall back to their saved or firmware lighting, and USB devices may drop off the
bus or come back with dead handles. The engine subscribes to suspend/resume broadcasts (`cc_hid::power`,
`PBT_APMRESUMEAUTOMATIC` and `PBT_APMRESUMESUSPEND`, no window needed) and calls `EngineHandle::reassert()`. That is
debounced (a wake reports itself twice, a second call within 5 s does nothing) and runs on its own thread: after
1 s for USB to settle, every driven device that is not faulted gets a reset (`release()`, which also clears the
driver's "already sent" caches and makes it re-send its init) followed by its current output (colours or mode with
device and master brightness applied); released and undriven devices are left alone. A
device whose writes keep failing is retried with the usual back-off and faulted after 5 s. For 20 s after the
resume the engine re-runs detection whenever a device present before the sleep (or removed just before the resume
broadcast) is missing or faulted; such a device comes back with what it was driven with. Outside a resume a
re-plugged device still starts undriven.

## Conflicts (`cc-conflicts`)

- Catalog `catalog.toml` embedded via `include_str!`, entries from `docs/research/03-conflicting-software-catalog.md`:
  `name, vendor, category, class, processes[], services[], drivers[], uninstall_names[], install_paths[],
  lighting_providers[], access = [hid|smbus|superio|lamparray|...], affects = [device family ids], severity, note`.
  Unverified names are kept but flagged `verified = false`. `class` is `suite` (full RGB suites: SignalRGB, OpenRGB,
  iCUE, Armoury Crate / Aura, Polychrome, RGB Fusion, MSI Center / Dragon Center) or `vendor_tool` (everything
  else, e.g. G HUB, Wootility, the LampArray service, monitors, drivers) (ADR 0001 #31, #32).
- Scanners (all unelevated, read-only): processes (Toolhelp), services and driver services with start type
  (`EnumServicesStatusEx(SERVICE_WIN32 | SERVICE_DRIVER)`), installed apps (Uninstall keys incl. Wow6432Node and
  HKCU), Dynamic Lighting (`HKCU\Software\Microsoft\Lighting` incl. `Providers`), autostart (Run / RunOnce in HKCU,
  HKLM and HKLM WOW6432Node, the per-user and common Startup folders with `.lnk` targets, each with its
  `Explorer\StartupApproved\{Run,Run32,StartupFolder}` state), OpenRGB SDK port (TCP connect 127.0.0.1:6742 is the
  single allowed local socket use and is behind a setting, default off).
- Output: `Finding { entry, class, evidence: Vec<Evidence>, autostart: Vec<AutostartItem>, running, installed }`.
  Autostart items match a catalog entry when the executable's file name matches its `processes`; a Win32 service of
  the entry is an autostart item too (enabled = start type Auto). The desktop app adds the affected devices.
- Actions (only on a user click, ADR 0001 #12 and #31), a fixed enum: terminate process (pid + name re-checked on
  the open handle), stop Win32 service, set service start type Auto / Manual, set the `StartupApproved` value
  (`02 ...` on, `03 ...` + FILETIME off, as Task Manager does; Run values are never deleted). "Close" = terminate the
  entry's processes and stop its running services; drivers are never touched. "Turn off autostart" = StartupApproved
  off and services to Manual; "Re-enable" restores StartupApproved and the remembered previous start type. Every
  action is checked against a fresh scan (`check_action`) and logged with its entry.
- Elevation: HKCU writes and terminations of programs without admin work run in the app first. Everything that
  needs administrator rights (HKLM StartupApproved, service stop and start type), the terminations of programs that
  also have admin work (so they run after their services were stopped and set to Manual and nothing restarts them)
  and terminations refused for lack of rights run afterwards in **one** elevated invocation of the app's own
  executable (`--elevated-actions <request.json>`, `ShellExecuteExW` with `runas`, owned by the main window), so one
  click causes at most one UAC prompt. Processes are only ended in the app's own Windows session. `main` enters this
  helper mode before Tauri, the single-instance guard, the engine or any logging setup. The helper accepts only
  `...\CaseControl\elevated\request-<n>-<n>.json` with no link or reparse point in its path, reads at most 64 KiB
  (64 actions), runs a fresh scan, refuses HKCU writes and every action the scan does not tie to the named catalog
  entry or session, runs the rest through the service control manager and the registry (no shell), and writes the
  per-action results to a new file next to the request. The app waits for a time sized from the batch (30 s plus
  12 s per service stop and 2 s per other action); if the helper is still running then, the results are reported as
  not confirmed yet and its files are left alone. Request files older than an hour are removed at the next start.
  Since ADR 0001 #42 the helper also compares the request's `session_id` with its own session and refuses the whole
  request on a mismatch (`run_elevated_request_in` with `HelperContext::for_request`), and writes its result with
  `write_helper_result`, which re-validates the request path (no link or reparse point, canonical path unchanged)
  right before creating the result file. A small window between that check and the create call remains (closing it
  would need a create relative to an opened directory handle).
- Start-up task actions (ADR 0001 #42): `register_boot_task` / `remove_boot_task` are field-less actions
  (`{"kind":"register_boot_task"}`; any extra field fails to parse) under the reserved entry id
  `casecontrol-boot-task`, which `check_action` accepts for these two actions only and refuses for every other one.
  They need administrator rights, run only inside the elevated helper (in `execute` outside it they fail) and need
  the request's session. The helper derives every value itself (`cc_conflicts::boot_task`): the principal is the user
  signed in to the helper's own session, which must be a user account (`S-1-5-21-...` or Entra `S-1-12-1-...`), equal
  to the helper token's user (approving the prompt with another administrator's credentials is refused), the owner
  of the request file, and have a `ProfileList` entry whose folder contains the request file; the executable is the
  helper's own `current_exe()`, which must pass `cc_system::check_exe_path`; arguments, S4U and least privilege are
  fixed by `cc_system::TaskSpec::boot`. The task is registered create-only, read back and verified (deleted again on
  any difference); remove deletes only a task that reads back as ours. The helper times 15 s per such action.
- Matching: catalog `processes` match executable file names; names that are too generic on their own
  (`generic_processes`, e.g. `CMService.exe`, `RunLed.exe`, `LightingService.exe`) match only when the full path
  contains one of the entry's `path_hints`.

## System glue (`cc-system`, ADR 0001 #42)

The one new home for Windows `unsafe` besides the crates listed under Security posture: everything in a private
`sys` module with RAII guards and `// SAFETY:` comments, pure logic outside it, and non-Windows stubs that return
`Error::Unsupported` so the workspace builds and tests elsewhere.

- `task`: CaseControl's two Task Scheduler tasks in the root folder `\` (owned by Windows, so the user cannot create
  or control the folder), named with the user's SID (`CaseControl Logon <SID>`, `CaseControl Start-up <SID>`).
  `TaskSpec::logon` (logon trigger for the SID, no delay, `InteractiveToken`, `LeastPrivilege`, priority 4, runs on
  battery, `PT0S`, `IgnoreNew`, `<exe> --hidden` in the exe's folder) and `TaskSpec::boot` (boot trigger, `S4U`,
  `LeastPrivilege`, `PT2M`, same settings, exactly `--apply-at-boot`) build escaped XML (schema 1.2) and refuse a
  non-user SID or an exe path that is not on a local drive. `TaskStore` (`WindowsTaskStore`: COM `ITaskService`,
  `RegisterTask` with `TASK_CREATE`, the SID and the logon type, no password, no SDDL; `MemoryTaskStore` for tests)
  plus `register_verified` (an existing task of that name is replaced only when `Ownership` says it is ours, then
  read back and compared with `TaskSpec::verify`; deleted on any difference), `delete_ours` (read back, delete only
  ours: principal, a single exec action with CaseControl's exe file name and the kind's argument) and `task_state`
  (Missing / Matches / Differs, e.g. a stale exe path / Foreign). `is_apply_at_boot_invocation` matches
  `<exe> --apply-at-boot` exactly. No `schtasks.exe`, no shell.
- `session_end`: `install(hwnd)` subclasses the given top-level window of the calling thread and tao's
  `Tao Thread Event Target` window (found with `EnumThreadWindows`), records the `lParam` of `WM_QUERYENDSESSION` /
  `WM_ENDSESSION` in an atomic and always calls `DefSubclassProc`; `current()` combines it with
  `GetSystemMetrics(SM_SHUTTINGDOWN)` into `SessionEnd::{NotEnding, SignOut, ShutdownOrRestart, CloseApp, Unknown}`
  (pure `classify`: `CLOSEAPP` first, then `LOGOFF`, a cancelled end is forgotten, ending without a message is
  `Unknown`).
- `lock`: `DeviceLock` is the named mutex `Global\CaseControl-Devices-<SID>` with the DACL
  `D:P(A;;GA;;;<SID>)(A;;GA;;;SY)`; `try_acquire` / `acquire_with_timeout` return `Acquired` / `Busy` /
  `HeldBySomeoneElse` (opening failed with access denied: another account created the name). The guard is not
  `Send` (mutex ownership is per thread); `hold_until_exit` keeps it for the process.
- `identity`: `current_user_sid`, `current_session_id`, `session_user` / `interactive_user`
  (`WTSQuerySessionInformation` + `LookupAccountName`), `profile_path` (HKLM `ProfileList\<SID>\ProfileImagePath`,
  read only), `file_owner_sid`, `is_user_sid`.
- `acl`: `check_exe_path(exe, sid)`: absolute drive-letter path, regular file, no reparse point in any component,
  canonical path unchanged, and owner plus DACL of the exe and every folder above grant the dangerous rights only to
  the SID, SYSTEM, Administrators or TrustedInstaller. Dangerous: on the exe write / append / delete / `WRITE_DAC` /
  `WRITE_OWNER`; on its folder also add file / add subfolder / delete child; further up delete / delete child /
  `WRITE_DAC` / `WRITE_OWNER` (creating entries there is harmless, `C:\` lets every user create folders). Inherit-only
  entries are skipped, deny entries ignored, unreadable allow entries and NULL DACLs fail (`acl::check_object` is the
  pure evaluation).

## Config (`cc-config`)

`%APPDATA%\CaseControl\config.toml`, `profiles\<name>.toml`; machine state, not settings, in
`%LOCALAPPDATA%\CaseControl`: `hardware-state.toml` (the last good own lighting and "unreleased" marker per device,
ADR 0001 #38) and `device-memory.toml` (the save-to-devices records, #42), both bound to the machine; see Engine. The
old roaming `hardware-state.toml` (what released builds wrote) is migrated while the new one is missing, only its
`unreleased` and `own_lighting` (device-memory tables in it are ignored), and left in place. Logs in
`%LOCALAPPDATA%\CaseControl\logs`.
Settings: `autostart`, `start_minimized`, `frame_rate`, `detectors.<family> = true|false`, `conflicts.check_openrgb_port`,
`conflicts.dont_ask` (catalog ids the start dialog no longer asks about), `conflicts.previous_start_types` (service name
-> start type before CaseControl set it to Manual), `last_profile`. The switches of ADR 0001 #42 are top-level fields,
each written only when it differs from its default: `keep_colors_on_sign_out` (default true), `save_to_devices`
(default false), `apply_before_sign_in` (default false); the desktop app reads them as `prefs::PowerSettings`. Profile: per `DeviceId` a `ModeState` or a direct color buffer, plus the device brightness per `DeviceId`. Unknown keys preserved.

Profiles (ADR 0001 #24, #33, #40):

- There is always exactly one active profile (`last_profile`). On start: without any profile, "Profile 1" is created
  from what the devices show (usually empty) and made active; with profiles but an empty or dangling `last_profile`,
  the first in list order (case-insensitive sort) becomes active; the active profile is then applied.
- Autosave: every change to what a device shows (`set_direct`, `set_zone`, `set_mode`, `set_device_brightness`, and
  the takeovers of `connect_all` / `set_master_brightness`) marks the active profile changed. A writer thread
  (`autosave.rs`) writes it at most every 300 ms (the first change after a quiet period at once, a burst as one
  trailing write with the latest values); profile operations and the exit path flush a pending change first. The
  write merges the engine's `current_settings()` (what it drives each connected device with) into the profile file
  (entries of devices that are not connected stay) and replaces the brightness map with the engine's. There is no
  Save / Save as, and `set_device_brightness(id, percent)` has no `persist` flag any more.
- Applying a profile (switch, start, new, duplicate, delete of the active one) uses `EngineHandle::apply_replacing`:
  connected devices the profile has no entry for forget their setting and, if driven, fall back to their own mode
  (readback), else the fallback colour (ADR 0001 #36); a device none of these fit is released.
- New profile: empty (no settings, no brightness entries), named "Profile N" with the lowest free N >= 1, becomes
  active and is applied. Duplicate: "<name> copy", then "<name> copy 2", ... (the name is cut so the suffix fits 64
  characters), becomes active. Rename: any profile; the active one follows. Delete: refused for the last profile;
  deleting the active one activates the first remaining one in list order and applies it.
- Global per-device overrides (hidden devices and zones, zone sizes) and the fallback colour are settings, not part
  of profiles. Device brightness changes that versions before ADR 0001 #40 kept in the settings are laid over the
  active profile on the next start, saved into it and cleared.

## Desktop app (`apps/desktop`)

- Tauri 2, features `tray-icon`; plugins: `tauri-plugin-single-instance`, `tauri-plugin-updater` (see Updates),
  `tracing` to a rolling log file. Autostart is a Task Scheduler logon task (see "Colours across shutdown, sign-out and
  start"); `tauri-plugin-autostart` is gone. CSP strict, no remote URLs, `dangerousRemoteDomainIpcAccess` empty.
- Close hides to tray; tray menu: Show, Apply profile submenu, Release devices, Quit.
- Command line (parsed by `LaunchArgs::parse`; exact matches, unknown arguments ignored): `--hidden` starts with the
  window hidden (the logon task passes it). `--quit` asks a running instance to shut down: the single-instance
  plugin decides inside `tauri::Builder::build`, before the window, tray, engine or device detection exist (they start
  in the setup hook, which only `App::run` runs). With an instance running, the second process forwards its argv and
  exits there; the running instance's callback calls `shutdown()` (devices released once, then exit) before the
  forward returns, so the `--quit` process usually ends after the release. With none running, `run()` releases the
  single-instance lock and returns without running the app (exit code 0). `--quit-for-uninstall` is the uninstaller's
  variant: the running instance releases its devices without saving to them and exits without installing a pending
  update (`shutdown_for_uninstall`, `AppState::shutdown_engine_for_uninstall`). `main` handles three modes before
  `run()` (no Tauri, no single-instance guard): `--elevated-actions` (see Conflicts), `--apply-at-boot` (exact
  `[exe, flag]`) and `--uninstall-cleanup` (exact `[exe, flag]` or `[exe, flag, --no-prompt]`), see below. A command
  line that names `--apply-at-boot` or `--uninstall-cleanup` in any other form (any case, also inside a longer
  argument) exits with code 2 before anything starts (`boot::malformed_mode_invocation`), and the window refuses to
  start in session 0 (`refuse_session_zero`, exit code 2).
- Commands: `list_devices`, `set_direct`, `set_zone`, `set_mode`, `resize_zone`, `save_to_hardware`, `release_device`,
  `release_all`, `connect_all`, `set_device_brightness`, `set_master_brightness`, `rescan`, `set_device_hidden`, `set_zone_hidden`, `list_profiles/apply_profile/
  create_profile/duplicate_profile/rename_profile/delete_profile`, `scan_conflicts`, `startup_conflicts`, `resolve_conflicts`, `close_program`,
  `set_autostart`, `set_dont_ask`, `get_settings/set_settings`, `set_apply_before_sign_in`, `set_save_to_devices`,
  `set_default_color`, `update_status`, `check_for_updates`, `install_update` (see Updates). Shapes that changed with ADR 0001
  #36: `release_all`
  returns `{ released: number, failed: { id, error }[] }` (the frontend shows "N devices released" when `released`
  is above 0); `get_settings` / `set_settings` carry `default_color` (`"#RRGGBB"`) in the view and the patch;
  `set_default_color(color, persist) -> "#RRGGBB"` sets the default colour like the brightness commands: live in
  the engine while the picker is dragged (`persist = false`), saved with the final value (`persist = true`; on a
  failed save the engine goes back to the saved colour); `connect_all` and `set_device_brightness` take over with the
  default colour as above; `DeviceView` carries `takeover`; `set_zone_hidden` hands the hidden zones to the engine
  before saving (the engine re-masks a device it drives itself). Hiding a driven device (`set_device_hidden`) releases it; the frontend shows
  "Device released" when the device was Active before. Events: `device-added` (`DeviceInfo`),
  `device-removed` (`{ id }`), `device-faulted` (`{ id, error }`), `device-state-changed`
  (`{ id, active, state: ModeState | null }`: `active` is whether CaseControl drives the device now, `state` the
  hardware mode it was switched to when that was the change), `devices-changed` (no payload), `profiles-changed`,
  `conflicts-updated`, `startup-conflicts`, `update-status` (`UpdateView`, see Updates).
- Conflicts at start (ADR 0001 #31): after setup (also with `--hidden`) a background thread scans; device control is
  already running. RGB suites that affect a detected, non-hidden device, are not in `conflicts.dont_ask` and have
  something to close or an autostart item that is on form the start dialog model. If it is not empty the window is
  shown and `startup-conflicts` sent; the frontend also asks for it with `startup_conflicts` on mount. The dialog
  offers per program "Close now" and "Turn off autostart" (preselected) and "Don't ask again"; Proceed sends one
  `resolve_conflicts` (at most one UAC prompt) and shows per-action results. While a dialog program still starts
  with Windows, the shell shows a warning notice that reopens the dialog. Vendor tools appear only on the Conflicts
  page, which offers End process / Stop service (`close_program`), Turn off / Re-enable autostart and the
  "Don't ask again" switch.
- Frontend: Vue 3 + TS + Vite, pnpm, `@tauri-apps/api`. English UI. Pages: Lighting (the devices overview,
  `/lighting`; called Devices until ADR 0001 #37), Conflicts, Settings. Profiles are picked and managed in a
  dropdown in the Lighting toolbar (ADR 0001 #40); the Profiles page is gone.
  Lighting toolbar (ADR 0001 #41): on the left (PageHeader slot `controls`) the profile dropdown, one takeover
  button, Show hidden and Rescan; on the right (slot `actions`) All brightness, then the fallback colour circle with
  its "?" tooltip. The takeover button reads "Take over all" (runs `connect_all`) while a released, non-hidden device
  can still be taken over (its `takeover` is not null), otherwise "Release all" (runs `release_all`, toast "N devices
  released", "Releasing…" while it runs, disabled while no device is Active); a running action keeps its label until
  it ends, and all labels share one grid cell so the button keeps the width of the longest. The right group is the
  flexible one (PageHeader `flexActions`): All brightness shrinks to its minimum before anything wraps, then the
  right group moves to a second row (kept at the right edge); below 964 px window width the left group takes the
  first row on its own and wraps. The colour popover and the "?" tooltip open right aligned (`align="end"`), and a
  closed tooltip takes no space, so nothing runs past the page edge.
  Visual direction: dark and spacious, few large elements, device preview in focus (ADR 0001 #15); components from
  the design system (`docs/design-system.md`, ADR 0001 #30).

### Colours across shutdown, sign-out and start (ADR 0001 #42)

- **Exit decision** (`power.rs`, `AppState::shutdown_engine`, exactly once per process). The setup hook installs
  `cc_system::session_end` on the main window and tao's event-target window; the exit path reads
  `session_end::current()` and decides with the pure `power::exit_plan`:

  | Why the app closes | Devices | Save to devices first (switch on) |
  |---|---|---|
  | `NotEnding`: tray Quit, `quit`, `--quit`, Update now | released | yes |
  | `CloseApp`: Restart Manager (installer, uninstaller) | released | no |
  | `ShutdownOrRestart`, `Unknown` (shutting down, no message seen) | kept (`ShutdownMode::Keep`) | yes |
  | `SignOut` | kept with "Keep colours when signing out" (default), else released | yes |

  Order: the autosave flush, then `EngineHandle::persist_all` when the switch is on, then
  `shutdown_with(mode, ..)`, then the device lock is let go. Budgets: on an explicit exit 5 s for the save and 3 s for
  the stop; at a session end or a Restart Manager close 1.5 s and 2 s (with the profile write well under Windows'
  5 s). A running switch of "Save colours to the devices" (`persist_lock`) is waited for at most 1 s at a session end
  (the wait is taken from the save's budget) and at most 12 s (its own budget plus a margin) on an explicit exit;
  after that the save is skipped and logged (a switch-off restore has already marked its devices pending). The
  uninstaller's quit (`ExitPlan::uninstall`) releases and never saves. The raw session message, `SM_SHUTTINGDOWN`,
  the switches and the decision are logged at info (`exit decision`).
- **Autostart** (`autostart.rs`): "Start with Windows" creates or deletes the per-user logon task
  (`TaskSpec::logon`, `<exe> --hidden`, `register_verified` / `delete_ours` in `cc-system`) inside the settings save;
  debug builds only change the setting and log. A task under our name that is not ours is reported with
  `cc_system::not_ours_message`, shown as it is. At start a release build checks on a background thread
  (`autostart::sync_at_start`; skipped while the settings cannot be saved, so an unreadable settings file never
  deletes the task, and serialised with the switch through `AppState::autostart_lock`): an old HKCU `Run\CaseControl`
  entry switched off in Task Manager, or a logon task of ours that is disabled (`TaskState::Disabled`), turns
  autostart off (a disabled task is never enabled again at start; only switching autostart on does); with autostart
  on the task is created when missing and updated in place when it differs (stale path); after that succeeded the old
  `Run\CaseControl` and `StartupApproved\Run\CaseControl` values are removed (`cc_conflicts::remove_own_run_entry`,
  HKCU only); with autostart off a task of ours is deleted. Task Manager's Startup apps no longer lists CaseControl.
- **Early apply**: `AppState::init` picks the active profile like the start does (`start_plan`, read-only), hands
  its visible device settings and its brightness (with the legacy overlay) to `EngineConfig::initial_settings` /
  `brightness` (`prefs::engine_config_for`), so each device is driven as soon as its backend found it.
  `start_profiles` then only does the bookkeeping (remember, legacy brightness) for that profile instead of applying
  it a second time; without profiles it still creates "Profile 1".
- **Cross-session device lock**: before it loads the machine state and starts its engine, the signed-in instance
  sets the per-user yield signal (`cc_system::request_yield`, kept open while it waits), which asks a running start-up
  instance to finish now, then takes `cc_system::DeviceLock` (waits at most 10 s, then starts anyway and logs it) and
  keeps it on the main thread until `shutdown_engine` stopped the engine. So the machine state is read after a
  start-up instance wrote its last records.
- **"Apply colours before sign-in"** (Settings, Startup; `set_apply_before_sign_in`, its own command so the settings
  lock is not held during the UAC prompt): on sends `RegisterBootTask`, off `RemoveBootTask` with entry
  `BOOT_TASK_ENTRY` straight to `elevated::run_admin_batch` (one UAC prompt, owned by the main window). The setting
  `apply_before_sign_in` changes only when the helper reports success; a declined prompt or a failure leaves it and
  the command rejects with the reason, shown in Settings (`commands::boot_task_error_text`: the helper's complete
  sentences, such as a prompt approved with another account, a request owned by Administrators or a task that is not
  ours, as they are; a short reason with "The start-up task was not created: "). After an `unknown` outcome (the
  helper still ran at its timeout) or when the setting cannot be saved, the task is read again and the switch follows
  it (`align_apply_before_sign_in`). At start (background thread, skipped while settings cannot be saved), when the
  task can be read, the switch is aligned the same way: missing or not ours turns it off, ours (also differing or
  disabled) turns it on; both logged. The switch and these checks hold `AppState::boot_task_lock`. Debug builds
  refuse ("Apply colours before sign-in is only available in installed builds."). The helper side uses
  `run_elevated_request_in` with `HelperContext::for_request` and writes its result with `write_helper_result`.
- **`--apply-at-boot`** (`boot.rs`, the start-up task, session 0, S4U): resolves the folders (the known folders when
  their `config.toml` exists, else `ProfileList\<own SID>` + `AppData\Roaming\CaseControl` /
  `AppData\Local\CaseControl\logs`, logged), logs to `boot-apply-YYYY-MM-DD.log` (UTC day, last 14 kept) in the
  logs folder together with the session, the token's elevation type and integrity level
  (`cc_system::token_elevation`), opens the yield signal, takes the device lock without waiting (held by the
  signed-in instance, or a yield already requested: exit at once), reads settings and the active profile read-only
  and bounded to 1 MiB per file (never creates or writes them; an unreadable or oversized settings file is not moved
  aside, an oversized profile file means no profile is applied, an oversized machine-state file means an in-memory
  state), starts the engine with hot-plug, `device_memory_writes: false` and the visible active profile as initial
  settings, waits at most 60 s until every profile device is present and Active (or faulted; a non-HID device not
  found by the first detection, e.g. the GPU when NvAPI is not available in session 0, and an absent device of a
  family switched off in Settings (`prefs::family_of`) are logged and skipped), stops waiting at once when the
  signed-in instance sets the yield signal, stops with `ShutdownMode::Keep`, lets go of the lock and exits. It never
  writes the devices' memory (no save, no restore; pending restores wait for the signed-in app). The machine state is
  written as usual (unreleased markers).
- **"Save colours to the devices"** (Settings, Hardware; `set_save_to_devices`): saves the switch, then on
  `persist_all(10 s)`, off `restore_saved(10 s)` (pending restores of absent devices stay and run when the device
  connects). Further triggers while on: tray Quit, `--quit`, Update now (before the installer), sign-out and
  shutdown/restart (see the table); never live edits, never `--apply-at-boot`. Every per-device result is logged;
  the devices that were not saved (can't store lighting, original state unknown, daily write limit reached, device
  error (faulted), failed, ran out of time, not confirmed in time, writing switched off, records cannot be stored) or
  not restored (failed, not connected, device error, daily limit) form one line in the settings view
  (`save_to_devices_note`, e.g. "Not saved: G560 (original state unknown); Wooting 60HE (can't store lighting).").
- **Uninstall** (`cleanup.rs`, `windows/hooks.nsh`): the NSIS `NSIS_HOOK_PREUNINSTALL` runs
  `"$INSTDIR\<exe>" --uninstall-cleanup` (plus `--no-prompt` in a silent `/S` uninstall) and waits, unless
  `$UpdateMode = 1` (an update) or the uninstaller runs in place from the install folder (`$EXEDIR == $INSTDIR`: the
  installer's reinstall page runs it with `_?=$INSTDIR` before installing again, while a real uninstall runs from
  NSIS's copy in `%TEMP%`; checked against the tauri-bundler 2.10 template). That mode asks a running instance to quit
  with `--quit-for-uninstall` (release, no save, no update install, the updater's files left alone; waits up to 15 s),
  takes the device lock (10 s), deletes the logon task, restores every device with a save backup (pending restores
  cleared first so the restore on connect and `restore_saved` never both run; the families of backed-up devices are
  detected even when switched off in Settings; engine without hot-plug, `restore_saved`, then release), and only if
  the start-up task exists (read back; when it cannot be read, the `apply_before_sign_in` setting decides) and the
  uninstall is not silent removes it through the elevated helper (one UAC prompt; declining is logged; a silent
  uninstall leaves the task and logs it). Best effort, always exit code 0.
- Settings page: group Startup ("Start with Windows", "Start minimized", "Apply colours before sign-in" with the hint
  "Needs administrator rights to turn on or off."), then group Hardware ("Keep colours when signing out"; "Save colours
  to the devices" with the hint "Devices that can store lighting show it from power-on. Writes their memory at most
  once per session." and the note line under it). Each of the two switches with their own command is disabled with
  `aria-busy` only while its own command runs (the other stays usable); errors show in the page's error notice as
  the backend gives them.

## Updates (`apps/desktop/src-tauri/src/updater.rs`, ADR 0001 #39, #41)

The app's only network access. Release builds only; debug builds never check, download or install.

- Endpoint `https://github.com/mkj777/CaseControl/releases/latest/download/latest.json` and the minisign public key
  are in `tauri.conf.json` (`plugins.updater`), with `requireSignedVersion: true` (the signature must carry the
  announced version, so a tampered `latest.json` cannot pair a new version number with an older signed installer)
  and `windows.installMode: "quiet"` (no installer UI). The NSIS installer is per user (ADR 0001 #32).
- Check: once after the start, on the `cc-updater` thread once the engine is up, then every 6 hours. A newer release
  is downloaded in the background and its signature verified by the plugin; it is then stored as the pending update
  in `%LOCALAPPDATA%\CaseControl\updates` (`pending.bin` + `pending.json` holding the version and the raw
  `latest.json`, both written via rename, the JSON last). The same version is not downloaded twice; a newer one
  replaces it.
- Install: on an explicit quit after the devices are released (`shutdown()` for tray Quit, the `quit` command and
  `--quit`; not on `RunEvent::Exit`, which also fires on logoff/shutdown where Windows may kill the installer) without
  restarting the app (`Restart::No`), and in the setup hook on the next start, before the engine opens any device,
  with a restart with the same arguments (`Restart::SameArgs`, e.g. `--hidden`). Before installing, the stored
  installer is verified again against the stored manifest (`Updater::restore_update` + `Update::verify`); a stale
  (not newer) or invalid one is deleted. The pending files stay until the installer really launched (on success the
  plugin ends the process with `std::process::exit` after `ShellExecuteW`, after Tauri's exit cleanup); before the
  start they are marked (`pending.attempted`), so an update whose installer ran without installing it is removed the
  next time instead of started again (no start/install loop), and an installer that cannot be started removes them
  at once.
- Every step is logged; failures (offline, GitHub down, bad signature) are retried at the next interval. No dialog.
- Settings, About group (ADR 0001 #41): next to Name, Version and Build an "Updates" row with the status line, then
  "Check for updates" and "Update now" (no heading of their own; the "Updates" heading above it is the frame rate
  group, as before). The status is an `UpdateView { status, pending, seq }` (`status`: `disabled {reason} | idle |
  checking | up_to_date | downloading {version} | ready {version} | installing {version} | failed {error}`, tagged
  by `state`; `pending`: the downloaded version waiting to be installed; `seq`: grows with every status change, the
  page keeps the view with the highest one, so a command reply cannot roll back a newer event). `ready` is never
  stored: `Updater::view` derives it from the pending store whenever nothing runs (stored idle or up to date), so it
  cannot outlive the update. `update_status` reads it on mount; every change is emitted as `update-status`. The
  disabled reason ("Updates are only available in installed builds") comes from the backend and is the status line
  and the check button's tooltip.
- "Check for updates" (`check_for_updates`) runs `Updater::check_once` now: the same check, download, signature
  check and `PendingStore` as the background thread. One running flag covers the background check, the button and
  Update now, so a check already running is not started twice (`CheckOutcome::AlreadyRunning`; the command then
  resolves at once with the current status).
- "Update now" (`install_update`, enabled only while `pending` is set and nothing runs) runs
  `install_update_now` on the main thread, like tray Quit and the `quit` command. It claims the exit (one flag for
  every explicit exit path: a tray Quit or `--quit` meanwhile is ignored, and Update now is refused while another
  exit runs), then `Updater::install_now` verifies the pending update and only then writes a pending profile change,
  releases every device through `AppState::shutdown_engine`, runs Tauri's exit cleanup and starts the quiet NSIS
  installer with `Restart::WithArgs(update_now_args)`: the current arguments without `--hidden` and `--quit`, so an
  autostarted (hidden) instance comes back with its window. The plugin's own restart can only pass this process's
  arguments, so it is off and the NSIS restart flags (`/R /ARGS ...`, escaped like the plugin does) are passed as
  installer arguments. If the installer cannot be started after the release, CaseControl starts itself again with
  the same filtered arguments (the single-instance lock is already released by the exit cleanup) and exits, so it
  comes back with its devices. A stale or invalid pending update, a running check or a missing update change
  nothing (the exit claim is given back) and the command rejects with the reason; debug builds reject too.
- The decisions live in `Updater<B: UpdateBackend>`; tests use a scripted backend and a temporary store.

## Testing

- `cc-core`: model invariants (zone ranges cover the buffer, mode caps vs state validation).
- `cc-drivers`: packet builders tested byte-exact against transcripts derived from the reference C++ (FakeTransport).
- `cc-conflicts`: scanners and system actions behind traits; tests inject fake process/service/autostart lists and a
  fake `SystemOps`, so no test terminates, stops or writes anything.
- Hardware tests are manual, run on this machine, documented in `docs/testing-hardware.md` as a checklist.

## CI (GitHub Actions, `windows-latest`)

Jobs on PR and push to main: `pnpm install --frozen-lockfile`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo nextest run --workspace`, `cargo deny check`, `pnpm vue-tsc --noEmit`, `pnpm build`, `cargo tauri build` (artifact on main).
Actions pinned by SHA, `permissions: contents: read`, `Swatinem/rust-cache`. Branch protection on main requires these checks.

Releases (`.github/workflows/release.yml`, see `docs/releasing.md`): a pushed tag `v*.*.*` first checks that the tag
matches the versions in `tauri.conf.json`, the workspace `Cargo.toml` and `package.json`, then runs `ci.yml` (as a
reusable workflow), then builds on `windows-latest` with `tauri-apps/tauri-action`, signs the updater artifacts with
the `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets and publishes the release (not a draft)
with the NSIS installer, its `.sig` and `latest.json`; the release notes are the tag message. Only that job has
`contents: write`.

## Security posture

No kernel driver, no elevation except the one user-triggered elevated helper run per conflict action (ADR 0001 #31)
and per switch of "Apply colours before sign-in" (ADR 0001 #42; also once at uninstall when the start-up task exists),
no service, no telemetry. The start-up task runs as the user with a limited token (S4U), never as SYSTEM or with
highest privileges. The only network access is the signed auto-update from GitHub releases (see Updates, ADR 0001 #39).
`cargo-deny` bans HTTP client crates (`reqwest`, `ureq`, `curl`, `isahc`, `attohttpc`, `surf`); `reqwest` is allowed
only as the direct dependency of `tauri-plugin-updater` (`wrappers`), which uses native TLS (SChannel) and enforces a license allowlist (licences compatible with the project's GPL-2.0-or-later).
All `unsafe` is confined to `cc-hid` (hot-plug and power notification FFI, named mutex), `cc-nvapi`, the single `sys` module of
`cc-conflicts` (Toolhelp, service control manager, process termination, ShellExecuteEx) and the single `sys` module of
`cc-system` (Task Scheduler COM, window subclassing, the cross-session mutex, token / WTS / account and file security
queries); each block carries a `// SAFETY:` comment.
