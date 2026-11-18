# Brief: update buttons, takeover toggle, toolbar order, per-LED white brightness (ADR 0001 #41)

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #33, #36, #37, #39, #40, #41, `docs/design-system.md`,
`docs/architecture.md` (Updates, Profiles), then `apps/desktop/src-tauri/src` (`updater.rs`, `commands.rs`, `state.rs`,
`lib.rs`, `build.rs`, capabilities), `apps/desktop/src` (`ipc.ts`, `pages/SettingsPage.vue`, `pages/LightingPage.vue`,
`pages/DevicePage.vue`, `components`), `crates/cc-core` (Zone, `brightness_only`, `Rgb::intensity`),
`crates/cc-drivers/src/nvidia_illumination`, `crates/cc-engine`. No new dependencies. Do not launch, tauri-build or
kill the app; nothing that writes to HID or NvAPI; never read files under `%USERPROFILE%\.tauri`. No commit.

## A. Update buttons in Settings

- A small "Updates" group in Settings: current version, a status line, "Check for updates", "Update now".
- Check for updates: runs a check now (also in release builds only; in debug builds the button is disabled with a
  tooltip "Updates are only available in installed builds"), downloads a newer signed update like the background
  check does (reuse the updater module / `PendingStore`, no second code path), and shows the result: "Up to date",
  "Downloading version X…", "Version X is ready", or a short error. A check already running (background or button) is
  not started twice.
- Update now: enabled only when a downloaded update is pending. Releases devices through the normal shutdown path,
  installs the pending update (quiet NSIS as on quit) and restarts the app afterwards (Tauri updater/process restart
  as appropriate). Log every step.
- Status changes reach the page via an event (e.g. `update-status`) plus a command to read the current status on
  mount. Unit tests for the status transitions behind the existing `UpdateBackend` trait.

## B. One takeover button

- Take over all and Release all become one button in the Lighting toolbar. Label "Take over all" while any visible
  device can still be taken over (the existing "has something to do" rule), otherwise "Release all". Each action keeps
  its current behaviour, busy text and toast. Keep the button width stable when the label flips.

## C. Lighting toolbar order (order only, no other changes)

- Left: profile dropdown, the takeover button, Show hidden, Rescan.
- Right: All brightness, then the fallback colour circle with its "?" tooltip.
- Responsive behaviour as today (wrapping at narrow widths must stay usable).

## D. Per-LED brightness for brightness-only devices (RTX 5080 FE: 3 white LEDs)

- On the device page of a device whose zones are all `brightness_only` (or for each such zone of a mixed device), show
  one brightness slider (0-100 %) per LED (per zone if a zone has one LED), labelled with the zone/LED name, instead
  of LED selection plus colour picker. The device brightness slider at the top still scales all of them.
- Store the level as a per-LED setting (a grey/white value per LED in a `DeviceSetting::Direct`, consistent with how
  `reduce_fixed_colors` / `Rgb::intensity` map colour to intensity for these zones), so it autosaves into the active
  profile and survives restarts. Check the NVIDIA driver really writes a separate brightness per zone from that
  buffer; if it cannot, say so in the report instead of faking it.
- Tests for the mapping (slider value -> stored value -> per-zone brightness written by the driver, through the mock
  `IllumApi`).

## Docs and checks

Update `docs/architecture.md` (Updates, Lighting toolbar, brightness-only devices). Run `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck` and `pnpm build` (in `apps/desktop`).
One final report: files changed, behaviour, check results, and every decision the brief did not cover.
