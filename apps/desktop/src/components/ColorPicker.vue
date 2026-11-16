<script setup lang="ts">
// Swatches for quick clicks plus a free picker that folds out below: saturation/value area, hue bar and hex input
// (ADR 0001 #20). Emits every change while dragging; the caller throttles what it sends.
import { computed, ref, watch } from 'vue'
import { SWATCHES, hexToHsv, hsvToHex, normalizeHex, sameColor, type Hsv } from '../lib/color'

const props = withDefaults(defineProps<{ modelValue: string; disabled?: boolean }>(), { disabled: false })
const emit = defineEmits<{ 'update:modelValue': [hex: string] }>()

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
      <button type="button" class="link more" :aria-expanded="open" :disabled="disabled" @click="open = !open">
        {{ open ? 'Fewer options' : 'Custom colour' }}
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
        <input
          v-model="hexInput"
          type="text"
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

.swatch {
  width: 36px;
  height: 36px;
  padding: 0;
  border: 1px solid var(--color-border-strong);
  border-radius: 50%;
}

.swatch:hover:not(:disabled) {
  border-color: var(--color-text-muted);
}

.swatch.current {
  outline: 2px solid var(--color-accent);
  outline-offset: 3px;
}

.more {
  margin-left: var(--space-2);
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
  border-radius: var(--radius);
  background-image:
    linear-gradient(to top, #000, rgba(0, 0, 0, 0)),
    linear-gradient(to right, #fff, rgba(255, 255, 255, 0));
  cursor: crosshair;
  touch-action: none;
}

.hue {
  position: relative;
  height: 16px;
  border-radius: 8px;
  background: linear-gradient(to right, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00);
  cursor: pointer;
  touch-action: none;
}

.thumb {
  position: absolute;
  width: 16px;
  height: 16px;
  border: 2px solid #fff;
  border-radius: 50%;
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.6);
  transform: translate(-50%, -50%);
  pointer-events: none;
}

.hue .thumb {
  top: 50%;
}

.hex input {
  width: 110px;
  font-family: ui-monospace, Consolas, monospace;
}

.current-color {
  width: 24px;
  height: 24px;
  border: 1px solid var(--color-border-strong);
  border-radius: 4px;
}
</style>
