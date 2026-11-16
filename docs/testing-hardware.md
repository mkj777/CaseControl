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
