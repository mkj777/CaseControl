# Brief: Lighting page, fallback colour in the toolbar, settings cleanup, sidebar (ADR 0001 #37)

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md`, ADR 0001 #14, #29, #33-#37, then
`apps/desktop`. You own `apps/desktop/**` except `src/pages/ConflictsPage.vue`, `src/pages/DevicePage.vue`,
`src/components/DevicePreview.vue` and `src/components/ui/ListRow.vue` (other workers run in parallel on those and on
`crates/cc-drivers`, `crates/cc-engine`). In `docs/design-system.md` and `docs/architecture.md` edit only the sections
your change touches. No new dependencies. Do not launch, tauri-build or kill the app; nothing that writes to HID.
Follow the `uncodixfy` skill if available. Other workers edit the tree at the same time: if a check fails only in
files you do not own, re-run once later and otherwise report it.

## Owner requests (2026-11-17)

1. **Navigation:** the sidebar item and page "Devices" become "Lighting" (route may become `/lighting`; keep
   `/devices` redirecting). The hidden h1 says "Lighting". Back links from the device view say "Lighting".
2. **Take over all:** the button "Connect all" becomes "Take over all" (also any toast/aria text that names it).
3. **Fallback colour in the Lighting toolbar:** remove the "Lighting" group from Settings. In the Lighting toolbar,
   next to Take over all, a colour circle (the current fallback colour, control height) that opens the colour picker
   in a popover; live update while dragging, persist on release (existing `set_default_color(color, persist)`).
   Accessible name "Fallback colour". Next to it a small "?" icon button; on hover and on keyboard focus it shows a
   tooltip (new design-system primitive `Tooltip`, tokens only) with the text:
   "Take over all gives CaseControl control of every device. A device with nothing else selected shows this colour."
   Code identifiers and the config key `default_color` stay as they are; only UI wording changes.
4. **Colour picker:** the "Custom colour" / "Fewer options" text toggle becomes a circle (swatch size) with an
   eyedropper icon (inline SVG, `currentColor`), placed with the swatches, accessible name "Custom colour", pressed
   state while open. It only folds the free picker out; no screen colour sampling.
5. **Settings cleanup:** remove the "Hide to tray" button (About), the whole "Conflicts" group (the OpenRGB port-check
   toggle; keep the setting's current behaviour and default, UI only), the hint under "Start with Windows" and the
   paragraph under "Device detectors".
6. **Sidebar:** the app icon (from `src-tauri/icons`, a size that renders crisp at 100 % and 150 %) to the left of
   "CaseControl" in the brand row, about the text's cap height to control height, vertically centred. The app
   version (from the existing app-info command or Tauri `getVersion`) at the bottom left of the sidebar, small and
   muted, e.g. "v0.1.0". Keep the About group in Settings as it is apart from removing Hide to tray.
7. Toolbar: the row now holds Take over all, fallback colour, ?, All brightness, Release all | Show hidden, Rescan.
   Keep it one row where it fits and wrap cleanly below that; disabled states as now (scan busy, releasing).

## Docs and checks

Update `docs/design-system.md` (Tooltip primitive, colour circle trigger, picker custom toggle, sidebar brand/version,
naming Lighting/Take over all/Fallback colour) and `docs/architecture.md` where page names appear. List every UI string
added, changed or removed. Checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo nextest run --workspace`, `pnpm typecheck`, `pnpm build`. No commit. One final report.
