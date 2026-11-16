<script setup lang="ts">
// Controls for one hardware mode, limited to what its capabilities allow (ADR 0001 #25): speed, brightness,
// direction and mode colours. Emits the whole new state on every change.
import { computed, ref, watch } from 'vue'
import type { Direction, Mode, ModeState } from '../ipc'
import { bounds, directionsOf, hasCap } from '../lib/device'
import ColorPicker from './ColorPicker.vue'
import { Button, Field, SegmentedControl, Select, Slider } from './ui'

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

/** Up to four directions fit a segmented control; more use a menu. */
const directionOptions = computed(() => directions.value.map((d) => ({ value: d, label: DIRECTION_LABEL[d] })))

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
    <Field v-if="speed" label="Speed">
      <template #default="{ id }">
        <Slider
          :id="id"
          :model-value="state.speed ?? speed[0]"
          :min="speed[0]"
          :max="speed[1]"
          :disabled="disabled"
          @update:model-value="update({ speed: $event })"
        />
      </template>
      <template #value>{{ state.speed }}</template>
    </Field>

    <Field v-if="brightness" label="Brightness">
      <template #default="{ id }">
        <Slider
          :id="id"
          :model-value="state.brightness ?? brightness[1]"
          :min="brightness[0]"
          :max="brightness[1]"
          :disabled="disabled"
          @update:model-value="update({ brightness: $event })"
        />
      </template>
      <template #value>{{ state.brightness }}</template>
    </Field>

    <Field v-if="directions.length > 0 && directions.length <= 4" label="Direction" group>
      <SegmentedControl
        :model-value="state.direction ?? directions[0] ?? 'Left'"
        :options="directionOptions"
        :disabled="disabled"
        @update:model-value="update({ direction: $event })"
      />
    </Field>
    <Field v-else-if="directions.length > 4" v-slot="{ id }" label="Direction">
      <Select
        :id="id"
        :model-value="state.direction ?? directions[0] ?? 'Left'"
        :disabled="disabled"
        @update:model-value="update({ direction: $event as Direction })"
      >
        <option v-for="d in directions" :key="d" :value="d">{{ DIRECTION_LABEL[d] }}</option>
      </Select>
    </Field>

    <Field v-if="colorRange && colorRange[1] > 0" label="Colours" layout="stacked" group>
      <div class="colors">
        <div class="row">
          <button
            v-for="(c, i) in state.colors"
            :key="i"
            type="button"
            class="chip"
            :class="{ current: i === activeColor }"
            :style="{ backgroundColor: c }"
            :aria-label="`Colour ${i + 1}`"
            :aria-pressed="i === activeColor"
            :disabled="disabled"
            @click="activeColor = i"
          />
          <Button v-if="state.colors.length < colorRange[1]" :disabled="disabled" @click="addColor">Add colour</Button>
          <Button
            v-if="state.colors.length > colorRange[0] && state.colors.length > 0"
            :disabled="disabled"
            @click="removeColor"
          >
            Remove colour
          </Button>
        </div>
        <ColorPicker
          v-if="state.colors.length > 0"
          :model-value="state.colors[activeColor] ?? '#FFFFFF'"
          :disabled="disabled"
          @update:model-value="setColor"
        />
      </div>
    </Field>
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

.colors {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  width: 100%;
}

/* Colour chips follow the LED border and selection rules. */
.chip {
  width: var(--control-height);
  padding: 0;
  border: 1px solid var(--led-border);
  border-radius: var(--radius-sm);
}

.chip:hover:not(:disabled) {
  border-color: var(--text-muted);
}

.chip.current {
  outline: 2px solid var(--selection);
  outline-offset: 2px;
}
</style>
