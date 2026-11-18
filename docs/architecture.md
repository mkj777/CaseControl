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
  cc-config/               settings and profiles (TOML in %APPDATA%\CaseControl), paths, migration
  cc-engine/               runtime: detection, per-device worker threads, frame scheduler, profiles, event bus
apps/
  desktop/                 Tauri 2 app
    src-tauri/             tauri commands, tray, autostart, single instance, logging setup
    src/                   Vue 3 + TypeScript + Vite (pnpm)
docs/                      research, decisions, this file
reference/openrgb/         copied GPLv2 sources used as protocol reference
.github/workflows/ci.yml
```

Dependency direction: `cc-core` <- `cc-hid`, `cc-nvapi`, `cc-config`, `cc-conflicts` <- `cc-drivers` <- `cc-engine` <- `apps/desktop`.
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
| `msi_mystic_light` | `MSIMotherboardController/*` | Detect on `1462:7D75` usage page 0x0001 usage 0x00; class by `get_feature_report(0x52)` length (185/162/112, else 761 path). Direct mode per ARGB header via feature reports. Only PIDs in the upstream tested list; no `ENABLE_UNTESTED` set. Port the `GetFirmwareVersionInvalid` gate verbatim (it separates the tested path from the brick-era one). The save-to-flash byte inside the mode packet is always zero except in `save_to_hardware()`. Last good own lighting (ADR 0001 #38): without the "unreleased" marker, a 185-byte config read at open that is not per-LED (no zone in effect 0x25) is reported as `own_lighting` (save byte 0), and a per-LED one belongs to another program (nothing stored or restored, an untouched release sends nothing). With the marker, the readback is CaseControl's: never stored; the stored state, if it passes `ConfigPacket::from_own_lighting` (185 bytes, report id 0x52, save byte 0, no zone in effect 0x25), is restored at open (so board and UI agree) and by `release()`; without one, `shows_own_lighting()` is false and the marker stays. The off output (Static 0) also darkens headers sized 0. `off_mode` is Static (brightness 0 = Static at brightness 0, no per-LED). | LibreHardwareMonitor's MSI HID controller only targets CoreLiquid AIOs (PIDs 0x6A04..0x6A16), so FanControl does not touch this device. |
| `wooting` | `WootingKeyboardController/*V3*` | `31E3:1310`, usage page 0xFF55 (V3). Direct per-key RGB, restore keyboard profile on release. |
| `logitech` | `LogitechController/LogitechHIDPP20Controller`, `LogitechLightspeedController`, `LogitechProtocolCommon` | G560 `046D:0A78` usage page 0xFF43 (zone-only HID++ 2.0, feature 0x8070). PRO Wireless through receiver `046D:C539` (Lightspeed, virtual PID 0x4079); the receiver accepts one connection, pair-index handling per upstream. |
| `nvidia_illumination` | `NVIDIAIlluminationController/*`, `dependencies/NVFC/nvapi.*` | Load `nvapi64.dll`, `NvAPI_GPU_ClientIllumZonesGetControl/SetControl`. Match `10DE:2C02` subsys `10DE:2095`. RGBW zones treated as RGBW. NvAPI Illumination is undocumented: function IDs and `#[repr(C)]` struct layouts/versions are copied byte-exact from `nvapi.h`. Always `GetControl` before any `SetControl`, keep the initial params, and `release()` writes them back. First hardware test is a round-trip (read, write the same values, read again) before any color change. |

Hardware checklist cases that every driver must pass on this machine (manual, documented in `docs/testing-hardware.md`):
FanControl running; `logi_lamparray_service` running and stopped (Logitech); G HUB running and closed (Logitech);
re-plug of the device while the app runs (hot-plug, profile re-association by `DeviceId`); app quit restores firmware mode.

## Engine (`cc-engine`)

- `Engine::start(config) -> EngineHandle`. Runs detection once, then owns one worker thread per controller with a
  bounded command channel (`SetDirect`, `SetZone`, `SetMode`, `Resize`, `Save`, `Release`, `Shutdown`).
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
  `%APPDATA%\CaseControl\hardware-state.toml`, in memory only without storage), hands its map to every detection
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
  # CaseControl machine state (ADR 0001 #38). Written by CaseControl; not meant to be edited.
  unreleased = ["hid:1462:7d75:7D7522120106"]

  [own_lighting]
  "hid:1462:7d75:7D7522120106" = "5201ff0000..."   # lower-case hex ([0-9a-f]), no separators
  ```

  The file is rewritten atomically, only when a value changed, and the in-memory state changes only after the write
  succeeded. Content that is not TOML, an `own_lighting` that is not a table, an `unreleased` that is not an array
  and invalid entries are ignored with a warning; unknown keys are dropped on the next write. A file that exists but
  cannot be read (I/O error, not UTF-8) makes the state read-only for the session: it is never overwritten.
- Error policy: write errors back off exponentially (50 ms .. 5 s), after 10 consecutive failures the device is marked
  `Faulted` and shown as such; never retried in a tight loop. A faulted device is released once, best effort, when
  its worker stops: on shutdown, and when detection is about to open it again. Detection stops the faulted workers
  of the backends it runs (all stops sent first, then one deadline, `Timing::faulted_release`, 500 ms); a device
  whose old worker has not ended by then is skipped in that pass and re-opened by a later one, so an old worker never
  writes after a new controller opened the device. A removed device is not released: its handle is gone; the
  re-opened controller finds the board's state itself (with the "unreleased" marker telling it the state is ours).
- Shutdown: `Release` on every device (also faulted ones, best effort), join threads with timeout, then exit.
  Ctrl-C/WM_CLOSE/tray Quit all funnel here.
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
- Matching: catalog `processes` match executable file names; names that are too generic on their own
  (`generic_processes`, e.g. `CMService.exe`, `RunLed.exe`, `LightingService.exe`) match only when the full path
  contains one of the entry's `path_hints`.

## Config (`cc-config`)

`%APPDATA%\CaseControl\config.toml`, `profiles\<name>.toml`, `hardware-state.toml` (machine state, not settings:
the last good own lighting per device, ADR 0001 #38; see Engine), logs in `%LOCALAPPDATA%\CaseControl\logs`.
Settings: `autostart`, `start_minimized`, `frame_rate`, `detectors.<family> = true|false`, `conflicts.check_openrgb_port`,
`conflicts.dont_ask` (catalog ids the start dialog no longer asks about), `conflicts.previous_start_types` (service name
-> start type before CaseControl set it to Manual), `last_profile`. Profile: per `DeviceId` a `ModeState` or a direct color buffer, plus the device brightness per `DeviceId`. Unknown keys preserved.

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

- Tauri 2, features `tray-icon`; plugins: `tauri-plugin-autostart` (HKCU Run), `tauri-plugin-single-instance`,
  `tauri-plugin-updater` (see Updates), `tracing` to a rolling log file. CSP strict, no remote URLs, `dangerousRemoteDomainIpcAccess` empty.
- Close hides to tray; tray menu: Show, Apply profile submenu, Release devices, Quit.
- Command line (parsed by `LaunchArgs::parse`; exact matches, unknown arguments ignored): `--hidden` starts with the
  window hidden (the autostart entry passes it). `--quit` asks a running instance to shut down: the single-instance
  plugin decides inside `tauri::Builder::build`, before the window, tray, engine or device detection exist (they start
  in the setup hook, which only `App::run` runs). With an instance running, the second process forwards its argv and
  exits there; the running instance's callback calls `shutdown()` (devices released once, then exit) before the
  forward returns, so the `--quit` process usually ends after the release. With none running, `run()` releases the
  single-instance lock and returns without running the app (exit code 0). `--elevated-actions` is handled separately
  by `main` (see Conflicts).
- Commands: `list_devices`, `set_direct`, `set_zone`, `set_mode`, `resize_zone`, `save_to_hardware`, `release_device`,
  `release_all`, `connect_all`, `set_device_brightness`, `set_master_brightness`, `rescan`, `set_device_hidden`, `set_zone_hidden`, `list_profiles/apply_profile/
  create_profile/duplicate_profile/rename_profile/delete_profile`, `scan_conflicts`, `startup_conflicts`, `resolve_conflicts`, `close_program`,
  `set_autostart`, `set_dont_ask`, `get_settings/set_settings`, `set_default_color`, `update_status`,
  `check_for_updates`, `install_update` (see Updates). Shapes that changed with ADR 0001
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

No kernel driver, no elevation except the one user-triggered elevated helper run per conflict action (ADR 0001 #31),
no telemetry. The only network access is the signed auto-update from GitHub releases (see Updates, ADR 0001 #39).
`cargo-deny` bans HTTP client crates (`reqwest`, `ureq`, `curl`, `isahc`, `attohttpc`, `surf`); `reqwest` is allowed
only as the direct dependency of `tauri-plugin-updater` (`wrappers`), which uses native TLS (SChannel) and enforces a license allowlist (licences compatible with the project's GPL-2.0-or-later).
All `unsafe` is confined to `cc-hid` (hot-plug and power notification FFI, named mutex), `cc-nvapi`, and the single `sys` module of
`cc-conflicts` (Toolhelp, service control manager, process termination, ShellExecuteEx); each block carries a `// SAFETY:` comment.
