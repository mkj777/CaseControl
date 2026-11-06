# Brief: `crates/cc-hid` (Phase 1)

You own `crates/cc-hid/**`. The stub `Cargo.toml` already depends on `cc-core`, `thiserror`, `tracing`,
`crossbeam-channel`, and on Windows `hidapi` (feature `windows-native`) and `windows` with a starting feature list that
you may extend. `#![allow(unsafe_code)]` is set in `lib.rs`; this crate and `cc-nvapi` are the only places unsafe code may live.

`cc-core` is being written concurrently by another worker. Its public API is specified in
`docs/briefs/phase1/01-cc-core.md`; code against that specification (`HotplugEvent`, `HotplugAction`, `HotplugHandle`,
`HotplugSender` live in `cc_core::backend`). If `cc-core` does not compile yet when you start, write and unit-test
everything that does not need it first, then integrate; if it is still missing at the end, report exactly which items
you could not compile against.

## Goal

The HID layer from `docs/architecture.md` section "HID layer": a transport trait, the hidapi implementation,
a scripted fake for tests, enumeration and matching, Windows hot-plug notifications, and a named-mutex guard.
Never open a device exclusively: hidapi opens with `FILE_SHARE_READ | FILE_SHARE_WRITE`, which is what coexistence
with FanControl requires (see `docs/research/02-fancontrol-coexistence.md`). Document this in the crate docs.

## Modules and contracts

- `transport`: `trait HidTransport: Send { write, read_timeout, send_feature_report, get_feature_report }` exactly as in
  the architecture (`data[0]`/`buf[0]` is the report id). `HidError` (thiserror): `NotFound(String)`, `Io(String)`,
  `Timeout`, `Disconnected`, `InvalidReport { expected: usize, got: usize }`, `Unsupported(&'static str)`,
  `Script(String)` (fake transport mismatches). `impl From<HidError> for cc_core::DeviceError` (Disconnected -> Disconnected,
  Timeout -> Timeout, rest -> Transport).
- `info`: `HidDeviceInfo { path: String, vid: u16, pid: u16, interface: i32, usage_page: u16, usage: u16,
  serial: Option<String>, product: Option<String>, manufacturer: Option<String>, release: u16 }`.
  `HidMatch { vid: u16, pid: Option<u16>, interface: Option<i32>, usage_page: Option<u16>, usage: Option<u16> }` with
  `fn matches(&self, &HidDeviceInfo) -> bool` and `const fn` constructors mirroring OpenRGB's `REGISTER_HID_DETECTOR`,
  `_I`, `_P`, `_PU`, `_IP`, `_IPU` variants (see `reference/openrgb/core/DetectionManager.h`).
- `hidapi` backend (`#[cfg(windows)]`, module `backend`): one process-wide `hidapi::HidApi` in a `OnceLock<Mutex<..>>`,
  created with `HidApi::new_without_enumerate()`; `pub fn enumerate() -> Result<Vec<HidDeviceInfo>, HidError>` refreshes
  the device list (`reset_devices` + `add_devices(0, 0)`) and maps every `DeviceInfo`; `pub fn open(path: &str) ->
  Result<Box<dyn HidTransport>, HidError>` via `open_path`. The hidapi device is `Send` but not `Sync`; the transport
  wrapper holds it and is used from one thread. `read_timeout` maps `Duration` to milliseconds (cap at `i32::MAX`).
  On non-Windows provide the same functions returning `HidError::Unsupported`.
- `fake`: `FakeTransport` built from a script of `Exchange` steps that are consumed strictly in order:
  `ExpectWrite(Vec<u8>)`, `Read(Vec<u8>)`, `ReadTimeout`, `ExpectFeature(Vec<u8>)`, `FeatureReply { report_id: u8, data: Vec<u8> }`.
  A mismatch returns `HidError::Script` with a hex dump of expected vs actual (both full buffers) and marks the fake as
  failed; `fn finish(self) -> Result<(), HidError>` fails if steps remain. Add a `recorded: Vec<Vec<u8>>` accessor for
  tests that prefer asserting after the fact. Provide a `ScriptBuilder` with chainable helpers.
- `recording`: `RecordingTransport<T: HidTransport>` wrapping any transport and logging each exchange as a hex line via
  `tracing::debug!` (target `cc_hid::record`) and into an in-memory `Vec<Record>` the caller can drain. This is how the
  orchestrator will capture transcripts from real devices for driver tests.
- `hotplug` (`#[cfg(windows)]`): `pub fn watch(tx: cc_core::backend::HotplugSender) -> Result<Box<dyn HotplugHandle>, HidError>`.
  Implementation: `CM_Register_Notification` with a `CM_NOTIFY_FILTER` of type `CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE`
  for `GUID_DEVINTERFACE_HID`; the callback receives `CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL` / `...REMOVAL` with the
  symbolic link in `u.DeviceInterface.SymbolicLink`. The callback must do minimal work: copy the link, action, and send
  on an internal channel. A watcher thread debounces per `(vid, pid)` for 500 ms and then forwards one `HotplugEvent`
  per burst. `stop()` calls `CM_Unregister_Notification` from the watcher thread, never from inside the callback.
  Pure function `pub fn parse_symbolic_link(link: &str) -> Option<(u16, u16)>` extracting `VID_xxxx&PID_xxxx` case-insensitively;
  test it with these real links from this machine:
  `\\?\HID#VID_1462&PID_7D75#9&c9e9596&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}`,
  `\\?\HID#VID_046D&PID_C539&MI_02&Col02#b&a5fc8a&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}`,
  `\\?\HID#VID_31E3&PID_1310&MI_02#b&3ab1eb27&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}`.
  Every `unsafe` block gets a `// SAFETY:` comment naming the pointer/lifetime invariant it relies on.
- `mutex` (`#[cfg(windows)]`): `NamedMutex::open_or_create(name: &str) -> Result<NamedMutex, HidError>` using
  `CreateMutexW` (creating if absent, otherwise opening the existing one; `ERROR_ACCESS_DENIED` on create falls back to
  `OpenMutexW(SYNCHRONIZE | MUTEX_MODIFY_STATE)`), `fn lock(&self, timeout: Duration) -> Result<MutexGuard<'_>, HidError>`
  with `WaitForSingleObject`; `WAIT_ABANDONED` counts as acquired (same policy as LibreHardwareMonitor's `Mutexes.cs`,
  see `docs/research/02-fancontrol-coexistence.md`); guard releases with `ReleaseMutex` on drop; handle closed on drop.
  Provide the well-known names as constants: `GLOBAL_SMBUS`, `GLOBAL_ISABUS`, `GLOBAL_PCI`, `GLOBAL_EC`,
  `GLOBAL_CORSAIR_LINK` (`Global\Access_SMBUS.HTP.Method`, `Global\Access_ISABUS.HTP.Method`, `Global\Access_PCI`,
  `Global\Access_EC`, `Global\CorsairLinkReadWriteGuardMutex`). Unused by v1 drivers; present for the later tier.

## Tests

- `FakeTransport`: a scripted happy path; write mismatch produces `Script` error with hex dump; leftover steps fail `finish`;
  `ReadTimeout` yields `Timeout`.
- `HidMatch::matches` across all wildcard combinations.
- `parse_symbolic_link` on the three links above and on garbage.
- `RecordingTransport` records writes and feature reports in order.
- `#[cfg(windows)]` smoke tests: `enumerate()` returns `Ok` (never opens anything); `hotplug::watch` registers, sleeps 100 ms,
  `stop()`s without panicking; `NamedMutex::open_or_create("Local\\CaseControlTest")` locks and unlocks twice.

## Example (not run in CI)

`examples/list_hid.rs`: prints every enumerated device as `vid:pid iface usage_page/usage product serial path`.
The orchestrator runs it on the development machine to confirm the Mystic Light, Wooting and Logitech interfaces appear.
