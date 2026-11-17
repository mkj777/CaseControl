# CaseControl design system

**Status: decided** (ADR 0001 #30, which amends #23). The owner's choices are listed under
[Decisions](#decisions). Other fixed inputs are ADR 0001 #3, #12 to #16 and #18 to #29. Adopted in
`apps/desktop` (phase 3 brief 02, all seven [migration](#migration) steps).

Files:

- `docs/design-system.md`: this document (rules, components, migration).
- `apps/desktop/src/tokens.css`: the tokens, the one source of truth. It moved here from
  `docs/design-system/tokens.css` during adoption so it lives next to the code and Vite serves it (the dev server
  does not serve files outside the app folder); `main.ts` imports it before `styles.css`.
- `apps/desktop/src/components/ui/`: the primitives, exported from `index.ts`.

## How it is structured

The shape follows redpanda's design system, which works because it is small and lives next to the code:

1. **One token file** with role names (`--bg`, `--surface`, `--text-muted`, `--border-strong`), not colour names.
   Components never use a raw value.
2. **One place for primitives** (redpanda: `src/components/ui.tsx`; here: `apps/desktop/src/components/ui/`).
   Every page imports from there; a page that needs a button uses the Button, it does not style its own.
3. **A few global base rules** in the stylesheet: font, focus ring, text selection, scrollbars, drag region.
4. **Fixed control heights and a short list of variants**, so things line up without per-page tuning.
5. **Decisions with consequences go in an ADR** (redpanda ADR 0005 for the custom title bar; here ADR 0001 #29).

CaseControl adds a third token layer redpanda does not need (redpanda gets it from Tailwind): a raw palette
underneath the roles, so the owner can retune greys without touching role names.

## Principles

1. **The device is the colour.** The UI is neutral greys and white. Saturated colour on screen comes from LED
   previews and colour swatches (ADR #23). The one exception (ADR #30): a muted red for Faulted, errors and danger
   and a muted amber for warnings, as text and thin lines only. Nothing competes with the previews: no gradients,
   glows, glass, coloured accents, emoji or decorative icons.
2. **Few, large things with space around them** (ADR #15). Prefer one more scroll over one more column. Large
   spacing separates sections; borders are the exception.
3. **Calm native tool.** System font, Windows conventions (title bar buttons, toggle switches for settings that
   apply immediately, in-app context menus), motion only to soften a colour change.
4. **Words carry meaning.** Status and danger are readable from the text alone; any visual cue is extra.
5. **Every value comes from a token, every repeated pattern from a primitive.** If a page needs a value that is not
   a token, the token set is wrong or the page is.

## Tokens

All in `apps/desktop/src/tokens.css`. Components and pages use layer 2 and 3 only.

### Layer 1: palette

A single 12-step neutral ramp without tint (R = G = B), `--grey-1` (darkest) to `--grey-12` (white), plus the
two status hues, which are the only non-grey UI colours (ADR #30).

| Token | Value | Used by role |
|---|---|---|
| `--grey-1` | `#0b0b0b` | `--led-off` |
| `--grey-2` | `#141414` | `--bg`, `--accent-text` |
| `--grey-3` | `#1a1a1a` | `--surface` |
| `--grey-4` | `#222222` | `--surface-raised`, `--field-bg` |
| `--grey-5` | `#2a2a2a` | `--surface-hover` |
| `--grey-6` | `#2f2f2f` | `--border`, `--surface-pressed` |
| `--grey-7` | `#3d3d3d` | `--border-strong`, `--field-border`, `--led-border` |
| `--grey-8` | `#6e6e6e` | `--field-border-hover`, `--text-disabled` |
| `--grey-9` | `#8a8a8a` | `--text-faint` |
| `--grey-10` | `#a0a0a0` | `--text-muted` |
| `--grey-11` | `#e8e8e8` | `--text`, `--accent`, `--focus`, `--selection`, `--field-border-focus` |
| `--grey-12` | `#ffffff` | `--accent-hover` |
| `--red` | `#f07178` | `--danger`, `--status-faulted`, `--error` |
| `--amber` | `#d4a54a` | `--warning` |

### Layer 2: roles

| Group | Tokens | Rule |
|---|---|---|
| Surfaces | `--bg`, `--surface`, `--surface-raised`, `--surface-hover`, `--surface-pressed` | Each level is one step lighter. A thing on `--surface` hovers to `--surface-raised`; a thing on `--surface-raised` hovers to `--surface-hover`. |
| Lines | `--border`, `--border-strong` | `--border` divides (cards, list rows). `--border-strong` for hover and overlays. |
| Fields | `--field-bg`, `--field-border`, `--field-border-hover`, `--field-border-focus` | Text fields, selects, number fields: a fill one step lighter than the surface and a subtle border (ADR #30). Focus turns the border `--text` and adds the focus ring. |
| Text | `--text`, `--text-muted`, `--text-faint`, `--text-disabled` | Body and names in `--text`; descriptions, labels, hints in `--text-muted`; placeholders and tertiary metadata in `--text-faint`; `--text-disabled` only for disabled controls. |
| Accent | `--accent`, `--accent-hover`, `--accent-text` | Neutral white. Primary button, switch on, selected segment. One primary action per view. |
| Focus, selection | `--focus`, `--selection` | 2 px outline with 2 px offset. Never removed. |
| Status, danger | `--status-faulted`, `--danger`, `--error`, `--warning` | Red for Faulted, errors and danger actions; amber for warnings (a running conflict). Text, 1 px borders and the 3 px notice edge only, never fills or large areas, and never inside the LED stage. |
| LED | `--led-off`, `--led-border` | See [LED and key preview](#led-and-key-preview). |
| Overlay | `--backdrop`, `--shadow-overlay` | Dialog backdrop; the one shadow, for menus and dialogs. |

### Layer 3: scales

**Spacing** (index = multiple of 4 px): `--space-1` 4, `-2` 8, `-3` 12, `-4` 16, `-5` 20, `-6` 24, `-8` 32,
`-10` 40, `-14` 56, `-16` 64.

| Use | Token |
|---|---|
| Inside a control, icon to label | `--space-2` |
| Between related items (label and value, buttons in a row) | `--space-3` |
| Inside a group (settings rows, list item content) | `--space-4` |
| Card padding, gap between tiles, dialog padding | `--space-6` |
| Page header to content, preview stage padding | `--space-10` |
| Between sections | `--space-14` |

**Type.** Font `--font-sans` (Segoe UI Variable, system fallback), `--font-mono` for ids, hex values and firmware
strings, `--font-icon` (Segoe Fluent Icons, ships with Windows 11) for functional glyphs only.

| Token | Size | Weight | Line height | Use |
|---|---|---|---|---|
| `--text-xl` | 28 px | 600 | 1.25 | Page title (h1), one per page |
| `--text-lg` | 18 px | 600 | 1.25 | Section title (h2), tile and dialog titles; list row names (400: profiles, conflict findings) |
| `--text-md` | 15 px | 400 | 1.5 | Body, controls |
| `--text-sm` | 13 px | 400 | 1.5 | Hints, metadata, badges, zone names |

Running text stops at `--measure` (60ch). Numbers that change (values next to sliders, LED counts) use
`font-variant-numeric: tabular-nums`. Sentence case everywhere ("Save to device", not "Save To Device").

**Radius:** `--radius-sm` 6 (LED keys, small swatches), `--radius-md` 10 (controls, menus, notices), `--radius-lg`
18 (cards, tiles, preview stage, dialogs), `--radius-full` (pills, dots, switch). The compact tile preview keeps
2 px keys, since 6 px would turn its 10 px keys into dots.

**Elevation.** No shadows in the page. Depth is surface step plus border: page `--bg`, cards `--surface` with
`--border`, interactive cards hover to `--surface-raised` with `--border-strong`. Only overlays (context menu,
dialog, toast, popover, tooltip) add `--shadow-overlay`, to separate them from what they cover.

**Motion.** `--duration` 120 ms, `--ease` ease-out, for `color`, `background-color` and `border-color` only.
Nothing moves, slides, fades in or scales, except the switch thumb (position, same duration). LED colours in the
preview do **not** transition: they mirror the hardware, which is already throttled by the engine (ADR #21), and a
fade makes the preview lag behind the device. `prefers-reduced-motion` sets `--duration` to 0.

**Stacking:** `--z-titlebar` 10, `--z-menu` 20 (also the toast, popover and tooltip), `--z-dialog` 30, `--z-dialog-menu` 40. Nothing
else sets a z-index.

## Layout

The window is at least 860 x 560 (`tauri.conf.json`).

```
┌───────────────┬──────────────────────────────────────────────┐
│ ◉ CaseControl │  (drag region)                   ─   □   ✕   │  32 px, --titlebar-height
│               │                                              │
│ Lighting      │   Page title                      [Action]   │  page header (Lighting: title hidden)
│ Profiles      │   [Lighting controls]       [View actions]   │  toolbar row (optional)
│ Conflicts     │                                              │  --space-10
│ Settings      │   content                                    │
│               │                                              │
│ v0.1.0        │                                              │  version, bottom left
└───────────────┴──────────────────────────────────────────────┘
  220 px           --page-pad-x on both sides
```

- **Sidebar** 220 px, fixed. **Content** scrolls on its own; the sidebar and the window buttons stay put.
- **Content padding:** `--page-pad-top` 40 px; `--page-pad-x` 56 px, and 32 px when the window is narrower than
  1100 px. That is the only breakpoint. At the minimum width this leaves 576 px of content.
- **Page widths:** text pages (Profiles, Conflicts, Settings) stop at `--page-max-text` 760 px, left aligned. The
  device pages are fluid up to `--page-max-wide` 1440 px (ADR #28).
- **Tile grid:** `repeat(auto-fill, minmax(var(--tile-min), 1fr))`, gap `--space-6`. With `--tile-min` 260 px:
  2 columns at 860 px, 2 at 1100 px, 3 at 1440 px, 5 at 1920 px.
- **Device view:** back link, header, selection controls, then the preview stage at full content width, then the
  controls panel, then sections (LED count, Device). The stage is the largest thing on the page.
- **Vertical rhythm:** page header to content `--space-10`; section to section `--space-14`; inside a section
  `--space-4`; a section starts with an h2.
- **Forms in rows:** label column 120 px, control up to `--control-max-width` (320 px), value 48 px (the former
  ModeControls grid) is the Field layout for every labelled control. Settings rows, switch rows and stacked fields
  stop at `--row-max-width` (640 px).

## Components

Where they live: `apps/desktop/src/components/ui/`, one `.vue` file per primitive and an `index.ts` that
re-exports them, so pages write `import { Button, Notice, StatusBadge } from '../components/ui'`. Native elements
(`button`, `input`, `select`) keep a base style in `styles.css` so plain markup already looks right; a component
exists where there is structure, state or accessibility wiring to share. Composite feature components
(DevicePreview, ColorPicker, ModeControls, BrightnessSlider) stay in `components/` and are built from the
primitives.

Each entry: anatomy, variants, states, rules. "States" always includes `:focus-visible` (focus ring) and disabled
unless noted.

### App shell, sidebar and title bar

- **Anatomy:** grid of sidebar (`--sidebar-width`) and content. Sidebar: the app icon and "CaseControl" at the top
  (`--text-lg`, 600, in the drag region) with `--space-4` above it, so it does not sit under the window edge (it
  does not line up with the content's title strip), then the nav `--space-6` below. At the bottom left the app
  version ("v0.1.0", from `app_info`), `--text-sm`, `--text-muted`, inset `--space-3` like the nav item text; it is
  left out when `app_info` fails.
- **Brand icon** (ADR #37): the bundle icon from `src-tauri/icons`, `--brand-icon-size` (24 px, between the name's
  cap height and the control height), vertically centred, `--space-2` left of the name, decorative (`alt=""`). A
  `srcset` of `32x32.png`, `Square44x44Logo.png` and `128x128.png` lets the browser take 32 px at 100 % and 44 px
  at 150 %, so it stays sharp at both. Content: a 32 px top strip that is a drag region, with
  minimise, maximise/restore and close at the top right, each `--window-button-width` x `--titlebar-height`. The
  shell also renders the window's one Toast.
- **Nav item:** `--control-height`, padding `--space-3`, `--radius-md`, `--text-muted`. Hover and current:
  `--text` on `--surface-raised`. Current item also gets `aria-current="page"`. Text only, no icons.
- **Window buttons:** glyphs from `--font-icon` (`E921` minimise, `E922` maximise, `E923` restore, `E8BB` close),
  10 px, `--text-muted`. Hover `--surface-hover` with `--text`, pressed `--surface-pressed`. Close hovers grey too,
  not red (ADR #23). Close hides to tray. No focus ring offset into the window edge: use `outline-offset: -2px`.
- **Rules:** nothing else goes into the title strip. Double-click on the strip toggles maximise. The shell is
  being built by the current phase 3 work (brief `docs/briefs/phase3/01-hide-layout-frame.md`); this entry
  describes the target, and the values align with what lands.

### Page header

- **Anatomy:** optional back link (`← Lighting`, `--text-muted`, hover `--text`, `--space-6` below), h1 with an
  optional status badge next to it, at most one description line in `--text-muted` up to `--measure`, and the
  view actions (slot `actions`) at the top right.
- **Hidden title** (`titleHidden`): a top-level page whose name the sidebar already shows and whose header is only
  a toolbar (the Lighting overview) has no visible title. The h1 stays as a visually hidden heading, so the page
  keeps its accessible name, and the toolbar row is the first thing on the page (no gap above it).
- **Toolbar row:** a page with page-wide lighting controls (slot `controls`) gets one toolbar row under the title
  (`--space-6` above it, none with a hidden title): the lighting controls grouped on the left, the view actions
  grouped on the right in the same row instead of next to the title. Gap `--space-4` inside a group, `--space-6`
  between the groups. The brightness slider is the flexible part: it is `--toolbar-control-width` (200 px) when
  there is room and shrinks to `--toolbar-control-min-width` (80 px) before anything wraps; only then do the view
  actions move to a second row (left aligned). The Lighting page (ADR #37): Take over all, the fallback colour
  button, the help "?" (IconButton in a Tooltip), All brightness, Release all | Show hidden, Rescan. Its lighting
  group measures about 585 px at its narrowest and the view group about 255 px, so the whole toolbar is one row
  from about 1200 px window width and the view actions take a second row below that (the default 1100 px
  included). With `wrapControls` (Lighting only), below 964 px window width (content under 680 px; a media query,
  since custom properties cannot be used there) the lighting group takes the first row on its own and its
  controls wrap, so at the 860 px minimum (576 px content) nothing runs past the edge. A device page: Brightness |
  Show hidden.
- **Variants:** list page (title, optional description, actions), detail page (back link, title, status badge).
  There is no metadata line under the title; facts such as kind and vendor go into the page's key-value list.
- **Rules:** at most two actions, secondary style (Rescan, Scan, the compact Show hidden switch). The primary
  action of a page is in its content, not in the header. `--space-10` below. The toolbar's lighting group holds
  only controls that act at once (BrightnessSlider, compact Switch, a secondary Button). A description is there only
  when the title alone does not say what the page is for (Conflicts); never one that restates the title or the
  controls ("Choose a device to set its colours", "Changes are saved right away").

### Card and tile

- **Card** (static container): `--surface`, 1 px `--border`, `--radius-lg`, padding `--space-6`, content gap
  `--space-4`. Used for the preview stage (padding `--space-10`) and the dialog body. Conflict findings are list
  rows since ADR #36.
- **Tile** (a card that is a link): same look, `min-height` 220 px, whole tile is the link target. Head (name
  `--text-lg` 600, status badge right, optionally one `--text-sm` muted line about what the device shows: the name
  of its own mode while released, the hardware mode it is driven with), compact preview pinned to the
  bottom. No kind or vendor line: the name and the preview identify the device.
- **States:** hover `--surface-raised` + `--border-strong`; focus ring; **hidden** (ADR #27, only when "Show
  hidden" is on): content at 45 % opacity, a "Hidden" badge, not a link, context menu still works.
- **Rules:** cards are not nested. Stacked cards are `--space-4` apart, tiles in a grid `--space-6`. Right-click or
  Shift+F10 on a tile opens its context menu.

### Button

| Variant | Look | Use |
|---|---|---|
| `secondary` (default) | `--surface-raised`, 1 px `--border`, `--text` | Almost everything |
| `primary` | `--accent` fill, `--accent-text`; hover `--accent-hover` | The one main action of a view: "Save current as…", the empty-state Rescan |
| `ghost` | no fill or border, `--text-muted`; hover `--text` | Low-weight actions in rows: All, None, Rename |
| `danger` | like `secondary` (`--surface-raised`, 1 px `--border`), text in `--danger`; hover border `--danger` | End process, Stop service, Save to device, Delete, and the confirm button of their dialogs |

- **Sizes:** `md` `--control-height` (36 px), padding 0 `--space-4`; `sm` `--control-height-sm` (28 px), padding
  0 `--space-3`, for list rows and dense control groups.
- **States:** hover (one surface step up), pressed `--surface-pressed`, focus ring, disabled (`--text-disabled`,
  no hover, cursor default), busy (the label says what is happening, "Scanning…", and the button is disabled; no
  spinner).
- **Rules:** labels are verbs that name the result ("Release", "Save to device", "End process OneApp.exe"). One
  `primary` per view. Danger actions always open a ConfirmDialog first; its confirm button is `danger` too, never
  `primary`, and Cancel has the initial focus.

### Icon button

- **Anatomy:** `--icon-button-size` square, `--radius-md`, one `--font-icon` glyph at 12 to 16 px,
  `--text-muted`.
- **Props:** `label` is required and becomes `aria-label` and `title` (the native tooltip; no tooltip library).
  `noTitle` drops the native `title` when a Tooltip wraps the button, so the two do not show at once.
- **States:** hover `--surface-hover` + `--text`, pressed, focus ring, disabled.
- **Rules:** only for actions every Windows user reads from the glyph alone: window buttons, close in a dialog,
  back, help ("?", glyph `E897`, label "Help", inside a Tooltip). Anything else gets a text button.

### Tooltip

- **Anatomy** (`components/ui/Tooltip.vue`, ADR #37): a box below the trigger, `--space-2` gap, left aligned to it:
  `--text-sm` `--text` on `--surface-raised`, 1 px `--border-strong`, `--radius-md`, `--shadow-overlay`, padding
  `--space-2` `--space-3`, up to `--tooltip-max-width` (280 px), `--z-menu`. Text only.
- **In code:** prop `text`; the default slot is the trigger and gets `describedBy`, the tooltip's id, for its
  `aria-describedby`. The tooltip has `role="tooltip"` and stays in the DOM while hidden (`visibility`), so screen
  readers read it with the trigger.
- **Behaviour:** shows on hover and on keyboard focus (`:focus-visible`, not after a click), stays while the
  pointer is on the trigger or the tooltip (the gap belongs to the tooltip), Escape hides it (WCAG 1.4.13). No
  delay and no animation.
- **Rules:** for a short explanation that does not fit a label (the Lighting help "?"). Never the only place of
  information a control needs to work, never interactive content. An IconButton trigger sets `noTitle`.

### Colour button

- **Anatomy** (`components/ColorButton.vue`, ADR #37): a circle `--control-height` across, filled with the current
  colour, 1 px `--led-border` (hover `--text-muted`), like a swatch. While its popover is open it has the selection
  outline (2 px `--selection`, 2 px offset). Below it a popover with a ColorPicker: `--surface`, 1 px
  `--border-strong`, `--radius-md`, `--shadow-overlay`, padding `--space-4`, width `--popover-width` (320 px: the
  swatches in rows of six and five), `--space-2` below the circle, `--z-menu`.
- **In code:** props `modelValue`, `label` (the accessible name of the circle and the popover), `disabled`; emits
  the picker's `update:modelValue` (live while dragging) and `change` (final value of a gesture).
- **Behaviour:** the circle toggles the popover (`aria-haspopup="dialog"`, `aria-expanded`). On open, focus moves
  to the current swatch (else the first). The popover is non-modal: Escape closes it and returns focus to the
  circle; a click outside or moving focus out closes it. Disabled: 50 % opacity, the popover closes.
- **Use:** the fallback colour in the Lighting toolbar ("Fallback colour", see Device states).

### Colour picker

- **Anatomy** (`components/ColorPicker.vue`, ADR #20): a row of swatch circles (`--control-height`), then the free
  picker folded out below it: saturation/value area, hue bar, hex field.
- **Custom toggle** (ADR #37): the last circle of the swatch row, same size and edge, `--surface-raised` with an
  eyedropper icon (inline SVG, 16 px, `currentColor`, `--text-muted`; hover `--surface-hover` + `--text`).
  Accessible name "Custom colour", `aria-pressed` while the free picker is open; pressed look `--surface-pressed`,
  `--text` icon, `--text-muted` edge. It only folds the free picker out and in; it does not sample the screen.

### Switch

- **Anatomy:** row with label (`--text`) and hint (`--text-sm` muted) on the left, switch on the right, row
  max 640 px. Track `--switch-width` x `--switch-height`, `--radius-full`.
- **States:** off: track `--surface-raised`, 1 px `--field-border-hover` (3.41:1, the switch keeps a visible edge),
  thumb `--text-muted`. On: track `--accent`,
  thumb `--accent-text`. Disabled: 50 % opacity on the track only, label `--text-disabled`.
- **Rules:** for settings that apply at once (all of Settings, "Show hidden"). `role="switch"`,
  `aria-checked`, Space toggles, the whole row is the click target. A checkbox stays only for "select several items
  in a list" (none yet).

### Segmented control

- **Anatomy:** a row of 2 to 4 equal buttons inside one `--field-border` outline on `--field-bg`, `--radius-md`,
  `--control-height`.
- **States:** selected segment `--surface-hover` with `--text`; others `--text-muted`, hover `--text`. Focus ring
  on the group's selected segment.
- **Rules:** for 2 to 4 short, mutually exclusive options visible at once: Direction (Left / Right, Up / Down,
  Horizontal / Vertical). Five or more options or long labels use a Select. `role="radiogroup"`, arrow keys move
  the selection.

### Slider

- **Anatomy:** label (Field), native `input type="range"`, value readout at the right in tabular figures.
- **Look:** native Chromium range with `accent-color: var(--accent)`; width fills the Field's control column
  (max 320 px).
- **States:** dragging sends live (ADR #21); disabled greys out via the native control.
- **Rules:** always show the value. Where an exact number matters (frame rate), pair it with a NumberField that
  edits the same value.

### Brightness slider

- **Anatomy:** a Field in the row layout with a Slider from 0 to 100 and the readout "N %"
  (`components/BrightnessSlider.vue`, ADR #33). Props: `label`, `value` (percent), `send(percent, persist)`,
  optional `disabled` and `hint`; emits `error`.
- **Behaviour:** while dragging it sends live with `persist = false`, coalesced so only the newest value is in
  flight (`latestSender`). When the drag ends it drops the queued live value, waits for the one in flight and sends
  the final value once more with `persist = true`, which is the one that gets saved. Until that send has finished
  it shows its own value, so a device refresh in between cannot pull the thumb back.
- **Layout:** the inline Field layout (label at its own width, slider `--toolbar-control-width` down to
  `--toolbar-control-min-width`, readout), since it always sits in a page toolbar.
- **States:** disabled (engine down, device locked, a rescan or Release all running on the Lighting page, a device
  the engine cannot take over: "Set a colour or mode first.", a hardware mode without a brightness parameter: "This
  mode has no brightness setting."). A failed send is emitted as `error`; the page shows it as an inline Notice.
- **Takes over:** on a released device it takes control like every lighting control: with its last setting, else
  its own mode as read back, else the fallback colour (ADR #36, #37), as the engine reports it
  (`DeviceView.takeover`). Only a device the engine cannot take over at all (no direct mode and no mode that takes a
  colour, nothing stored or read back) gets a disabled slider with the hint "Set a colour or mode first."
- **Rules:** every brightness control is this component (master brightness, labelled "All brightness", on the
  Lighting page; device brightness on a device page), usually in the PageHeader `controls` row. The `send` callback decides what `persist` saves;
  the slider never saves on its own.

### Number field

- **Anatomy:** native `input type="number"`, width set in `ch` for the longest value plus 4ch, tabular figures,
  then the allowed range as a `--text-sm` muted hint ("1 to 60", "0 to 120").
- **States:** focus (border `--field-border-focus` plus the focus ring), invalid (border
  `--error`, `aria-invalid`, inline error under the field),
  disabled.
- **Rules:** commits on change or Enter, not per keystroke. Out-of-range input: clamp to the range on commit and keep the
  hint visible (decision 12). Input that is not a number (empty field) goes back to the current value. The
  component renders the range hint itself.

### Text field

- **Anatomy:** `--control-height`, padding 0 `--space-3`, `--field-bg`, 1 px `--field-border`, `--radius-md`.
  Placeholder `--text-faint`. Monospace variant for hex values.
- **States:** hover border `--field-border-hover`; focus border `--field-border-focus` (12.98:1 against the fill)
  plus the 2 px focus ring, so a focused field is unmistakable even though the resting edge is subtle; invalid
  border `--error` plus the inline error; disabled (`--text-disabled`, fill `--surface`, border `--border`).
- **Rules:** every field has a visible label (Field) or, inside a one-field form such as "Profile name", an
  `aria-label` plus a placeholder that names it. Text selection is allowed in fields and in the copyable text listed
  under [Text selection](#text-selection).

### Field

- **Anatomy:** label (`--text-muted`), control, optional hint and inline error below. Three layouts: stacked
  (label above), row (label column 120 px, control column up to 320 px, value column 48 px) and inline (label at
  its own width, control `--toolbar-control-width` shrinking to `--toolbar-control-min-width` in a tight toolbar,
  value at its own width; for a page toolbar).
- **Wiring:** the default slot receives `id` (for the control, so the label's `for` works), `describedBy` (hint and
  error ids) and `invalid`. With `group`, the label names a `role="group"` instead of one control (colour chips,
  segmented control).
- **Rules:** wraps every labelled control so labels, hints and errors line up across pages.

### Select and menu

- **Select:** native `<select>` styled like a text field (same fill, border and focus) (`color-scheme: dark` gives the dark dropdown). Used for
  the mode menu and other lists of 5+ options. Disabled placeholder options ("Device default") stay as they are.
- **Rules:** do not build a custom dropdown while the native one does the job. A custom menu, if ever needed,
  reuses the context menu below.

### Context menu

- **Anatomy:** `--surface`, 1 px `--border-strong`, `--radius-md`, `--shadow-overlay`, padding `--space-1`,
  min width `--menu-min-width`. Items `--control-height-sm`, padding 0 `--space-3`, `--text-md`. Optional
  separator: 1 px `--border`, `--space-1` margin.
- **States:** item hover and keyboard focus `--surface-hover`; disabled item `--text-disabled`.
- **Behaviour:** opens at the pointer, clamped inside the window; also opens with Shift+F10 or the Menu key on a
  focused tile or zone. Arrow keys move, Enter activates, Esc or a click outside closes and returns focus to the
  element that opened it. One level, no submenus. `role="menu"`, `role="menuitem"`, `--z-menu`. Pages pass
  `menuPoint(event)` as the position: a keyboard-opened menu has no pointer, so it opens at the focused element. A
  hidden tile is not a link but stays focusable (`tabindex="0"`) so its menu (Unhide) is reachable by keyboard.
- **Rules:** the browser menu is suppressed app-wide except in text fields. Every action in a context menu is
  also reachable another way or harmless (Hide / Unhide).

### Menu button

- **Anatomy:** an IconButton with the "More" glyph (`E712`, a horizontal ellipsis) that opens the context menu
  `--space-1` under itself, right aligned to the button at `--menu-min-width` (a wider menu reaches further right,
  kept inside the window). `components/ui/MenuButton.vue`, props `label`, `items`, `disabled`;
  `aria-haspopup="menu"` and `aria-expanded` on the button.
- **Behaviour:** the context menu's keyboard and close rules; Esc returns focus to the button, a second press on the
  button closes the menu.
- **Rules:** for the less common actions of one list row, so the row shows one action button. Its items are the
  same labels the buttons would carry; a danger item still opens its ConfirmDialog. Not shown when a row has no more
  actions. Unlike a context menu, its items need not be reachable elsewhere: the button is visible and focusable.

### Dialog

- **Anatomy:** backdrop `--backdrop`; panel width `--dialog-width` (max window minus `--space-6` each side),
  `--surface`, 1 px `--border-strong`, `--radius-lg`, `--shadow-overlay`, padding `--space-6`. Title `--text-lg`;
  body `--text-muted`, paragraphs `--space-3` apart; actions right aligned, `--space-6` above, Cancel first.
- **Variants:** Dialog is the shell (title, body slot, `actions` slot, `role` `dialog` or `alertdialog`, `busy`).
  ConfirmDialog sits on top of it (`role="alertdialog"`, `danger` for danger confirms). The element marked
  `data-autofocus` gets the initial focus.
- **Behaviour:** focus moves to the safe choice (Cancel) on open; focus is trapped inside; Esc and a backdrop
  click cancel unless busy; focus returns to the opener on close.
- **Rules:** the title is a question ("Save to device?"), the confirm label repeats the action ("Save to device"),
  the body says what happens and what cannot be undone. Never `window.confirm` or `alert`.

### Status badge

One pill, `--text-sm`, padding 2 px `--space-2`, `--radius-full`, 1 px border, no fill. The label always carries
the meaning; colour is reserved for the states that need attention.

| Status | Look |
|---|---|
| Active (CaseControl drives the device, also at brightness 0) | `--text`, border `--border-strong` |
| Released (the device shows its own lighting, also before CaseControl ever drove it) | `--text-muted`, border `--border-strong` |
| Hidden | `--text-faint`, dashed border `--border-strong` |
| Faulted | `--status-faulted` text and border |
| Running (conflict) | `--warning` text and border |
| Installed, not running | `--text-muted`, border `--border-strong` |
| Active profile | `--text`, border `--text` |

In code: `<StatusBadge status="…">label</StatusBadge>` with `active`, `released`, `hidden`, `faulted`, `running`,
`not-running` (covers "Installed, not running" and "Not running") and `current` (active profile).

A device badge always shows the engine's real status, which it reports with `device-state-changed`; the page never
guesses it.

Severity group headings on the Conflicts page stay headings, not badges.

### Empty state

- **Anatomy:** h2, one or two sentences in `--text-muted` up to 56ch saying why it is empty and what to do, at most
  one action (primary). Left aligned, padding `--space-14` top and bottom. No illustration, no icon.
- **Rules:** used for "No devices detected", "No profiles yet", "Nothing found", "Device not found". Where the
  empty state is the whole page ("Device not found") it is the page's h1 (`level` 1) with the h2 look.

### Notice and inline error

- **Notice** (page level): `--surface`, 1 px `--border-strong`, `--radius-md`, padding `--space-4` `--space-5`,
  max width `--measure` + padding; first line `--text`, details `--text-sm` muted. `--space-6` below.
- **Variants:** `info` (default, `role="status"`); `warning` (`role="status"`, 3 px left edge `--warning`, for
  example "Parts of the scan failed"); `error` (`role="alert"`, 3 px left edge `--error`, for failed actions and the
  engine error). The text stays `--text` / `--text-muted`; only the edge is coloured.
- **Inline error** (field level): `--text-sm`, `--error`, under the field, linked with
  `aria-describedby`; the field gets `aria-invalid`.
- **Rules:** messages say what happened and what to do next. Success messages ("Saved to the device.") use `info`
  and disappear on the next action. Stacked notices are `--space-3` apart. Error notices and inline errors are
  selectable (see [Text selection](#text-selection)). A short confirmation that should go away on its own is a
  Toast, not a Notice.

### Toast

- **Anatomy:** one line of `--text` on `--surface-raised`, 1 px `--border-strong`, `--radius-md`,
  `--shadow-overlay`, padding `--space-2` (`--space-4` on the left), max width `--dialog-width`, and an IconButton
  "Dismiss" (glyph `E8BB`, as the window's close button) at the right. Fixed at the bottom right of the window,
  `--space-6` from both edges, over the content (`--z-menu`; dialogs cover it).
- **Behaviour:** dismisses itself after 5 s or with its button. One at a time: a new toast replaces the current one
  and starts the 5 s again. No animation (see Motion). The live region (`role="status"`, `aria-live="polite"`) is
  always in the DOM, so screen readers announce each toast.
- **In code:** `components/ui/Toast.vue` (props `toast: { id, text } | null`, `duration` = 5000; emits `dismiss`),
  rendered once by the shell; pages call `showToast(text)` from `stores/toast.ts`.
- **Rules (ADR #36):** only for releases whose result is not visible where the user acted: hiding a driven device
  ("Device released", only if it was driven) and Release all ("1 device released" / "N devices released", the
  number of devices actually released; none, no toast). Errors stay Notices; a toast never carries an action.

### List row

- **Anatomy:** `min-height` 72 px, padding `--space-4` `--space-2`, 1 px `--border` below (the list has one above);
  name (`--text-lg` 400) with optional badge on the left, actions on the right as `sm` buttons (one secondary, the
  rest ghost). Exception: a danger action (Delete) uses the `sm` danger button, not ghost. The first row draws the
  top line, so the list itself needs no border.
- **States:** default, current (badge "Active"), editing (the row turns into an inline form with the same height),
  disabled while busy, expanded (slot `details`, below the row inside its divider, `--space-4` above).
- **Actions:** stay on the right at their width. Opt-in (`stackActions`, used by Conflicts): the row is a size
  container (`container: list-row / inline-size`); when the row is narrower than 640 px the actions take their own
  line under the name and the row's text, left aligned, `--space-3` below, and wrap if they still do not fit. A text
  page is 576 px wide at the 860 px minimum window and 760 px from about 1050 px, so findings stack in narrow
  windows and keep one line from about 940 px. A page sets it only on rows that have an action besides the chevron.
- **Expandable row** (Conflicts, ADR #36): the last action is an IconButton chevron (glyph `E70D` down / `E70E` up,
  label "Show details" / "Hide details", `aria-expanded`, `aria-controls` on the details: the page passes the id
  as `detailsId`, which ListRow puts on its details wrapper). The page decides what is open; open rows stay open
  across a rescan.
- **Rules:** rows are not cards. Used for profiles, for conflict findings (grouped under the severity h2s, see
  [Conflicts list](#conflicts-list)) and for
  "Found but not started" on Lighting (no actions).

### Conflicts list

One `ul` of ListRows per severity group (High, Medium, Low, Info), under the group's h2, groups `--space-10` apart
(subsections of one list, so closer than page sections). One compact row per finding, with `stackActions`:

```
SignalRGB  [Running]                         [End process SignalRgb.exe]  [⋯]  [⌄]
Corsair K70, ASUS mainboard
```

- **Line 1:** name and status badge (Running / Installed, not running / Not running). **Line 2:** the affected
  devices in `--text-sm` muted, always on their own line ("None of your detected devices" when none).
- **Actions:** one `sm` button, the first in this order: End process / Stop service (danger, behind the confirm
  dialog, one per close target), Turn off autostart (secondary), Re-enable autostart (ghost). The other actions go
  into a Menu button ("More actions"), shown only when there are any. The chevron is last and keeps the row's right
  edge, also when the actions stack under the text at narrow widths.
- **Result:** the outcome Notice of an action ("Done.", "Some steps did not work.", "Some steps are not confirmed
  yet. Scan again in a moment.", with the result lines) shows under the row it was taken on, above its details,
  until the next action; only when that row is gone after the rescan does it show above the list.
- **Details** (chevron): the catalogue note (muted, up to `--measure`), a key-value list (Vendor, Device families,
  Found, Starts with Windows with each entry's command in `--font-mono`; evidence and commands selectable), the
  permission-prompt note when an autostart change needs one, and "Don't ask again at start" for RGB suites.

### Key-value list

- `<dl>` grid, `max-content 1fr`, gap `--space-2` `--space-6`, `dt` muted, values that are ids in `--font-mono`
  (`dd.mono`, selectable).
  Replaces `.details`, `.facts` and `.info`.

### LED and key preview

The preview is the focus of the app (ADR #15), so it gets the most room and the fewest decorations.

- **Stage:** a card with padding `--space-10`, full content width. Background `--surface`.
- **LED shapes:** linear zones are round dots, matrix zones are square keys (`--radius-sm`) on the device's grid,
  single zones are a rectangle (`--radius-md`). Sizes scale with the stage width (ADR #28) up to a cap: dot 28 px,
  key by grid, single 160 x 80 px. Compact preview on tiles: dot 8 px, key gap 2 px, single 32 x 16 px.
- **Colour:** the fill is exactly what the device shows. `#000000` is drawn as `--led-off`. Every LED has a 1 px
  `--led-border`, so off LEDs stay visible. No transition on the fill (see Motion).
- **What it shows:** while CaseControl drives the device, its colours; while released, the device's own per-LED colours if its driver can read them, else **unknown**: no fill and a
  dashed `--led-border`, so the layout shows without made-up colours. Never draw white or black for a colour that
  is not known.
- **Selection:** 2 px outline in `--selection` with 2 px offset. Hover on a selectable LED: border `--text-muted`.
- **Zone names:** `--text-sm` ghost buttons above each zone; click selects the zone, Ctrl-click adds.
- **Hidden zones** (Show hidden on): 45 % opacity, "Hidden" badge next to the zone name, not selectable.
- **Selection controls** (All, None, count) sit above the stage (ADR #28), ghost buttons.
- **Rules:** no labels, glows, reflections or device photos in the stage. Swatches and colour chips follow the same
  border and selection rules.

## Device states

- **Released** (the device shows its own lighting): badge "Released"; the tile line and the device view's mode menu
  name its own mode when it can be read ("Own lighting (Rainbow wave)", else "Own lighting"); the preview shows its
  own colours or unknown LEDs. Every control stays usable and takes the device over.
- **Off** (ADR #35): brightness 0 (a device's Brightness or All brightness) turns lights off;
  the device stays Active.
- **Take over all / Release all** (Lighting toolbar, secondary buttons; Take over all was "Connect all" before
  ADR #37): Release all hands every device back, is disabled while no device is Active, and confirms with a toast
  ("N devices released"). While it runs it reads "Releasing…" and All brightness and Take over all are disabled.
  Take over all takes every released, visible, non-faulted device over with the same rules as brightness (ADR #36):
  last setting, else its own mode as read back, else the fallback colour (white unless changed) on every LED, or,
  without a direct mode, in its first mode that takes a colour; it is disabled when every such device is already
  Active.
- **Fallback colour** (Lighting toolbar, ADR #37; the `default_color` setting, called "Default colour" before): a
  Colour button "Fallback colour" next to Take over all, followed by the help "?" whose Tooltip reads "Take over
  all gives CaseControl control of every device. A device with nothing else selected shows this colour." It only
  feeds later takeovers; changing it does not repaint devices that are already driven. Like a brightness slider it
  goes live while dragging and is saved once with the final value (ColorPicker's `change` event: end of a drag, a
  click, a key step or a hex entry). Its errors are its own Notice, so a colour change clears only those. Disabled
  while a rescan runs, until the settings are loaded, and for the session when settings cannot be saved.
- **Rescan busy** (Lighting): while a rescan runs the Rescan button reads "Scanning…" and is disabled, the toolbar
  controls (Take over all, Fallback colour, All brightness, Release all, Show hidden) are disabled (the help "?"
  stays usable), and the tile grid is dimmed to
  50 % opacity (the disabled look of Switch and ColorPicker) and `inert` (no clicks, focus or context menu). The
  page section carries `aria-busy` while it runs.

## Copy and filler

- **No dot separators.** Never join facts with "·" (or "•", "|"). Show a secondary fact on its own line, as a row of
  a key-value list, or drop it.
- **No restating helper text.** No subtitle, hint or note that repeats what the title, the label or the visible
  state already says ("Choose a device to set its colours and effects.", "Set how many LEDs are connected to each
  header.", "Open in the tray without showing this window."). Keep a hint only when it adds a fact the control does
  not show (a range, a side effect, a permission prompt, a non-obvious gesture).
- **No confirmation notices for visible state.** When the result of an action is visible (a badge changes to
  Released), no notice repeats it. The two release toasts (ADR #36) are the owner's exception: the tile of a hidden
  device disappears, and Release all changes many badges at once.
- **No decorative bits.** No duplicate labels, ornaments or filler lines; controls that belong together sit in one
  row (see the page toolbar), not scattered over several.

## Text selection

The app body is `user-select: none` (a desktop tool: dragging over a page does not paint selections). Text that
people copy into a search or a bug report is selectable (ADR 0001 #32):

- error and fault messages: Notice `error` (the whole notice), the Field inline error, fault and failure texts;
- device ids and other monospace values (`KeyValueList` `dd.mono`);
- conflict evidence: process and service names, paths, autostart entries and their commands, scan errors and
  action results.

Primitives handle their own cases. Elsewhere add the utility class `selectable` from `styles.css`
(`user-select: text`, text cursor). Fields stay selectable as before. Nothing else is.

## Accessibility

Contrast, WCAG 2.x, measured with the decided values:

| Foreground | on `--bg` | on `--surface` | on `--surface-raised` | Target |
|---|---|---|---|---|
| `--text` `#e8e8e8` | 15.04 | 14.20 | 12.98 | 4.5 text |
| `--text-muted` `#a0a0a0` | 7.04 | 6.66 | 6.08 | 4.5 text |
| `--text-faint` `#8a8a8a` | 5.34 | 5.04 | 4.61 | 4.5 text |
| `--text-disabled` `#6e6e6e` | 3.61 | 3.41 | 3.12 | exempt (disabled) |
| `--danger` / `--error` `#f07178` | 6.44 | 6.08 | 5.56 | 4.5 text |
| `--warning` `#d4a54a` | 8.14 | 7.69 | 7.03 | 4.5 text |
| `--field-border` `#3d3d3d` | 1.70 | 1.60 | 1.46 | 3.0 control boundary, **waived** (ADR #30) |
| `--field-border-hover` `#6e6e6e` (also the switch edge) | 3.61 | 3.41 | 3.12 | 3.0 control boundary |
| `--field-border-focus` / `--focus` `#e8e8e8` | 15.04 | 14.20 | 12.98 | 3.0 focus indicator |
| `--border-strong` `#3d3d3d` | 1.70 | 1.60 | 1.46 | decorative only |
| `--border` `#2f2f2f` | 1.38 | 1.30 | 1.19 | decorative only |
| `--accent-text` on `--accent` | 15.04 | | | 4.5 text |

Red and amber pass 4.5:1 even on `--surface-hover` (5.02 and 6.34), so the owner's values are used unchanged.
Fields miss the 3:1 control-edge target by the owner's choice (ADR #30): they are recognisable by their lighter
fill and fixed height, and focus is made unmistakable (white border plus the 2 px ring). Today's faint text
`#6c6f77` (3.70 on bg) is replaced by `--grey-9` for content.

- **Focus:** `:focus-visible` gives a 2 px `--focus` outline at 2 px offset on every interactive element, including
  LED keys and tiles. Never `outline: none` without a replacement.
- **Keyboard:** everything works without a mouse. Tab order follows reading order. Context menus open with
  Shift+F10 or the Menu key. Dialogs trap focus and give it back. Zones are selectable by keyboard through the zone
  name buttons and All / None. Per-LED keyboard selection does not exist yet; if it is added, arrow keys move within
  a zone and Space toggles.
- **Targets:** at least 24 x 24 px (WCAG 2.5.8); controls are 28 or 36 px.
- **Semantics:** native elements first (`button`, `select`, `input`, `dl`), `aria-current` in the nav,
  `role="switch"` / `radiogroup` / `menu` / `alertdialog` on the custom ones, status text in the DOM, not only in a
  colour or shape.
- **Motion:** `prefers-reduced-motion` turns transitions off.

## How to add a component

Mirrors how redpanda grew `ui.tsx`:

1. **Look first.** Can an existing primitive or a new variant of one do it? A variant beats a new component; a
   prop beats a variant.
2. **Put it in `components/ui/`**, named for what it is (`StatusBadge`, not `DeviceStatusPill`), exported from
   `components/ui/index.ts`. Props are the minimum the pages need; accessibility inputs are required props (an
   IconButton without `label` does not compile).
3. **Style with role tokens only.** No hex values, no pixel values except borders and the component's own fixed
   sizes, which go into `tokens.css` under "Component and layout sizes" if more than one place needs them. A new
   role token needs a row in the tables above.
4. **Cover the states:** hover, pressed, focus-visible, disabled, and busy or invalid where they apply. Check it
   at 860 x 560 and with reduced motion.
5. **Document it here** with anatomy, variants, states and rules, in the same shape as the entries above.
6. **Replace the copies.** Move every page that had its own version over to the primitive in the same change, so
   there is never a primitive and a scoped look-alike side by side.

UI copy follows the existing rules: English, sentence case, verbs on buttons, no exclamation marks.

## Migration

Current inventory and where each pattern goes. Wait until the phase 3 work on `apps/desktop` (hide / unhide,
fluid layout, title bar) is merged, then adopt in this order; each step builds and type-checks on its own.

### Inconsistencies found

- Spacing names do not match values: `--space-8` is 40 px, `--space-10` is 56 px.
- The same pill exists three times with different emphasis rules: `.status` (DevicesPage), `.state`
  (ConflictsPage), `.marker` (ProfilesPage).
- Three empty-state shapes: `.empty` with h2 and button (Devices), `p.empty.muted` (Profiles, Conflicts), `.gone`
  (DevicePage).
- Three key-value lists: `.details` (DevicePage), `.facts` (Conflicts), `.info` (Settings), each with its own gap.
- The card look (`--color-surface`, border, `--radius-lg`) is repeated in `.tile`, `.stage`, `.finding` and
  `.dialog`.
- `.notice` margins are redefined per page (`--space-8` on Devices, `--space-6` elsewhere); errors and messages
  look identical.
- `.name` is 18/600 on Devices and Conflicts, 18/400 on Profiles.
- Disabled looks differ: buttons change text colour, ColorPicker drops to 50 % opacity.
- Hard-coded values: `120px minmax(0, 320px) 48px` (ModeControls), 40 px chips, 36 px swatches, 22 px dots,
  96 x 56 swatch, 280 px inputs, `#ffffff` (primary hover), `rgba(0, 0, 0, 0.6)` (backdrop), the monospace stack
  twice, `999px`.
- Number fields: LED count rejects out-of-range input with an error, frame rate clamps silently.
- ConfirmDialog always styles the confirm as `primary`, so "End process" is the most prominent button on screen; it
  has no focus trap.
- Settings use checkboxes for settings that apply at once (Windows uses toggle switches).
- LED dots transition their colour (120 ms) although they mirror live hardware.
- Text fields and selects have no fill of their own; the decided look adds `--field-bg`.

### Mapping

| Current | Becomes |
|---|---|
| `styles.css` `:root` variables | `tokens.css` (legacy aliases bridge the old names) |
| `--space-8` (40) / `--space-10` (56) | `--space-10` / `--space-14` (rename before importing tokens.css) |
| `--color-*`, `--font-size*`, `--radius`, `--content-width` | roles and scales without the prefix, see the legacy block |
| `button`, `button.primary`, `button.link` | base styles plus Button (`secondary`, `primary`, `ghost`) |
| `.page`, `.page-header`, `.page-description` | PageHeader; `.page` takes `--page-max-text` or `--page-max-wide` |
| `.notice` (styles.css and every page) | Notice (`info`, `error`) |
| `.status`, `.state`, `.marker` | StatusBadge |
| `.tile` (Devices), `.stage` (Device), `.finding` (Conflicts) | Card; Tile for the device link |
| `.empty`, `.gone` | EmptyState |
| `.details`, `.facts`, `.info` | KeyValueList |
| `.item` (Profiles), `.failures li` (Devices) | ListRow |
| `.control` grid (ModeControls), `.setting` (Settings), `.zone` (Device) | Field |
| Settings checkboxes, "Show hidden" | Switch |
| Direction `<select>` (ModeControls) | SegmentedControl |
| range inputs (ModeControls, Settings) | Slider |
| `input type="number"` (LED count, frame rate) | NumberField |
| ConfirmDialog | Dialog shell + ConfirmDialog on top |
| context menu from phase 3 | ContextMenu in `components/ui/` |
| title bar buttons from phase 3 | IconButton |
| DevicePreview `.led`, `.dot`, `.cell`, `.swatch` | LED rules above (`--led-*`, `--selection`, no transition) |
| `.muted`, `.small`, `.row` utilities | kept as the only global utilities |

### Order

Done in phase 3 (brief 02). The inventory and mapping above describe the code before adoption.

1. **Tokens.** Rename the two shifted spacing uses (`--space-8` → `--space-10`, `--space-10` → `--space-14` in
   App.vue, styles.css and the pages), add `tokens.css` before `styles.css` in `main.ts`, delete the old `:root`
   variables. Visible change: neutral greys, field fill, faint text.
2. **Base styles.** Point `styles.css` element rules (body, headings, button, input, select, focus) at the role
   tokens; fixed control heights; `--field-*` on fields.
3. **Display primitives** (no behaviour, biggest de-duplication): Notice, StatusBadge, Card / Tile, EmptyState,
   PageHeader, KeyValueList, ListRow.
4. **Input primitives:** Button, Field, Switch (Settings), Slider, NumberField, SegmentedControl (Direction),
   TextField.
5. **Overlays:** Dialog shell with focus trap, ConfirmDialog on top of it, ContextMenu and IconButton taken over
   from the phase 3 code.
6. **Layout:** page width tokens, `--page-pad-x` breakpoint, `--tile-min`, LED preview sizes and the removed LED
   transition.
7. **Clean up:** delete the legacy block in `tokens.css` and any scoped style that duplicates a primitive.

## Decisions

Taken by the owner on 2026-11-16 and recorded as ADR 0001 #30 (amends #23).

| # | Topic | Decision |
|---|---|---|
| 1 | Grey values | Neutral greys without a tint (`--grey-1` to `--grey-12` above). |
| 2 | Text levels | Text, muted, faint, plus disabled. |
| 3 | Field borders | Subtle border plus a lighter fill; accepted below the WCAG 3:1 control-edge target; focus shows a white border and the focus ring. |
| 4 | Status and danger | Muted red `#f07178` for Faulted, errors and danger; muted amber `#d4a54a` for warnings such as a running conflict. The only non-grey UI colours; text and thin lines, never fills. |
| 5 | Danger button | Like secondary with red text; also the confirm button in danger dialogs. |
| 6 | Radius | 6 / 10 / 18. |
| 7 | Type | 13 / 15 / 18 / 28, Segoe UI Variable. |
| 8 | Control height | 36 px (`sm` 28 px). |
| 9 | Spacing names | Index = multiple of 4 px. |
| 10 | Motion | 120 ms on UI colours, no transition on LEDs. |
| 11 | Preview stage | `--surface` card. |
| 12 | Number field out of range | Clamp on commit, range hint stays visible. |
| 13 | Settings controls | Toggle switches, label left, switch right. |
| 14 | Page widths | Text pages 760 px, device pages fluid to 1440 px. |
| 15 | Component approach | Native elements styled globally, Vue components where there is structure or state. |
| 16 | Overlay shadow | One soft shadow for menus and dialogs. |
| 17 | Title bar | 32 px high, 46 px Segoe Fluent glyph buttons. |
