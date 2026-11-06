# Brief: `crates/cc-conflicts` (Phase 1)

You own `crates/cc-conflicts/**`. Stub `Cargo.toml` has `cc-core`, `serde`, `toml`, `thiserror`, `tracing`, and on
Windows `windows` with Services, Registry, ToolHelp, Threading, Shell and Security features (extend as needed).
The Win32 calls this crate needs (`CreateToolhelp32Snapshot`, `Process32NextW`, `OpenProcess`, `QueryFullProcessImageNameW`,
`OpenSCManagerW`, `EnumServicesStatusExW`, `QueryServiceConfigW`, `TerminateProcess`, `ShellExecuteW`) are `unsafe fn`
in the `windows` crate. Put all of them into one module declared as `#[allow(unsafe_code)] mod sys;` (`cfg(windows)`),
expose only safe functions from it, and keep every other module free of `unsafe`. Every `unsafe` block carries a
`// SAFETY:` comment (clippy denies undocumented ones). Registry access stays safe: use the `windows-registry` crate
(add it to your `Cargo.toml` with an explicit version and list it under deps added).

`cc-core` is being written concurrently; you only need `cc_core::family` ids (see `docs/briefs/phase1/01-cc-core.md`).
If it is not compiled yet, define the family id strings locally in a `const` and note it; the orchestrator will switch
the reference afterwards.

## Goal

The conflict view's data layer from `docs/architecture.md` section "Conflicts": an embedded catalog of software that
competes for RGB devices or low-level hardware access, scanners that detect it without elevation, findings that tie
evidence to catalog entries, and two opt-in actions. Facts come from `docs/research/03-conflicting-software-catalog.md`
and the local inventory in `docs/research/00-overview-and-local-inventory.md`; detection API constraints (what works
unelevated, `EnumDeviceDrivers` being useless on 24H2) are in research 03 section 6.

## Catalog (`catalog.toml`, embedded with `include_str!`)

Schema per entry:
```toml
[[entry]]
id = "signalrgb"                     # stable snake_case id
name = "SignalRGB"
vendor = "WhirlwindFX"
category = "rgb_suite"               # rgb_suite | vendor_rgb | hardware_monitor | fan_control | os_feature | driver
processes = ["SignalRgb.exe", "SignalRgbLauncher.exe", "SignalRgbService.exe"]
services = ["SignalRgb.Service"]     # Win32 services; glob '*' suffix allowed
drivers = ["SignalRgbDriver"]        # kernel driver service names
uninstall_names = ["SignalRgb"]      # substring match on DisplayName, case-insensitive
install_paths = ["%LOCALAPPDATA%\\VortxEngine"]
access = ["hid", "kernel_driver", "lamparray"]   # hid | smbus | superio | kernel_driver | lamparray | gpu_i2c
affects = ["msi_mystic_light", "wooting", "logitech", "nvidia_illumination"]   # cc_core::family ids, or "all"
severity = "high"                    # info | low | medium | high
verified = true                      # false when research marked the names UNVERIFIED
note = "Kernel driver auto-starts even when the app is closed; registers as a Windows Dynamic Lighting provider."
```
Cover every row of research 03 section 1 plus: `inpoutx64` (InpOut32-family port I/O driver, category `driver`,
severity high, note that OpenRGB documentation says to remove it), `pawnio` (category `driver`, severity `info`,
note: Microsoft-signed, used by FanControl and OpenRGB, cooperative; not a conflict by itself), `fancontrol`
(category `fan_control`, severity `info`, note: cooperates; overlap only through its device plugins), `winring0`
(driver `WinRing0x64`/`WinRing0_1_2_0`, high, Defender-flagged), `openrgb` (processes, port 6742), Windows Dynamic Lighting
as `category = "os_feature"`. Keep `verified = false` where research 03 says UNVERIFIED.

Loader: `Catalog::embedded() -> Catalog`, validated at load (unique ids, known enum values, every `affects` id is a
`cc_core::family` id or `"all"`); a test asserts the embedded catalog validates and has at least 25 entries.

## Scanners (traits + Windows implementations)

- `trait ProcessSource { fn processes(&self) -> Result<Vec<ProcessInfo>, ScanError>; }` with
  `ProcessInfo { pid: u32, name: String, exe_path: Option<String> }`. Windows: `CreateToolhelp32Snapshot` +
  `Process32FirstW/NextW`; path via `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`,
  tolerate access denied (path `None`).
- `trait ServiceSource { fn services(&self) -> Result<Vec<ServiceInfo>, ScanError>; }` with
  `ServiceInfo { name, display_name, kind: ServiceKind { Win32, Driver }, state: ServiceState { Running, Stopped, Other(u32) },
  start_type: Option<StartType { Boot, System, Auto, Manual, Disabled }>, binary_path: Option<String> }`.
  Windows: `OpenSCManagerW(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE)`, `EnumServicesStatusExW` with
  `SERVICE_WIN32 | SERVICE_DRIVER`, `SERVICE_STATE_ALL`; `QueryServiceConfigW` for start type and path, tolerate denied.
- `trait RegistrySource { fn installed_apps(&self) -> Result<Vec<InstalledApp>, ScanError>; fn dynamic_lighting(&self) -> Result<Option<DynamicLighting>, ScanError>; }`
  with `InstalledApp { display_name, version: Option<String>, publisher: Option<String>, install_location: Option<String>, hive: Hive { HklmNative, HklmWow6432, Hkcu } }`
  and `DynamicLighting { ambient_enabled: bool, foreground_controls: bool, providers: Vec<String>, device_count: usize }`
  (`HKCU\Software\Microsoft\Lighting`, `...\Providers`, `...\Devices\*`). Windows: the `windows-registry` crate.
- `OpenRgbProbe` (optional, `ScanOptions.check_openrgb_port: bool`, default `false`): `TcpStream::connect_timeout`
  to `127.0.0.1:6742` with 200 ms. This is the only socket use allowed in the whole project (ADR 0001 #13) and must stay
  behind the option.
- `Scanner::new(catalog, sources...)` and `fn scan(&self, opts: &ScanOptions) -> ScanReport { findings: Vec<Finding>, errors: Vec<String> }`.
  Matching: process by exe name case-insensitive; service/driver by name case-insensitive with `*` suffix glob;
  installed by substring; path by existence after env expansion. `Finding { entry_id, name, vendor, category, severity,
  verified, access, affects, running: bool, installed: bool, evidence: Vec<Evidence> }` with
  `Evidence { Process { pid, name, path }, Service { name, state, kind }, Driver { name, state, path }, Installed { display_name, version, hive }, Path(String), DynamicLighting(DynamicLighting), OpenRgbPort }`.
  Dynamic Lighting always yields a finding of the `windows_dynamic_lighting` entry when the key exists, with evidence
  including the provider list (SignalRGB appears there on this machine). Sort findings: running first, then severity.

## Actions (opt-in, called only from an explicit UI click)

- `pub fn terminate_process(pid: u32, expected_name: &str) -> Result<(), ActionError>`: re-read the process name for
  `pid`, refuse if it differs (`ActionError::NameMismatch`), then `OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess`.
  Same-user only, no elevation. `tracing::warn!` before acting with pid, name, and the reason string the caller passes.
- `pub fn request_stop_service(name: &str) -> Result<(), ActionError>`: `ShellExecuteW` verb `runas`, file `sc.exe`,
  parameters `stop "<name>"`; validate `name` against `^[A-Za-z0-9_.\- ]+$` first; this shows UAC and returns immediately.
  Log the same way. Document that stopping does not disable autostart (ADR 0001 #12 forbids that).

## Tests

- Catalog validation (parses, unique ids, `affects` ids valid, at least 25 entries, every entry has at least one detection key).
- Matching with fake sources that reproduce this machine's inventory from research 00: running `FanControl.exe`,
  `lghub_updater.exe`, `logi_lamparray_service.AMD64.exe`; services `FanControl.Service` stopped, `SignalRgb.Service` stopped,
  `LGHUBUpdaterService` running, `logi_lamparray_service` running; drivers `inpoutx64` running, `PawnIO` running,
  `SignalRgbDriver` running; installed `SignalRgb 2.5.84`, `FanControl 285`, `PawnIO 2.0.1`, `Logitech G HUB`,
  `SteelSeries GG`; Dynamic Lighting providers `["SignalRGB_4n6rtbnm9e9y2", "WindowsLighting"]`.
  Assert: SignalRGB finding `running = true` with a Driver evidence and the Dynamic Lighting provider; inpoutx64 finding high;
  PawnIO finding severity info; FanControl finding running with severity info; sort order.
- Glob and case-insensitivity of service matching; `terminate_process` name-mismatch path with a fake lookup
  (make the name lookup injectable so the refusal is testable without killing anything).
- `#[cfg(windows)]` smoke: real sources return `Ok`; a full `scan` with default options returns `Ok` (no content asserts).

## Example (not run in CI)

`examples/scan.rs`: runs a real scan and prints findings as a table. The orchestrator runs it and compares with the inventory.
