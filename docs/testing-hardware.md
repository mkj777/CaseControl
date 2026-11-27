# Hardware test checklist

Manual tests on the development machine (MS-7D75, Wooting 60HE ARM, G560, PRO Wireless, RTX 5080 FE). CI never touches
hardware. Before any write test: OpenRGB (app and `OpenRGB` service) and SignalRGB (app and `SignalRgb.Service`) are
stopped. FanControl stays running for every test.

## Per driver

Run in this order, read-only first:

| Step | Command | Expect |
|---|---|---|
| probe | `cargo run -p cc-drivers --example <family>_probe` | device found, description printed, nothing changes visibly |
| demo | `cargo run -p cc-drivers --example <family>_demo` | red, green, blue (GPU: white, 50 %, 10 % brightness) for 3 s each, then the previous lighting returns |

Families: `msi_mystic_light` (pass JRAINBOW1/2 LED counts as arguments if they differ from the stored ones),
`wooting`, `logitech`, `nvidia_illumination` (the demo aborts unless a GetControl/SetControl round trip is identical).

## App

- [ ] Start: tray icon present, window opens; `--hidden` starts in tray only.
- [ ] Devices overview lists all five devices with live previews.
- [ ] Device view: whole-device colour, zone colour, single-LED selection (click, drag, Ctrl-click) reach the hardware live.
- [ ] Hardware mode per device (e.g. Mystic Light Rainbow wave, G560 breathing) with speed/brightness.
- [ ] Resize JRAINBOW1/2, restart: size persists.
- [ ] Release device: firmware lighting returns; changing a colour afterwards takes control again.
- [ ] Profiles: save, apply, rename, delete; tray Apply profile; restart applies the last profile.
- [ ] Quit (tray): every device returns to its firmware lighting.
- [ ] Hot-plug: unplug and re-plug the Wooting and the Lightspeed receiver while running; the device disappears and
      comes back with its setting.
- [ ] Logitech with G HUB running and with `logi_lamparray_service` running / stopped.
- [ ] Conflicts page lists OpenRGB, SignalRGB, G HUB, LampArray, inpoutx64; End process / Stop service (UAC) work.
- [ ] FanControl keeps reading sensors and controlling fans the whole time (no errors in its log).

## Colours across shutdown, sign-out and start (ADR 0001 #42)

Installed release build only (debug builds create no tasks). Logs: `%LOCALAPPDATA%\CaseControl\logs`
(`casecontrol.<date>.log`, and `boot-apply-<date>.log` for the start-up task). Every exit writes one `exit decision`
line with the raw session message (`Query`/`End` with its flags), `shutting_down`, the switches and the chosen mode;
note them in the results table, they are what Windows sent.

Before: all devices driven by CaseControl with a distinctive profile (e.g. all red); "Save colours to the devices" off.

- [ ] **Shutdown** (Fast Startup off: `powercfg /h off` or Control Panel): Start > Power > Shut down. The colours stay
      while Windows shuts down (no hand-back flash). Log: `why=ShutdownOrRestart` (or `Unknown`), `mode=Keep`. After
      the next power-on and sign-in: no flash of the old look on the MSI board when CaseControl starts.
- [ ] **Shutdown with Fast Startup on**: as above; record what the log says (Fast Startup may report a sign-out).
- [ ] **Restart**: colours stay until the reset; log `ShutdownOrRestart`, `Keep`.
- [ ] **Sign-out, "Keep colours when signing out" on** (default): colours stay on the sign-in screen; log
      `why=SignOut`, `mode=Keep`.
- [ ] **Sign-out, switch off**: every device returns to its own lighting; log `SignOut`, `mode=Release`.
- [ ] **Tray Quit / `casecontrol-desktop.exe --quit`**: every device returns to its own lighting; log `NotEnding`,
      `Release`.
- [ ] **Installer over a running app** (manual install of the same or a newer version): devices released; log
      `CloseApp`, `Release`.
- [ ] **Start with Windows**: switch on; Task Scheduler (root folder) shows `CaseControl Logon <SID>` starting
      `casecontrol-desktop.exe --hidden`; `HKCU\...\Run\CaseControl` is gone. Sign out and in: the app starts hidden.
      Note the logon start time (sign-in to the `CaseControl starting` log line) and the time until the MSI board
      shows the profile (`active profile seeded into the engine` to the board's first output). Switch off: the task
      is gone.
- [ ] **Migration**: with an older version installed and its Run entry switched off in Task Manager, install this one:
      autostart is off in Settings, the Run value and `StartupApproved\Run\CaseControl` are gone, no task exists.
- [ ] **Apply colours before sign-in on**: one UAC prompt; Settings shows the switch on; Task Scheduler shows
      `CaseControl Start-up <SID>` (runs whether the user is signed in or not, `--apply-at-boot`). Decline the UAC
      prompt once: the switch stays off and the error shows in Settings.
- [ ] **Power-on before sign-in**: shut down, power on, wait on the sign-in screen: the profile's colours appear
      before sign-in. `boot-apply-<date>.log`: which folders were used (`KnownFolders` or `ProfileList`), the devices
      found, skipped devices (the GPU if NvAPI is not available in session 0), `keeping the colours`, and the `token`
      line (expected: elevation `default`, integrity `medium` or `system`; note what it says). Sign in quickly while
      it still runs: the signed-in log shows `asked the start-up instance to finish`, the boot log `asked to finish;
      stopping now`, and the signed-in instance takes the lock within a few seconds (at most 10 s; log `device lock
      taken` or `starting without it`). No device memory is written by the start-up task (no `save colours` or
      `restore saved state` lines in the boot log).
- [ ] **Apply colours before sign-in off**: one UAC prompt; the task is gone; the switch is off.
- [ ] **Start-up task deleted by hand**: with the switch on, delete `CaseControl Start-up <SID>` in the Task Scheduler
      (as administrator), restart CaseControl: the switch shows off (log `apply colours before sign-in follows the
      start-up task`).
- [ ] **Logon task disabled by hand**: with Start with Windows on, disable `CaseControl Logon <SID>` in the Task
      Scheduler, restart CaseControl: Start with Windows shows off (log `the logon task was disabled`); the task is
      not enabled again. Switching it on again works.
- [ ] **UAC with another account**: switch Apply colours before sign-in on and approve the prompt with another
      administrator's name and password: the switch stays off and Settings shows the full sentence about the other
      account.
- [ ] **Both switches at once**: switch Save colours to the devices while the UAC prompt of Apply colours before
      sign-in is open: the save switch works; only the switch whose command runs is greyed out.
- [ ] **Save colours to the devices on**: the line under the switch names the devices that were not saved (expected:
      Wooting, PRO Wireless, RTX 5080 "can't store lighting"; G560 possibly "original state unknown"). Log one result
      per device. Power off completely (unplug or PSU switch for 10 s), power on, stay in the BIOS or on the sign-in
      screen without the start-up task: the MSI board (and the G560 if saved) shows the saved colours from power-on.
- [ ] **Save at Quit / sign-out / shutdown** with the switch on: change a colour, Quit: the log shows one save (or
      `Unchanged` when nothing changed) before the release. At most 20 writes per device and day.
- [ ] **Save colours to the devices off**: the original saved state is written back (power-cycle shows the old
      hardware effect); an unplugged G560 is listed as "not connected; restored when connected" and restored when it
      is plugged in again (log).
- [ ] **Uninstall**: with Start with Windows, Apply colours before sign-in and Save colours to the devices on, uninstall
      from Settings > Apps: the running app quits, one UAC prompt for the start-up task (declining leaves the task and
      is logged), both tasks are gone afterwards, saved devices show their original saved state after a power-cycle.
      The app log shows `the uninstaller asked to quit` and `uninstall=true`, `persist=false` in the exit decision (no
      save to the devices on that quit). An update (Update now) runs none of this.
- [ ] **Uninstall with a pending update**: let an update download (Settings shows "Version x is ready") but do not
      install it, then uninstall: the running app quits without starting the update installer (no second installer
      window, the app does not come back), the clean-up runs as above.
- [ ] **Silent uninstall** (`uninstall.exe /S` from the install folder): no UAC prompt appears; the clean-up log says
      `silent uninstall: the start-up task is left`; the logon task is gone; saved devices are restored.
- [ ] **Uninstall with a switched-off family**: with Save colours to the devices on (MSI saved), switch the MSI
      detector off in Settings, uninstall: the MSI board is still restored (clean-up log lists it).
- [ ] **Upgrade with the old setup keeps the start-up task**: with Start with Windows and Apply colours before sign-in
      on, run the installer of a newer (or the same) version over the installed one and choose to uninstall first on
      its reinstall page: no clean-up runs (no UAC prompt, no `uninstall clean-up` log line), both tasks still exist
      afterwards and Settings shows both switches on; the saved device state is not restored.

## Results

| Date | Test | Result | Notes |
|---|---|---|---|
| 2026-11-16 | `nvidia_illumination_probe` | pass | 3 single-colour zones, brightness 50/60/60 |
| 2026-11-16 | `wooting_probe` | pass | 67 LEDs, ISO layout |
| 2026-11-16 | `msi_mystic_light_probe` | pass | 185-byte class, firmware AP/LD 0.6/1.5, JRAINBOW1/2 stored counts 20/20, sync per-LED protocol |
| 2026-11-16 | `logitech_probe` | pass | G560 4 zones; PRO Wireless (wpid 4079, index 1) 2 zones, GetEffect readback all zero |
| 2026-11-16 | `msi_mystic_light_demo -- 20 20` | pass | 42 LEDs; owner: all fans reacted, previous lighting (JRGB effect 25, Rainbow wave 26) returned |
| 2026-11-16 | `wooting_demo` | inconclusive | no errors; owner saw nothing, keyboard brightness is 0 in its own profile |
| 2026-11-16 | `logitech_demo` (G560) | inconclusive | no errors; owner saw nothing, speaker brightness is 0 in G HUB settings |
| 2026-11-16 | `logitech_demo` (PRO Wireless) | partial | red/green shown, then `device timed out` on blue and on release; owner: mouse reacted and returned to its previous state |
| 2026-11-16 | `nvidia_illumination_demo` | pass (software) | round trip identical, state restored identically; owner unsure whether the light changed visibly |
| 2026-11-16 | app: start, window, tray, overview | pass | debug exe (`tauri build --debug --no-bundle`), 5 devices |
| 2026-11-16 | app: Mystic Light whole device, live picker, single LEDs (click/drag/Ctrl) | pass | only JRAINBOW1 has visible LEDs (20, confirmed); JRGB1/2 and JRAINBOW2 are empty headers |
| 2026-11-16 | app: Mystic Light hardware mode (Rainbow wave, speed/brightness), Release | pass | |
| 2026-11-16 | app: start dialog Proceed (SignalRGB + OpenRGB, turn off autostart) | pass | one UAC prompt; `OpenRGB` and `SignalRgb.Service` now Manual |
| 2026-11-16 | app: hide zones JRGB1/JRGB2/JRAINBOW2 | pass | |
