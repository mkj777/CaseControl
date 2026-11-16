<script setup lang="ts">
// Draws a device's LEDs in their current colours: matrix zones as a key grid, linear zones as rows of dots,
// single zones as one larger swatch. Everything scales with the width of the preview (ADR 0001 #28): dots and keys
// grow with the space up to a cap, long rows wrap. In the device view it is also where LEDs are selected
// (ADR 0001 #22) and where a right-click on a zone opens its menu (ADR 0001 #26).
import { computed, onBeforeUnmount, ref } from 'vue'
import type { DeviceDescription, Zone } from '../ipc'
import { matrixCells } from '../lib/device'
import { beginSweep, selectZone, sweepOver, zoneIndices, EMPTY, type Selection, type SweepMode } from '../lib/selection'

const props = withDefaults(
  defineProps<{
    description: DeviceDescription
    colors: readonly string[]
    selection?: Selection
    /** LEDs can be selected (device view in static colour mode). */
    interactive?: boolean
    /** `stage`: large with zone names (device view); `tile`: small and passive (overview tile). */
    variant?: 'stage' | 'tile'
    /** Indices of hidden zones. */
    hiddenZones?: ReadonlySet<number>
    /** Draw hidden zones (greyed, labelled, not selectable) instead of leaving them out. */
    showHidden?: boolean
  }>(),
  { selection: () => EMPTY, interactive: false, variant: 'tile', hiddenZones: () => new Set<number>(), showHidden: false },
)

const emit = defineEmits<{
  select: [selection: Selection]
  /** Right-click on a zone's name or one of its LEDs (stage only). */
  'zone-menu': [zone: number, event: MouseEvent]
}>()

interface ZoneLayout {
  zone: Zone
  index: number
  kind: 'matrix' | 'linear' | 'single'
  width: number
  hidden: boolean
  cells: { key: number; led: number | null }[]
}

const stage = computed(() => props.variant === 'stage')

const layout = computed<ZoneLayout[]>(() =>
  props.description.zones.flatMap((zone, index): ZoneLayout[] => {
    const hidden = props.hiddenZones.has(index)
    if (hidden && !props.showHidden) return []
    const matrix = matrixCells(zone)
    if (matrix) return [{ zone, index, kind: 'matrix', width: matrix.width, hidden, cells: matrix.cells }]
    const leds = zoneIndices(zone).map((led) => ({ key: led, led }))
    return [{ zone, index, kind: zone.kind === 'Single' ? 'single' : 'linear', width: 0, hidden, cells: leds }]
  }),
)

const color = (led: number) => props.colors[led] ?? '#000000'
const isSelected = (leds: number[]) => leds.length > 0 && leds.every((i) => props.selection.has(i))
const selectable = (z: ZoneLayout) => props.interactive && !z.hidden

// Drag sweep: the mode of the stroke is fixed at pointer-down.
const sweep = ref<SweepMode | null>(null)
let current: Selection = EMPTY

function endSweep() {
  sweep.value = null
  window.removeEventListener('pointerup', endSweep)
  window.removeEventListener('pointercancel', endSweep)
}

function onDown(e: PointerEvent, z: ZoneLayout, leds: number[]) {
  if (!selectable(z) || e.button !== 0 || leds.length === 0) return
  e.preventDefault()
  // Mouse pointers are not captured implicitly, but release defensively so pointerenter reaches other LEDs.
  ;(e.target as Element).releasePointerCapture?.(e.pointerId)
  const [first, ...rest] = leds as [number, ...number[]]
  const start = beginSweep(props.selection, first, e.ctrlKey || e.metaKey)
  current = rest.reduce((sel, i) => sweepOver(sel, i, start.mode), start.sel)
  sweep.value = start.mode
  emit('select', current)
  window.addEventListener('pointerup', endSweep)
  window.addEventListener('pointercancel', endSweep)
}

function onEnter(z: ZoneLayout, leds: number[]) {
  if (!sweep.value || z.hidden) return
  const mode = sweep.value
  const next = leds.reduce((sel, i) => sweepOver(sel, i, mode), current)
  if (next !== current) {
    current = next
    emit('select', current)
  }
}

function onZoneName(e: MouseEvent, z: ZoneLayout) {
  if (!selectable(z)) return
  emit('select', selectZone(props.selection, zoneIndices(z.zone), e.ctrlKey || e.metaKey))
}

function onMenu(e: MouseEvent, z: ZoneLayout) {
  if (!stage.value) return
  e.preventDefault()
  emit('zone-menu', z.index, e)
}

const singleLeds = (z: ZoneLayout) => z.cells.map((c) => c.led as number)

onBeforeUnmount(endSweep)
</script>

<template>
  <div class="preview" :class="[variant, { interactive }]">
    <div
      v-for="z in layout"
      :key="z.index"
      class="zone"
      :class="[z.kind, { hidden: z.hidden }]"
      @contextmenu="onMenu($event, z)"
    >
      <div v-if="stage" class="zone-head">
        <button
          v-if="selectable(z)"
          type="button"
          class="zone-name link"
          :title="`Select ${z.zone.name} (Ctrl-click to add)`"
          @click="onZoneName($event, z)"
        >
          {{ z.zone.name }}
        </button>
        <span v-else class="zone-name">{{ z.zone.name }}</span>
        <span v-if="z.hidden" class="hidden-label">Hidden</span>
      </div>

      <div
        v-if="z.kind === 'matrix'"
        class="matrix"
        :style="{ gridTemplateColumns: `repeat(${z.width}, minmax(0, 1fr))`, '--w': z.width }"
      >
        <template v-for="cell in z.cells" :key="cell.key">
          <span v-if="cell.led === null" class="cell empty" />
          <span
            v-else
            class="cell led"
            :class="{ selected: selectable(z) && selection.has(cell.led) }"
            :style="{ backgroundColor: color(cell.led) }"
            :title="stage ? description.leds[cell.led]?.name : undefined"
            @pointerdown="onDown($event, z, [cell.led])"
            @pointerenter="onEnter(z, [cell.led])"
          />
        </template>
      </div>

      <div v-else-if="z.kind === 'single'" class="singles">
        <span
          class="swatch led"
          :class="{ selected: selectable(z) && isSelected(singleLeds(z)) }"
          :style="{ backgroundColor: z.cells[0] ? color(z.cells[0].led as number) : undefined }"
          @pointerdown="onDown($event, z, singleLeds(z))"
          @pointerenter="onEnter(z, singleLeds(z))"
        />
      </div>

      <div v-else class="strip" :style="{ '--n': Math.max(1, z.cells.length) }">
        <span
          v-for="cell in z.cells"
          :key="cell.key"
          class="dot led"
          :class="{ selected: selectable(z) && selection.has(cell.led as number) }"
          :style="{ backgroundColor: color(cell.led as number) }"
          :title="stage ? description.leds[cell.led as number]?.name : undefined"
          @pointerdown="onDown($event, z, [cell.led as number])"
          @pointerenter="onEnter(z, [cell.led as number])"
        />
        <span v-if="z.cells.length === 0" class="muted small">No LEDs</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Sizes follow the preview's width (container query units): a row of n dots shares the width, clamped between a
   readable minimum (long zones wrap) and a cap (short zones do not balloon). */
.preview {
  container-type: inline-size;
  display: flex;
  flex-direction: column;
  user-select: none;
}

.preview.stage {
  --gap: 8px;
  --dot-min: 14px;
  --dot-max: 36px;
  --key-max: 56px;
  gap: var(--space-6);
}

.preview.tile {
  --gap: 3px;
  --dot-min: 6px;
  --dot-max: 18px;
  --key-max: 22px;
  gap: var(--space-2);
}

.zone {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.zone.hidden {
  opacity: 0.4;
}

.zone-head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  font-size: var(--font-size-sm);
}

.zone-name {
  color: var(--color-text-muted);
}

.hidden-label {
  padding: 0 var(--space-2);
  border: 1px solid var(--color-border-strong);
  border-radius: 999px;
  color: var(--color-text-muted);
}

.led {
  border: 1px solid var(--color-border-strong);
  transition: background-color var(--transition);
}

.interactive .zone:not(.hidden) .led {
  cursor: pointer;
}

.led.selected {
  outline: 2px solid var(--color-accent);
  outline-offset: 2px;
}

.matrix {
  display: grid;
  gap: 4px;
  max-width: calc(var(--w) * var(--key-max));
}

.tile .matrix {
  gap: 2px;
}

.cell {
  aspect-ratio: 1;
  border-radius: 4px;
}

.tile .cell {
  border-radius: 2px;
}

.cell.empty {
  border: 0;
}

.strip {
  display: flex;
  flex-wrap: wrap;
  gap: var(--gap);
}

.dot {
  flex: none;
  width: clamp(var(--dot-min), calc((100cqi - (var(--n) - 1) * var(--gap)) / var(--n)), var(--dot-max));
  aspect-ratio: 1;
  border-radius: 50%;
}

.swatch {
  display: block;
  width: clamp(96px, 24cqi, 220px);
  aspect-ratio: 16 / 9;
  border-radius: var(--radius);
}

.tile .swatch {
  width: clamp(32px, 16cqi, 72px);
  aspect-ratio: 2 / 1;
  border-radius: 4px;
}
</style>
