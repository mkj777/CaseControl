# Brief: Conflicts page redesign (owner 2026-11-17)

Owner: "Conflicts-Liste ist komplett schlechte UI/UX. Analysieren und minimalistischer und übersichtlicher machen,
vor allem für dynamisches und responsives UI-Design."

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md` (owner rules: no "·" separators, no filler or
restating helper text, one coherent layout, neutral greys, red/amber only for danger/warning), ADR 0001 #12, #31, #32,
#36, #37, then `apps/desktop/src/pages/ConflictsPage.vue`, `apps/desktop/src/components/ui/*`, the conflict types in
`apps/desktop/src/ipc.ts` and the backend commands in `apps/desktop/src-tauri/src/commands.rs` (read only). Follow the
`uncodixfy` skill if available.

You own `apps/desktop/src/pages/ConflictsPage.vue`, `apps/desktop/src/components/ui/ListRow.vue` (Profiles uses it and
must look and behave exactly as now) and new primitives you add under `components/ui/`, plus the Conflicts / list
sections of `docs/design-system.md`. Other workers edit other files at the same time; do not touch theirs. No new
dependencies, no backend changes. Do not launch, tauri-build or kill the app.

## Do

1. **Analyse first** (in the report): what makes the current page hard to read at 860 px, ~1100 px and wide windows
   (density, hierarchy, repeated labels, button clutter, wrapping, how state and actions relate).
2. **Redesign and build** a minimal, scannable page that adapts fluidly to the width (container queries or a grid
   that collapses columns; no horizontal scroll; nothing overlaps or wraps awkwardly at 860 px). Keep all behaviour:
   severity groups (or a better grouping if clearer), status, affected devices, close process/service with the
   confirm dialog, turn off / re-enable autostart, admin-prompt note, don't ask again for suites, evidence and
   autostart details (selectable, ADR #32), scan button, notices, empty state. Prefer fewer visible buttons per row
   (e.g. one primary action, the rest in an overflow menu using the existing ContextMenu/menu primitives) if it reads
   better. Keep wording unless a string is filler; list every string added, changed or removed.
3. Update the design-system doc with the pattern you settle on.

## Checks

`pnpm typecheck`, `pnpm build`. If a check fails only in files you do not own, re-run once later and otherwise report
it. No commit. One final report: analysis, the design decisions you took (so the owner can review them), strings,
check results.
