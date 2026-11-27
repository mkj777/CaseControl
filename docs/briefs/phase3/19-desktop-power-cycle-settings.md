# Brief: keep colours, logon autostart, before-sign-in, save to devices in the app (ADR 0001 #42, part 3 of 3)

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #3, #31, #32, #33, #34, #38, #40, #41, #42, `docs/design-system.md`,
`docs/architecture.md`, briefs 17 and 18 in this folder and their final reports (given to you below), then the scout
report `C:\Users\maxik\AppData\Local\Temp\claude\390fc6e9cd42ba40\ba8ab383-1ff6-47cc-843f-8afd412f1190\scratchpad\scout\desktop.md`.
You own `apps/desktop` (Rust and Vue), `docs/architecture.md`, `README.md`, `docs/testing-hardware.md`. Small fixes in
`cc-engine`, `cc-system` or `cc-conflicts` are allowed only when an API from briefs 17/18 does not fit; report each. No new
third-party dependencies (removing `tauri-plugin-autostart` is fine). Do not launch, tauri-build or kill the app; nothing
that writes to HID or NvAPI; do not create or delete real scheduled tasks or registry values (tests use fakes). No commit.

## 1. Session end (A, A2)

- Install `cc-system`'s session-end watch on the main thread in `setup` (main window HWND plus tao's event-target
  window).
- Exit handler (`RunEvent::Exit` / `ExitRequested`), decided by the session-end kind:
  - NotEnding (tray Quit, `--quit`, Update now): today's path, release.
  - CloseApp (Restart Manager, i.e. our installer/uninstaller): release.
  - ShutdownOrRestart or Unknown: keep (`ShutdownMode::Keep`).
  - SignOut: keep when "Keep colours when signing out" is on (default), else release.
  - Before stopping: flush autosave as today; when "Save colours to the devices" is on, persist first (brief 17 API)
    within the time budget; total work at session end must stay well under 5 s.
  - Log the raw flags and the decision at info, so the owner's test runs show what Windows sent.

## 2. Logon task autostart (B)

- "Start with Windows" now manages the per-user logon task (brief 18) instead of the HKCU Run entry; keep the
  debug-build guard (debug builds only change the setting and log). At start in release builds with autostart on:
  make sure the task exists and points at the current exe (re-create it when not), then remove the old HKCU
  `Run\CaseControl` and `StartupApproved\Run\CaseControl` values. When the old entry was switched off in Task Manager,
  migrate as autostart off. Remove `tauri-plugin-autostart` if nothing else needs it.
- Early apply: `AppState::init` loads the active profile (with the existing migration rules) before the engine starts
  and passes its visible settings and start brightness into `EngineConfig` (brief 17 section 2); `start_profiles` must
  not apply twice in a way that flickers.
- Uninstall: an NSIS `installerHooks` file with `NSIS_HOOK_PREUNINSTALL`, guarded by `$UpdateMode <> 1`, that runs the
  installed exe with a fixed flag `--uninstall-cleanup` and waits. That mode (handled in `main` before Tauri, like the
  elevated helper) asks a running instance to quit through the existing `--quit` path, waits for it, deletes the logon
  task, restores devices that have a save backup (brief 17 restore), and, only if the start-up task exists, removes it
  through the elevated helper (one UAC prompt; declining leaves it and is logged). Best effort, never blocks the
  uninstall for long.

## 3. Apply colours before sign-in (C)

- Setting `apply_before_sign_in` (default off), switched by its own command (not inside `update_settings`, which holds
  the settings mutex during a UAC prompt): on = elevated `register_boot_task`, off = `remove_boot_task`, both through
  `run_admin_batch`; the setting changes only when the helper reports success; UAC declined or failure leaves it as it
  was and shows the error in Settings. Debug builds: refuse with a clear message (no task for a debug exe).
- `--apply-at-boot` mode, caught in `main` before `run()` (no Tauri, webview, tray, single-instance plugin, updater,
  autosave or conflict check): own log file `boot-apply-<date>.log` in the logs folder; resolve the user's config and
  data folders robustly (the profile may not be loaded under S4U: fall back to `ProfileList\<own SID>`; log which path
  was used); read settings and the active profile read-only (never create or write settings or profiles); exit at once
  when the cross-session lock shows the signed-in instance is running; otherwise hold the lock, start the engine with
  hotplug on, apply the visible active profile and brightness, wait (bounded, about 60 s) until the profile's devices
  are present and driven, stop with `ShutdownMode::Keep`, release the lock and exit. Never saves to devices. A device
  that cannot be opened in session 0 (NvAPI likely) is logged and skipped.
- The signed-in instance takes the cross-session lock before its engine starts, waiting at most a few seconds for a
  running start-up instance, and holds it while its engine runs.

## 4. Save colours to the devices (D)

- Setting `save_to_devices` (default off), own command: on = persist all capable devices now (brief 17 API), off =
  restore all backups (pending restores for absent devices are kept and run when the device connects). Triggers while
  on: enabling, tray Quit, `--quit`, Update now (before the installer), sign-out and shutdown/restart. Never on live
  edits, never in `--apply-at-boot`. Per-device results are logged; skipped devices (no capability, original state
  unknown, daily cap) are shown in Settings as one short line under the switch.

## 5. Settings page (owner decision, copy is fixed)

- Group "Startup": "Start with Windows", "Start minimized", "Apply colours before sign-in" with hint "Needs
  administrator rights to turn on or off."
- New group "Hardware" right after "Startup": "Keep colours when signing out" (on by default) and "Save colours to the
  devices" with hint "Devices that can store lighting show it from power-on. Writes their memory at most once per
  session." No confirmation dialog.
- Use the existing setting-row and switch components, design-system tokens, the existing error display; switches with a
  UAC prompt or device writes show their busy state and stay usable afterwards. Regenerate permissions for new commands
  (`build.rs`, capabilities) like the existing ones; add `allow-quit` only if needed.

## Docs, tests, checks

Update `docs/architecture.md` (session end, autostart, before-sign-in mode, save to devices, uninstall cleanup),
`README.md` if it mentions autostart, and add a hardware test checklist for the owner to `docs/testing-hardware.md`
(shutdown, restart, sign-out with the setting on/off, power-on with Fast Startup off, logon start time from the logs,
before-sign-in on/off with UAC, save to devices on/off with restore, uninstall cleanup). Unit tests for the exit
decision table, migration rules, settings patch and the boot-mode path resolution. Run `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build` (in `apps/desktop`). One final
report: files changed, behaviour, check results, every decision this brief did not cover, and anything only a hardware
test can confirm.
