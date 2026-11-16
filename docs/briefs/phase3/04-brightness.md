# Brief: device brightness, master brightness, All off with fade, Release all (ADR 0001 #33)

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md`, ADR 0001 #19-#33, then `crates/cc-engine`,
`crates/cc-config`, `apps/desktop`. You own `crates/cc-engine/**`, `crates/cc-config/**`, `apps/desktop/**`. No new
dependencies. Do not launch the app or `tauri dev/build` (the orchestrator tests with the owner's hardware).

## Model

- Output colour = stored colour x device brightness x master brightness (x fade factor). The stored colours (engine
  buffer, `DeviceSetting::Direct`, `current_settings()`) stay unscaled, so turning brightness back up restores them
  exactly. Do the scaling in the engine at frame output, not in the UI.
- Device brightness 0-100 % per device. For a device in a hardware mode whose caps have brightness, the slider maps to
  the mode's brightness range instead (scaled by master too); without mode brightness the slider is disabled in that
  mode with a short hint.
- Master brightness 0-100 % applies to all devices on top.
- All off: fades every active, non-hidden device to black, then holds black; All on fades back to the current
  state. Direct mode: engine-side fade over a fixed duration (500 ms, ~frame-rate steps, latest-wins, cancellable by
  a new toggle). Hardware mode: if the mode has brightness, step it down to the minimum during the fade, then switch
  to the device's "Off" mode if it has one, else to direct black; on, restore the previous mode state, stepping its
  brightness up. Changes made while off are stored but not shown until on.
- Release all: existing `release_all`, also as a button in the overview.

## Persistence

- `Profile` gains `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")] brightness: BTreeMap<DeviceId, u8>`
  (missing = 100). Save profile stores current device brightness; apply restores it. `DeviceSetting` keeps its shape.
- `Settings` gains `master_brightness: u8` (default 100, clamped) and `all_off: bool` (default false), written only
  when not default.
- Start: if `all_off` is set, devices go straight to black (no flash of the last profile's colours, no fade at
  start); the profile is still applied underneath so All on fades to it.
- Device brightness survives a restart through `last_profile` plus a per-device value in settings for devices
  changed since (decide the simplest consistent rule and document it in the doc comment and your report).

## UI (design-system primitives only)

- Device view header: brightness Slider with percent value, next to / below the title, above the selection row.
- Devices overview header: master brightness Slider, All off / on Switch (label "All off" or similar), Release all
  button (secondary). Disabled states while the engine is unavailable.
- Sidebar: the "CaseControl" title strip has too little spacing (owner feedback); give it proper padding consistent
  with the design-system spacing scale and align it with the content title strip.

## Tests

Engine: scaling math, brightness never mutates stored colours, fade reaches black and back, cancel mid-fade,
hardware-mode brightness mapping and Off fallback, all_off at start produces black without a coloured frame.
Config: round trips, old files unchanged. Desktop: commands and settings persistence.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build`. Report changes, check
results, UI strings added, open points. Do not commit.
