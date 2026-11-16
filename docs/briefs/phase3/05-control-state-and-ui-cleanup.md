# Brief: honest control state, every control takes over, UI cleanup (owner test 2026-11-16)

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md`, `docs/architecture.md`, ADR 0001 #18-#34,
then `crates/cc-engine`, `crates/cc-drivers`, `apps/desktop`. You own all of them. No new dependencies. Do not launch,
tauri-build or kill the app; do not run driver examples or anything writing to HID (the owner's app is running).

## Owner findings

1. After start without a profile every device showed "Active", but CaseControl did not drive any of them (they kept
   their hardware lighting). Brightness, All off etc. did nothing; only a colour change took control.
2. Brightness, master brightness and All off never take a released device back; only a colour change does.
3. The status chip is inaccurate.
4. The device view does not show that All off is on, or that the device is released.
5. "CaseControl" at the top of the sidebar still has too little top margin.
6. No "·" dot separators anywhere. Less filler UI: no redundant explanatory text or decorative artefacts; one
   coherent, connected layout instead of scattered controls.

## A. Control model (engine + drivers)

- Status must be the truth: `Active` only while CaseControl drives the device (a setting is being output);
  `Released` while the device shows its own lighting, including "never taken since start"; `Faulted` as before.
  Emit `DeviceStateChanged` whenever this changes (taken, released, all off/on), and make the desktop forward it so
  the UI updates without a manual refresh.
- Hardware readback: where a driver can read the device's current lighting, expose it (e.g. a
  `Controller::current_mode() -> Option<ModeState>` or the initial readback the drivers already capture: MSI config
  packet per zone, Logitech GetEffect, NVIDIA GetControl). `DeviceView` gets `hardware: Option<ModeState>` (what the
  device shows on its own) for the UI.
- Every lighting control takes control of a released/undriven, non-hidden device: colours, mode, device brightness,
  master brightness, All off. If the device has no setting yet, adopt its hardware readback as the setting (a `Mode`)
  and output it with brightness applied, so nothing visibly jumps; without a readback: All off takes control with
  black, and All on hands the device back to its own lighting (release) if it had no setting before; brightness on
  such a device has nothing to scale, so the UI disables the slider with a short reason.
- Release stays an explicit hand-back; after it any control above takes over again.
- Tests with fakes for each rule (status truth at start, takeover by brightness/all off, adopt readback, all on
  restoring hardware for a device without setting, events emitted).

## B. UI

- Status chip from the real status, live via events. Released tile/preview shows what the hardware shows: the
  readback mode name (e.g. "Rainbow wave") instead of fake white/black LEDs; unknown readback shows nothing fake.
- Device view: All off is visible (preview dark plus a clear "All off" state with a way to turn lights on);
  Released is visible (hardware mode shown, controls still usable and taking over).
- Sidebar: give the brand proper top margin so it does not sit cramped under the window edge (it need not align
  with the content title strip any more); check at 100 % and 150 % scaling mentally via the tokens.
- Remove every "·" separator (e.g. "Motherboard · MSI"); show secondary facts in a cleaner way or drop them.
- Remove filler: page subtitles/hints that restate the obvious (e.g. "Choose a device to set its colours and
  effects."), duplicate labels, decorative bits. Overview: one coherent toolbar row for the lighting controls
  (Brightness, All off, Release all) and view controls (Show hidden, Rescan), visually grouped, not spread over
  several rows. Apply the same discipline to the device view, Profiles, Conflicts and Settings. If the
  `uncodixfy` skill is available to you, follow it.
- Update `docs/design-system.md` with the rules this adds (no dot separators, no restating helper text, toolbar
  pattern, released/all-off states). List every UI string you add, change or remove.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build`. No commit. Send exactly
one final report.
