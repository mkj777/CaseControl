<script setup lang="ts">
// Swatches for quick clicks plus a free picker that folds out below: saturation/value area, hue bar and hex input
// (ADR 0001 #20). The last circle in the swatch row (eyedropper icon, "Custom colour", pressed while open) folds the
// free picker out and in (ADR 0001 #37). Emits every change while dragging (`update:modelValue`); the caller
// throttles what it sends. `change` comes with the final value of a gesture: when a drag ends, and with every
// click, key step or hex entry.
import { computed, ref, watch } from 'vue'
import { SWATCHES, hexToHsv, hsvToHex, normalizeHex, sameColor, type Hsv } from '../lib/color'
import { TextField } from './ui'

const props = withDefaults(defineProps<{ modelValue: string; disabled?: boolean }>(), { disabled: false })
const emit = defineEmits<{ 'update:modelValue': [hex: string]; change: [hex: string] }>()

const open = ref(false)
// Kept separately from the hex value so hue survives greys and black.
const hsv = ref<Hsv>(hexToHsv(props.modelValue))
const hexInput = ref(props.modelValue)

watch(
  () => props.modelValue,
  (hex) => {
    hexInput.value = hex
    if (sameColor(hsvToHex(hsv.value), hex)) return
    const next = hexToHsv(hex)
    hsv.value = next.s === 0 || next.v === 0 ? { ...next, h: hsv.value.h } : next
  },
)

function set(next: Hsv) {
  hsv.value = next
  const hex = hsvToHex(next)
  hexInput.value = hex
  emit('update:modelValue', hex)
}

function pick(hex: string) {
  if (props.disabled) return
  emit('update:modelValue', hex)
  emit('change', hex)
}

const hueColor = computed(() => hsvToHex({ h: hsv.value.h, s: 1, v: 1 }))
const clamp01 = (x: number) => Math.min(1, Math.max(0, x))

function track(e: PointerEvent, apply: (x: number, y: number) => void) {
  if (props.disabled || e.button !== 0) return
  const el = e.currentTarget as HTMLElement
  el.setPointerCapture(e.pointerId)
  const update = (ev: PointerEvent) => {
    const r = el.getBoundingClientRect()
    apply(clamp01((ev.clientX - r.left) / r.width), clamp01((ev.clientY - r.top) / r.height))
  }
  update(e)
  const stop = () => {
    el.removeEventListener('pointermove', update)
    el.removeEventListener('pointerup', stop)
    el.removeEventListener('pointercancel', stop)
    emit('change', hsvToHex(hsv.value))
  }
  el.addEventListener('pointermove', update)
  el.addEventListener('pointerup', stop)
  el.addEventListener('pointercancel', stop)
}

const onArea = (e: PointerEvent) => track(e, (x, y) => set({ h: hsv.value.h, s: x, v: 1 - y }))
const onHue = (e: PointerEvent) => track(e, (x) => set({ ...hsv.value, h: Math.min(359.9, x * 360) }))

function nudge(e: KeyboardEvent, target: 'area' | 'hue') {
  const step = e.shiftKey ? 0.1 : 0.02
  const { h, s, v } = hsv.value
  const keys: Record<string, () => Hsv> =
    target === 'area'
      ? {
          ArrowLeft: () => ({ h, s: clamp01(s - step), v }),
          ArrowRight: () => ({ h, s: clamp01(s + step), v }),
          ArrowUp: () => ({ h, s, v: clamp01(v + step) }),
          ArrowDown: () => ({ h, s, v: clamp01(v - step) }),
        }
      : {
          ArrowLeft: () => ({ h: Math.max(0, h - step * 360), s, v }),
          ArrowRight: () => ({ h: Math.min(359.9, h + step * 360), s, v }),
        }
  const f = keys[e.key]
  if (!f || props.disabled) return
  e.preventDefault()
  set(f())
  emit('change', hsvToHex(hsv.value))
}

function onHex() {
  const hex = normalizeHex(hexInput.value)
  if (hex) pick(hex)
  else hexInput.value = props.modelValue
}
</script>

<template>
  <div class="picker" :class="{ disabled }">
    <div class="swatches" role="group" aria-label="Colours">
      <button
        v-for="s in SWATCHES"
        :key="s.hex"
        type="button"
        class="swatch"
        :class="{ current: sameColor(s.hex, modelValue) }"
        :style="{ backgroundColor: s.hex }"
        :title="s.name"
        :aria-label="s.name"
        :disabled="disabled"
        @click="pick(s.hex)"
      />
      <!-- Folds the free picker out below; it does not sample the screen (ADR 0001 #37). -->
      <button
        type="button"
        class="swatch custom-toggle"
        aria-label="Custom colour"
        title="Custom colour"
        :aria-pressed="open"
        :disabled="disabled"
        @click="open = !open"
      >
        <!-- Eyedropper, after Lucide "pipette" (ISC). -->
        <svg
          viewBox="0 0 24 24"
          width="16"
          height="16"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="m2 22 1-1h3l9-9" />
          <path d="M3 21v-3l9-9" />
          <path d="m15 6 3.4-3.4a2.1 2.1 0 1 1 3 3L18 9l.4.4a2.1 2.1 0 1 1-3 3l-3.8-3.8a2.1 2.1 0 1 1 3-3l.4.4Z" />
        </svg>
      </button>
    </div>

    <div v-if="open" class="custom">
      <div
        class="area"
        tabindex="0"
        role="slider"
        aria-label="Saturation and brightness"
        :aria-valuetext="`saturation ${Math.round(hsv.s * 100)}%, brightness ${Math.round(hsv.v * 100)}%`"
        :style="{ backgroundColor: hueColor }"
        @pointerdown="onArea"
        @keydown="nudge($event, 'area')"
      >
        <span class="thumb" :style="{ left: `${hsv.s * 100}%`, top: `${(1 - hsv.v) * 100}%` }" />
      </div>
      <div
        class="hue"
        tabindex="0"
        role="slider"
        aria-label="Hue"
        aria-valuemin="0"
        aria-valuemax="360"
        :aria-valuenow="Math.round(hsv.h)"
        @pointerdown="onHue"
        @keydown="nudge($event, 'hue')"
      >
        <span class="thumb" :style="{ left: `${(hsv.h / 360) * 100}%` }" />
      </div>
      <label class="hex row">
        <span class="muted small">Hex</span>
        <span class="current-color" :style="{ backgroundColor: modelValue }" />
        <TextField
          v-model="hexInput"
          class="hex-input"
          mono
          maxlength="7"
          spellcheck="false"
          :disabled="disabled"
          @change="onHex"
          @keydown.enter="onHex"
        />
      </label>
    </div>
  </div>
</template>

<style scoped>
.picker {
  display: flex;
  flex-direction: column;
  gap: var(--space-5);
}

.disabled {
  opacity: 0.5;
}

.swatches {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-3);
}

/* Swatches follow the LED border and selection rules. */
.swatch {
  width: var(--control-height);
  padding: 0;
  border: 1px solid var(--led-border);
  border-radius: var(--radius-full);
}

.swatch:hover:not(:disabled) {
  border-color: var(--text-muted);
}

.swatch.current {
  outline: 2px solid var(--selection);
  outline-offset: 2px;
}

/* The custom toggle has the swatch's size and edge but no colour of its own: a raised surface with the icon. */
.custom-toggle {
  color: var(--text-muted);
  background: var(--surface-raised);
  transition:
    color var(--transition),
    background-color var(--transition),
    border-color var(--transition);
}

.custom-toggle:hover:not(:disabled) {
  color: var(--text);
  background: var(--surface-hover);
}

.custom-toggle[aria-pressed='true'] {
  color: var(--text);
  background: var(--surface-pressed);
  border-color: var(--text-muted);
}

.custom {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  max-width: 420px;
}

/* The two overlays are the picker's function (saturation and value axes), not decoration. */
.area {
  position: relative;
  height: 200px;
  border-radius: var(--radius-md);
  background-image:
    linear-gradient(to top, #000, rgba(0, 0, 0, 0)),
    linear-gradient(to right, #fff, rgba(255, 255, 255, 0));
  cursor: crosshair;
  touch-action: none;
}

.hue {
  position: relative;
  height: 16px;
  border-radius: var(--radius-full);
  background: linear-gradient(to right, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00);
  cursor: pointer;
  touch-action: none;
}

/* The thumb sits on arbitrary colours, so it is white with a dark edge, like the native pickers. */
.thumb {
  position: absolute;
  width: 16px;
  height: 16px;
  border: 2px solid #fff;
  border-radius: var(--radius-full);
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.6);
  transform: translate(-50%, -50%);
  pointer-events: none;
}

.hue .thumb {
  top: 50%;
}

.hex-input {
  width: 110px;
}

.current-color {
  width: 24px;
  height: 24px;
  border: 1px solid var(--led-border);
  border-radius: var(--radius-sm);
}
</style>
