<script setup lang="ts">
// Draws a device's LEDs in their current colours: matrix zones as a key grid, linear zones as rows of dots,
// single zones as one larger swatch. In the device view it is also where LEDs are selected (ADR 0001 #22).
import { computed, onBeforeUnmount, ref } from 'vue'
import type { DeviceDescription, Zone } from '../ipc'
import { matrixCells } from '../lib/device'
import { beginSweep, selectZone, sweepOver, zoneIndices, EMPTY, type Selection, type SweepMode } from '../lib/selection'

const props = withDefaults(
  defineProps<{
    description: DeviceDescription
    colors: readonly string[]
    selection?: Selection
    /** Large, labelled and selectable (device view) vs. small and passive (overview tile). */
    interactive?: boolean
  }>(),
  { selection: () => EMPTY, interactive: false },
)

const emit = defineEmits<{ select: [selection: Selection] }>()

interface ZoneLayout {
  zone: Zone
  index: number
  kind: 'matrix' | 'linear' | 'single'
  width: number
  cells: { key: number; led: number | null }[]
}

const layout = computed<ZoneLayout[]>(() =>
  props.description.zones.map((zone, index) => {
    const matrix = matrixCells(zone)
    if (matrix) return { zone, index, kind: 'matrix', width: matrix.width, cells: matrix.cells }
    const leds = zoneIndices(zone).map((led) => ({ key: led, led }))
    return { zone, index, kind: zone.kind === 'Single' ? 'single' : 'linear', width: 0, cells: leds }
  }),
)

const color = (led: number) => props.colors[led] ?? '#000000'
const isSelected = (leds: number[]) => leds.length > 0 && leds.every((i) => props.selection.has(i))

// Drag sweep: the mode of the stroke is fixed at pointer-down.
const sweep = ref<SweepMode | null>(null)
let current: Selection = EMPTY

function endSweep() {
  sweep.value = null
  window.removeEventListener('pointerup', endSweep)
  window.removeEventListener('pointercancel', endSweep)
}

function onDown(e: PointerEvent, leds: number[]) {
  if (!props.interactive || e.button !== 0 || leds.length === 0) return
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

function onEnter(leds: number[]) {
  if (!sweep.value) return
  const mode = sweep.value
  const next = leds.reduce((sel, i) => sweepOver(sel, i, mode), current)
  if (next !== current) {
    current = next
    emit('select', current)
  }
}

function onZoneName(e: MouseEvent, zone: Zone) {
  emit('select', selectZone(props.selection, zoneIndices(zone), e.ctrlKey || e.metaKey))
}

onBeforeUnmount(endSweep)
</script>

<template>
  <div class="preview" :class="{ interactive, compact: !interactive }">
    <div v-for="z in layout" :key="z.index" class="zone" :class="z.kind">
      <button
        v-if="interactive"
        type="button"
        class="zone-name link"
        :title="`Select ${z.zone.name} (Ctrl-click to add)`"
        @click="onZoneName($event, z.zone)"
      >
        {{ z.zone.name }}
      </button>

      <div
        v-if="z.kind === 'matrix'"
        class="matrix"
        :style="{ gridTemplateColumns: `repeat(${z.width}, minmax(0, 1fr))` }"
      >
        <template v-for="cell in z.cells" :key="cell.key">
          <span v-if="cell.led === null" class="cell empty" />
          <span
            v-else
            class="cell led"
            :class="{ selected: interactive && selection.has(cell.led) }"
            :style="{ backgroundColor: color(cell.led) }"
            :title="interactive ? description.leds[cell.led]?.name : undefined"
            @pointerdown="onDown($event, [cell.led])"
            @pointerenter="onEnter([cell.led])"
          />
        </template>
      </div>

      <div v-else-if="z.kind === 'single'" class="singles">
        <span
          class="swatch led"
          :class="{ selected: interactive && isSelected(z.cells.map((c) => c.led as number)) }"
          :style="{ backgroundColor: z.cells[0] ? color(z.cells[0].led as number) : undefined }"
          @pointerdown="onDown($event, z.cells.map((c) => c.led as number))"
          @pointerenter="onEnter(z.cells.map((c) => c.led as number))"
        />
      </div>

      <div v-else class="strip">
        <span
          v-for="cell in z.cells"
          :key="cell.key"
          class="dot led"
          :class="{ selected: interactive && selection.has(cell.led as number) }"
          :style="{ backgroundColor: color(cell.led as number) }"
          :title="interactive ? description.leds[cell.led as number]?.name : undefined"
          @pointerdown="onDown($event, [cell.led as number])"
          @pointerenter="onEnter([cell.led as number])"
        />
        <span v-if="z.cells.length === 0" class="muted small">No LEDs</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.preview {
  display: flex;
  flex-direction: column;
  user-select: none;
}

.preview.interactive {
  gap: var(--space-6);
}

.preview.compact {
  gap: var(--space-2);
}

.zone {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.zone-name {
  align-self: flex-start;
  font-size: var(--font-size-sm);
}

.led {
  border: 1px solid var(--color-border-strong);
  transition: background-color var(--transition);
}

.interactive .led {
  cursor: pointer;
}

.led.selected {
  outline: 2px solid var(--color-accent);
  outline-offset: 2px;
}

.matrix {
  display: grid;
  gap: 4px;
}

.compact .matrix {
  gap: 2px;
}

.cell {
  aspect-ratio: 1;
  border-radius: 4px;
}

.compact .cell {
  border-radius: 2px;
}

.cell.empty {
  border: 0;
}

.strip {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.compact .strip {
  gap: 3px;
}

.dot {
  width: 22px;
  height: 22px;
  border-radius: 50%;
}

.compact .dot {
  width: 8px;
  height: 8px;
}

.swatch {
  display: block;
  width: 96px;
  height: 56px;
  border-radius: var(--radius);
}

.compact .swatch {
  width: 32px;
  height: 16px;
  border-radius: 4px;
}
</style>
