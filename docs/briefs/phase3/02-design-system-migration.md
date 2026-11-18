# Brief: adopt the design system in the desktop frontend

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md`, `docs/design-system/tokens.css` and ADR 0001
#15, #20, #23, #26-#30. You own `apps/desktop/src/**` only (not `src-tauri`; another worker is changing the Rust side
and will later add the startup-conflict UI on top of your primitives). No new npm dependencies, no UI library.

## Goal

Carry out the migration list in `docs/design-system.md` completely, steps 1 to 7:

1. Fix the spacing names that change meaning, import `tokens.css` (copy it to `apps/desktop/src/tokens.css` or import
   it from its doc location, whichever keeps one source of truth; say which).
2. Base element styles on the new tokens.
3. Display primitives: Notice (info/warning/error), StatusBadge, Card/Tile, EmptyState, PageHeader, KeyValueList, ListRow.
4. Input primitives: Button (variants incl. danger), Field, Switch, Slider, NumberField (clamps), SegmentedControl,
   TextField, Select.
5. Overlays: Dialog shell with focus trap and Escape, ConfirmDialog on top of it (danger confirm uses the danger
   variant, Cancel has initial focus), ContextMenu and IconButton/WindowControls moved onto the primitives.
6. Layout tokens (760 px text pages, fluid to 1440 px device pages) and LED preview sizes; no motion on LED colours.
7. Remove the legacy aliases once nothing uses them.

Primitives live in `components/ui/` with an `index.ts`, pages import from there. Settings use switches. Every page
(Devices, Device, Profiles, Conflicts, Settings) and the shell use the primitives; no page keeps its own copy of a
status pill, card, empty state or notice. Behaviour must not change except where the design system says so
(clamping number fields, dialog focus, switches). Keep all existing UI strings unless the design system renames one;
list every changed string.

## Checks

`pnpm typecheck`, `pnpm build`. Do not launch the app or run `tauri dev/build`. Report per migration step what you did,
deviations from the design doc (and update the doc where the code is the better answer, saying so), and changed strings.
