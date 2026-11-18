# Brief: hide devices/zones, fluid layout, own window frame (after first hardware test)

Read `docs/briefs/phase2/00-common-rules.md` and `docs/briefs/phase2/07-desktop.md` first; their rules still apply
(no new npm or cargo dependencies, English UI, neutral greys only, no `window.confirm`/`alert`). Owner decisions:
ADR 0001 #26 to #29 in `docs/decisions/0001-project-decisions.md`. You own `apps/desktop/**` and may extend
`crates/cc-config` (`DeviceOverrides`) as described below. Do not launch or `tauri build` the app: the owner has the
debug exe running and the orchestrator tests with hardware.

## Background

On the owner's board (MSI Mystic Light) only JRAINBOW1 has LEDs; JRGB1, JRGB2 and JRAINBOW2 are empty headers that
are detected anyway. The owner wants to hide such things, wants the layout to use the window better, and wants no
Windows title bar.

## 1. Hidden devices and zones (ADR #26, #27)

- `cc_config::DeviceOverrides` gains `hidden: bool` and `hidden_zones: BTreeSet<String>` (by zone name), both
  `#[serde(default)]`, skipped when empty/false so existing files stay unchanged. Round-trip and old-file tests.
- Hidden device = not controlled: hiding releases it (`engine.release`); every write command (`set_direct`,
  `set_zone`, `set_mode`, `resize_zone`, `save_to_hardware`) refuses a hidden device with a clear error; profile
  apply (command, tray, startup `last_profile`) skips hidden devices. Unhiding changes nothing on the hardware.
- Hidden zone = gets black in direct colour: the desktop Rust side masks hidden zone slices to black before every
  `set_direct`, `set_zone` (refuse a hidden zone index) and before applying a profile's `Direct` setting. Hiding a zone
  of a device whose current setting is `Direct` re-sends the masked buffer right away. Hardware modes are unchanged
  (they act on the whole board). Put the masking in a plain function with unit tests.
- Commands `set_device_hidden(id, hidden)` and `set_zone_hidden(id, zone_name, hidden)`, persisted into settings via
  the existing save path (respect `settings_error`). Add their permissions to the capability. Expose the hidden state
  to the frontend (e.g. in `get_settings` or a field next to the device view), mirror it in `ipc.ts`.
- UI: right-click a device tile (overview) or a zone (zone name or any of its LEDs, device view) opens a small in-app
  context menu: "Hide device" / "Hide zone", or "Unhide ..." for hidden items. Suppress the browser's default context
  menu app-wide except in text inputs. A "Show hidden" toggle in the overview and in the device view (session state,
  off by default) shows hidden items greyed out with a "Hidden" label; they are not selectable and their controls are
  disabled. Without the toggle hidden items are not drawn. "All" selects visible zones only; a colour with no
  selection goes to visible zones only.

## 2. Layout (ADR #28)

- Overview: tiles fill the window width (e.g. `repeat(auto-fill, minmax(…, 1fr))`), the tile preview grows with the
  tile.
- Device view: the preview scales with the available width instead of fixed LED/key sizes (linear zones as LED rows
  whose dots grow with the width up to a sensible cap, the key matrix scales by width, single zones as a larger
  swatch). Long linear zones may wrap but stay readable.
- The selection controls (All, None, selection count) sit above the preview card, not below or inside it.

## 3. Window frame (ADR #29)

- `decorations: false` on the main window; resizing from the edges and the Windows shadow must still work.
- "CaseControl" at the top of the sidebar; minimise, maximise/restore and close at the top right over the content.
  Close calls the existing hide-to-tray path. The top strip (sidebar header and content top) is a drag region
  (`data-tauri-drag-region`); double-click toggles maximise. Buttons are plain line glyphs in the neutral palette,
  hover in grey (no red, ADR #23). Add the needed `core:window:*` permissions to the capability, nothing broader.

## Checks

`pnpm typecheck`, `pnpm build`, `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo nextest run --workspace` (or `cargo test`), `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`.
Report what you changed per section, anything you could not do, and every UI copy string you introduced.
