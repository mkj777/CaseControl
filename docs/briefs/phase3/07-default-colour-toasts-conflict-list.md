# Brief: default colour for takeover, scan state, release toasts, conflicts list, sidebar (ADR 0001 #36)

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md`, ADR 0001 #31-#36, then `crates/cc-engine`,
`crates/cc-config`, `apps/desktop`. You own them plus `docs/architecture.md` and `docs/design-system.md` (not the ADR
file). No new dependencies. Do not launch, tauri-build or kill the app; nothing that writes to HID. Follow the
`uncodixfy` skill if available.

## Owner findings (test 2026-11-17)

1. Connect all only takes devices that have a stored setting; the others stay Released and nothing changes.
   Cause: `Slot::take_over` (`crates/cc-engine/src/engine.rs`) falls back only to the hardware readback, and Wooting
   has none, Logitech/NVIDIA often none that validates.
2. Rescan should make the page inactive while it scans.
3. Conflicts should be a list, not a row of cards, so it reads better.
4. The "CaseControl" brand in the sidebar now has a bit too much top space.
5. Hiding a device should show a notice "Device released" that disappears after 5 s; Release all likewise, e.g.
   "5 devices released".

## Owner decisions

- Takeover fallback order: stored setting, else hardware readback, else a **configurable default colour** (Settings,
  default white `#FFFFFF`) as a static direct colour on every LED, scaled by brightness as usual. This applies to
  every takeover (Connect all, brightness, master brightness), so every non-faulted, non-hidden device can be taken;
  remove the "brightness slider disabled, nothing to scale" case and its reason text. A device without a direct mode
  and without setting/readback: use its first mode that accepts a colour with the default colour if there is one,
  else it stays released (say which devices that hits, if any). Persist as `Settings::default_color` (written only
  when not white). Settings page: one colour field for it using existing primitives/ColorPicker; changing it does not
  repaint devices already driven.
- Release toast: bottom right of the window, over the content, auto-dismiss after 5 s (also dismissable), one at a
  time (a new one replaces the old). Texts: hide a device → "Device released" (only if it was driven); Release all
  → "1 device released" / "N devices released" with the count of devices actually released (none → no toast; the
  button is disabled then anyway). Build it as a design-system primitive (`Toast`) with the existing tokens.
- Conflicts page: a list instead of cards, grouped by the existing severity groups. One compact row per finding:
  name, affected devices, status, actions (close / Turn off autostart / Re-enable), and a chevron that expands
  details below the row: evidence, autostart entries with commands, the admin-prompt note, "Don't ask again" for
  suites. Rows separated by dividers (existing `ListRow` if it fits). Keep all behaviour and the confirm dialog.
- Rescan: while scanning, the button shows "Scanning…" and is disabled, and the page content (toolbar controls and
  tiles) is inactive: dimmed via an existing disabled/muted token, not clickable, `aria-busy` on the region.
- Sidebar brand: reduce the top padding from `--space-8` to `--space-4` (`App.vue` `.brand`); keep the rest.

## Docs and tests

- Engine tests: takeover with default colour for a device with neither setting nor readback (Connect all and
  brightness), readback still preferred over default, setting preferred over both, release count returned.
- `release_all` (engine + command) returns the number of devices released, for the toast.
- Config: round trip of `default_color`, old files unchanged.
- Update `docs/design-system.md` (Toast pattern, list pattern for Conflicts, scan busy state) and
  `docs/architecture.md` (takeover order, command shapes). List every UI string you add, change or remove.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build`. No commit. One final
report.
