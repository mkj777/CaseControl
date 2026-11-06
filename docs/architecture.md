# Architecture (v1)

Target: Windows 11, x86_64. One process. Rust workspace plus a Tauri 2 desktop app with a Vue 3 frontend.
Design goals in priority order: never damage hardware, never interfere with FanControl or other tools, no
network, no admin, low idle cost, a backend boundary that admits SMBus/PawnIO later.

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
                  pub resizable: Option<std::ops::RangeInclusive<usize>> }
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
| `msi_mystic_light` | `MSIMotherboardController/*` | Detect on `1462:7D75` usage page 0x0001 usage 0x00; class by `get_feature_report(0x52)` length (185/162/112, else 761 path). Direct mode per ARGB header via feature reports. Only PIDs in the upstream tested list; no `ENABLE_UNTESTED` set. Port the `GetFirmwareVersionInvalid` gate verbatim (it separates the tested path from the brick-era one). The save-to-flash byte inside the mode packet is always zero except in `save_to_hardware()`. LibreHardwareMonitor's MSI HID controller only targets CoreLiquid AIOs (PIDs 0x6A04..0x6A16), so FanControl does not touch this device. |
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
- Error policy: write errors back off exponentially (50 ms .. 5 s), after 10 consecutive failures the device is marked
  `Faulted` and shown as such; never retried in a tight loop.
- Shutdown: `Release` on every device, join threads with timeout, then exit. Ctrl-C/WM_CLOSE/tray Quit all funnel here.
- Event bus (`tokio::sync::broadcast` or crossbeam) consumed by the Tauri layer and re-emitted as Tauri events.

## Conflicts (`cc-conflicts`)

- Catalog `catalog.toml` embedded via `include_str!`, entries from `docs/research/03-conflicting-software-catalog.md`:
  `name, vendor, processes[], services[], drivers[], uninstall_keys[], paths[], access = [hid|smbus|superio|lamparray],
  affects = [device family ids], severity, note`. Unverified names are kept but flagged `verified = false`.
- Scanners (all unelevated): processes (`sysinfo` or Toolhelp), services and driver services
  (`EnumServicesStatusEx(SERVICE_WIN32 | SERVICE_DRIVER)`), installed apps (Uninstall keys incl. Wow6432Node and HKCU),
  Dynamic Lighting (`HKCU\Software\Microsoft\Lighting` incl. `Providers`), OpenRGB SDK port (TCP connect 127.0.0.1:6742
  is the single allowed local socket use and is behind a setting, default off).
- Output: `Finding { entry, evidence: Vec<Evidence>, running: bool, affects_our_devices: Vec<DeviceId> }`.
- Actions (opt-in per click): `terminate_process(pid)` (same user, no elevation), `stop_service(name)` via
  `ShellExecuteW("runas", "sc.exe", "stop <name>")` which shows UAC. Every action is logged with the finding it came from.
- The first findings on this machine will be: `SignalRgbDriver` (driver running, app closed), `inpoutx64` (port I/O driver
  running), SignalRGB as Dynamic Lighting provider, G HUB updater, Logitech LampArray service.

## Config (`cc-config`)

`%APPDATA%\CaseControl\config.toml`, `profiles\<name>.toml`, logs in `%LOCALAPPDATA%\CaseControl\logs`.
Settings: `autostart`, `start_minimized`, `frame_rate`, `detectors.<family> = true|false`, `conflicts.check_openrgb_port`,
`last_profile`. Profile: per `DeviceId` a `ModeState` or a direct color buffer plus per-zone overrides. Unknown keys preserved.

## Desktop app (`apps/desktop`)

- Tauri 2, features `tray-icon`; plugins: `tauri-plugin-autostart` (HKCU Run), `tauri-plugin-single-instance`,
  `tauri-plugin-log` or `tracing` bridge. CSP strict, no remote URLs, `dangerousRemoteDomainIpcAccess` empty.
- Close hides to tray; tray menu: Show, Apply profile submenu, Release devices, Quit.
- Commands: `list_devices`, `set_direct`, `set_zone`, `set_mode`, `resize_zone`, `save_to_hardware`, `release_device`,
  `list_profiles/save_profile/apply_profile/delete_profile`, `scan_conflicts`, `terminate_process`, `stop_service`,
  `get_settings/set_settings`, `rescan`. Events: `device-added`, `device-removed`, `device-faulted`, `conflicts-updated`.
- Frontend: Vue 3 + TS + Vite, pnpm, `@tauri-apps/api`. English UI. Pages: Devices, Profiles, Conflicts, Settings.
  Visual direction: dark and spacious, few large elements, device preview in focus (ADR 0001 #15).

## Testing

- `cc-core`: model invariants (zone ranges cover the buffer, mode caps vs state validation).
- `cc-drivers`: packet builders tested byte-exact against transcripts derived from the reference C++ (FakeTransport).
- `cc-conflicts`: scanners behind traits, tests inject fake process/service lists.
- Hardware tests are manual, run on this machine, documented in `docs/testing-hardware.md` as a checklist.

## CI (GitHub Actions, `windows-latest`)

Jobs on PR and push to main: `pnpm install --frozen-lockfile`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo nextest run --workspace`, `cargo deny check`, `pnpm vue-tsc --noEmit`, `pnpm build`, `cargo tauri build` (artifact on main).
Actions pinned by SHA, `permissions: contents: read`, `Swatinem/rust-cache`. Branch protection on main requires these checks.

## Security posture

No kernel driver, no elevation except the user-triggered service stop, no network, no telemetry, no auto-update.
`cargo-deny` bans HTTP client crates (`reqwest`, `ureq`, `curl`; the exact list is derived from `cargo tree` after the Tauri
scaffold exists, since Tauri may pull `hyper` transitively) and enforces a license allowlist (GPL-compatible is fine, project is private).
All `unsafe` is confined to `cc-hid` (hot-plug FFI, named mutex), `cc-nvapi`, and the single `sys` module of
`cc-conflicts` (Toolhelp, service control manager, process termination, ShellExecute); each block carries a `// SAFETY:` comment.
