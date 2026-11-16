# Brief: `apps/desktop` (Phase 2/3)

Read `00-common-rules.md` first. You own `apps/desktop/**` (Rust in `src-tauri`, Vue in `src`). Read the finished
crates you build on: `cc-engine` (EngineHandle, DeviceView, DeviceStatus, EngineEvent), `cc-config` (Paths, Settings,
ProfileStore, Profile, DeviceSetting), `cc-conflicts` (Scanner, Finding, actions), `cc-drivers::all_backends()`.
Owner decisions you must follow: ADR 0001 #3, #12 to #16 and #18 to #25. Do not add npm or cargo dependencies beyond
`cc-*` workspace crates, `crossbeam-channel`, `parking_lot` and what is already in the manifests; if one seems
unavoidable, stop short of adding it and report it.

## Rust side (`src-tauri`)

- Startup (`setup`): `Paths::from_env()` + `ensure()`, `Settings::load`, `ProfileStore`, start the engine with
  `cc_drivers::all_backends()`, frame rate, disabled families and zone sizes from settings, hot-plug on. Then, per
  ADR #24, if `last_profile` names an existing profile, `engine.apply` it. Keep everything in one managed `AppState`.
  Engine start failure must not stop the app: show it on the Devices page instead.
- Every exit path (tray Quit, `quit` command, `RunEvent::ExitRequested`/`Exit`, Windows session end) goes through one
  `shutdown()` that calls `engine.shutdown(3 s)` exactly once and then exits. Hide-to-tray on close stays.
- Commands (errors as `String`): `list_devices`, `set_direct`, `set_zone`, `set_mode`, `resize_zone` (also persists
  the size by zone name into `settings.devices`), `save_to_hardware`, `release_device`, `release_all`, `rescan`,
  `list_profiles`, `save_profile(name)` (from `engine.current_settings()`), `apply_profile(name)` (sets and saves
  `last_profile`), `rename_profile`, `delete_profile` (clears `last_profile` if it was that one), `scan_conflicts`,
  `terminate_process(pid, finding)`, `stop_service(name, finding)`, `get_settings`, `set_settings` (applies frame rate
  and detector changes to the running engine; autostart via `tauri-plugin-autostart` enable/disable),
  plus the existing `app_info`, `show_window`, `hide_to_tray`, `quit`.
- Events: forward engine events as `device-added`, `device-removed`, `device-faulted`, `device-state-changed`;
  `profiles-changed` after save/rename/delete; `conflicts-updated` after a scan.
- Tray: Show, Apply profile (submenu of profile names, rebuilt on `profiles-changed`; a check mark on `last_profile`),
  Release devices (`release_all`), Quit.
- Keep commands thin: logic that can be unit tested (e.g. settings patching, zone-size persistence, profile/tray model)
  lives in plain functions with tests.

## Frontend (`src`)

Visual direction (ADR #15, #23): dark, spacious, few large elements, neutral greys and white only; the device colours
in the previews are the only colour in the UI. Replace `--color-accent` with a neutral. No gradients, glows, glass,
emoji or decorative icons; system font; restrained motion. It should read like a calm native tool, not a gaming app.

- Shell: existing sidebar navigation with Devices, Profiles, Conflicts, Settings.
- Devices overview (ADR #18): grid of large tiles, one per device: name, kind, status (Active/Released/Faulted), a
  small live preview of its current colours. Empty state when nothing is detected (with a Rescan button and a hint to
  close other RGB software, linking to Conflicts). Engine start error shown here.
- Device view (route with the device id; back arrow to the overview): a large preview as the focus.
  Matrix zones draw as a key grid from `ZoneKind::Matrix.map`, linear zones as rows of LED dots per zone with the zone
  name, single zones as one larger swatch. Selection (ADR #22): click selects one LED, drag sweeps several, Ctrl-click
  adds/removes; zone name click selects the zone; "All" and "None". Without a selection the colour applies to the
  whole device.
- Colour (ADR #20, #21): a row of swatches (white, red, orange, yellow, green, cyan, blue, purple, magenta, off) and a
  fold-out picker below: saturation/value area, hue bar, hex input. Changes go live while dragging (call `set_direct`
  with the full buffer, throttled with requestAnimationFrame; the engine coalesces further). Write it as a small set of
  components with pure colour/selection helpers in plain TS modules.
- Mode menu (ADR #25): "Static (per LED)" for the direct mode plus the device's hardware modes; for a hardware mode
  show only the controls its caps allow (speed, brightness, direction, mode colours with the same swatch/picker).
- Resizable zones (ARGB headers): an LED-count field per zone in the device view (within the zone's range).
- Device actions: Release (hand back to hardware), Save to device (only if the driver supports it; explain that it
  writes the device's flash), status and fault message.
- Profiles page: list with the active one marked, Apply, Save current as..., Rename, Delete (in-app confirm, never
  `window.confirm`/`alert`).
- Conflicts page: Scan button and scan on open; findings grouped by severity with name, vendor, evidence, whether it is
  running and which of our devices it affects; actions "End process" / "Stop service" behind an in-app confirm that
  says what will happen (service stop shows a UAC prompt). No autostart changes (ADR #12).
- Settings page: start with Windows, start minimized, frame rate (1 to 60), detector toggle per family, OpenRGB port
  check, app info.
- `ipc.ts` mirrors every command and payload type; keep TS types in sync with the Rust serde output (check the actual
  JSON shape of `DeviceView`, `ModeState`, `Rgb` ("#RRGGBB"), `ZoneKind`, `DeviceStatus`, `Finding`).

## Checks

`pnpm typecheck`, `pnpm build`, `cargo clippy -p casecontrol-desktop --all-targets -- -D warnings`,
`cargo test -p casecontrol-desktop`, `cargo fmt`. Do not launch the app (the orchestrator does that with hardware).
