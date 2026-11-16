<script setup lang="ts">
// Controls for one hardware mode, limited to what its capabilities allow (ADR 0001 #25): speed, brightness,
// direction and mode colours. Emits the whole new state on every change.
import { computed, ref, watch } from 'vue'
import type { Direction, Mode, ModeState } from '../ipc'
import { bounds, directionsOf, hasCap } from '../lib/device'
import ColorPicker from './ColorPicker.vue'

const props = withDefaults(defineProps<{ mode: Mode; state: ModeState; disabled?: boolean }>(), { disabled: false })
const emit = defineEmits<{ update: [state: ModeState] }>()

const speed = computed(() => (hasCap(props.mode, 'SPEED') && props.mode.speed ? bounds(props.mode.speed) : null))
const brightness = computed(() =>
  hasCap(props.mode, 'BRIGHTNESS') && props.mode.brightness ? bounds(props.mode.brightness) : null,
)
const directions = computed(() => directionsOf(props.mode))
const colorRange = computed(() =>
  props.mode.color_mode === 'ModeSpecific' && props.mode.colors ? bounds(props.mode.colors) : null,
)

const activeColor = ref(0)
watch(
  () => props.state.colors.length,
  (n) => {
    if (activeColor.value >= n) activeColor.value = Math.max(0, n - 1)
  },
)

function update(patch: Partial<ModeState>) {
  emit('update', { ...props.state, ...patch })
}

function setColor(hex: string) {
  const colors = [...props.state.colors]
  colors[activeColor.value] = hex
  update({ colors })
}

function addColor() {
  const colors = [...props.state.colors, props.state.colors.at(-1) ?? '#FFFFFF']
  activeColor.value = colors.length - 1
  update({ colors })
}

function removeColor() {
  const colors = props.state.colors.filter((_, i) => i !== activeColor.value)
  update({ colors })
}

const DIRECTION_LABEL: Record<Direction, string> = {
  Left: 'Left',
  Right: 'Right',
  Up: 'Up',
  Down: 'Down',
  Horizontal: 'Horizontal',
  Vertical: 'Vertical',
}
</script>

<template>
  <div class="controls">
    <label v-if="speed" class="control">
      <span class="label">Speed</span>
      <input
        type="range"
        :min="speed[0]"
        :max="speed[1]"
        step="1"
        :value="state.speed ?? speed[0]"
        :disabled="disabled"
        @input="update({ speed: Number(($event.target as HTMLInputElement).value) })"
      />
      <span class="value">{{ state.speed }}</span>
    </label>

    <label v-if="brightness" class="control">
      <span class="label">Brightness</span>
      <input
        type="range"
        :min="brightness[0]"
        :max="brightness[1]"
        step="1"
        :value="state.brightness ?? brightness[1]"
        :disabled="disabled"
        @input="update({ brightness: Number(($event.target as HTMLInputElement).value) })"
      />
      <span class="value">{{ state.brightness }}</span>
    </label>

    <label v-if="directions.length > 0" class="control">
      <span class="label">Direction</span>
      <select
        :value="state.direction ?? directions[0]"
        :disabled="disabled"
        @change="update({ direction: ($event.target as HTMLSelectElement).value as Direction })"
      >
        <option v-for="d in directions" :key="d" :value="d">{{ DIRECTION_LABEL[d] }}</option>
      </select>
    </label>

    <div v-if="colorRange && colorRange[1] > 0" class="colors">
      <span class="label">Colours</span>
      <div class="row">
        <button
          v-for="(c, i) in state.colors"
          :key="i"
          type="button"
          class="chip"
          :class="{ current: i === activeColor }"
          :style="{ backgroundColor: c }"
          :aria-label="`Colour ${i + 1}`"
          :disabled="disabled"
          @click="activeColor = i"
        />
        <button
          v-if="state.colors.length < colorRange[1]"
          type="button"
          :disabled="disabled"
          @click="addColor"
        >
          Add colour
        </button>
        <button
          v-if="state.colors.length > colorRange[0] && state.colors.length > 0"
          type="button"
          :disabled="disabled"
          @click="removeColor"
        >
          Remove colour
        </button>
      </div>
      <ColorPicker
        v-if="state.colors.length > 0"
        :model-value="state.colors[activeColor] ?? '#FFFFFF'"
        :disabled="disabled"
        @update:model-value="setColor"
      />
    </div>
    <p v-else-if="mode.color_mode === 'Random'" class="muted">The device picks the colours in this mode.</p>
    <p v-else-if="mode.color_mode === 'PerLed'" class="muted">This mode uses the current LED colours.</p>

    <p v-if="!speed && !brightness && directions.length === 0 && !colorRange && mode.color_mode === 'None'" class="muted">
      This mode has no settings.
    </p>
  </div>
</template>

<style scoped>
.controls {
  display: flex;
  flex-direction: column;
  gap: var(--space-5);
}

.control {
  display: grid;
  grid-template-columns: 120px minmax(0, 320px) 48px;
  align-items: center;
  gap: var(--space-4);
}

.label {
  color: var(--color-text-muted);
}

.value {
  font-variant-numeric: tabular-nums;
  color: var(--color-text-muted);
}

.colors {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.chip {
  width: 40px;
  height: 40px;
  padding: 0;
  border: 1px solid var(--color-border-strong);
  border-radius: var(--radius);
}

.chip.current {
  outline: 2px solid var(--color-accent);
  outline-offset: 3px;
}
</style>
