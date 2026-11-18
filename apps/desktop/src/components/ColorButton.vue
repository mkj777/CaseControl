<script setup lang="ts">
// A colour circle the size of a control that opens a ColorPicker in a popover below it (docs/design-system.md,
// "Colour button"; ADR 0001 #37: the fallback colour in the Lighting toolbar). It passes the picker's events
// through: `update:modelValue` while dragging, `change` with the final value of a gesture. The popover is
// non-modal: Escape closes it and returns focus to the circle; a click outside or moving focus out closes it too.
// `align="end"` lines the popover up with the circle's right edge, for a circle at the right end of a row.
import { nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import ColorPicker from './ColorPicker.vue'

const props = withDefaults(
  defineProps<{ modelValue: string; label: string; disabled?: boolean; align?: 'start' | 'end' }>(),
  { disabled: false, align: 'start' },
)
const emit = defineEmits<{ 'update:modelValue': [hex: string]; change: [hex: string] }>()

const panelId = useId()
const open = ref(false)
const root = ref<HTMLElement | null>(null)
const trigger = ref<HTMLButtonElement | null>(null)
const panel = ref<HTMLElement | null>(null)

function onPointerDown(e: PointerEvent) {
  if (root.value && !root.value.contains(e.target as Node)) open.value = false
}

function listen(on: boolean) {
  const method = on ? 'addEventListener' : 'removeEventListener'
  window[method]('pointerdown', onPointerDown as EventListener, true)
}

watch(open, async (isOpen) => {
  listen(isOpen)
  if (!isOpen) return
  await nextTick()
  // Start on the current swatch when there is one, else on the first one.
  const el = panel.value?.querySelector<HTMLElement>('.swatch.current') ?? panel.value?.querySelector('button')
  el?.focus()
})

watch(
  () => props.disabled,
  (disabled) => {
    if (disabled) open.value = false
  },
)

onBeforeUnmount(() => listen(false))

function onKey(e: KeyboardEvent) {
  if (e.key !== 'Escape' || !open.value) return
  e.stopPropagation()
  open.value = false
  trigger.value?.focus()
}

function onFocusOut(e: FocusEvent) {
  const next = e.relatedTarget as Node | null
  if (next && root.value && !root.value.contains(next)) open.value = false
}
</script>

<template>
  <span ref="root" class="color-button" @keydown="onKey" @focusout="onFocusOut">
    <button
      ref="trigger"
      type="button"
      class="circle"
      :class="{ open }"
      :style="{ backgroundColor: modelValue }"
      :aria-label="label"
      aria-haspopup="dialog"
      :aria-expanded="open"
      :aria-controls="open ? panelId : undefined"
      :disabled="disabled"
      @click="open = !open"
    />
    <div v-if="open" :id="panelId" ref="panel" class="panel" :class="{ end: align === 'end' }" role="dialog" :aria-label="label">
      <ColorPicker
        :model-value="modelValue"
        :disabled="disabled"
        @update:model-value="emit('update:modelValue', $event)"
        @change="emit('change', $event)"
      />
    </div>
  </span>
</template>

<style scoped>
.color-button {
  position: relative;
  display: inline-flex;
  flex: none;
}

/* Follows the swatch rules: LED edge, hover edge, the selection outline while its popover is open. */
.circle {
  width: var(--control-height);
  padding: 0;
  border: 1px solid var(--led-border);
  border-radius: var(--radius-full);
}

.circle:hover:not(:disabled) {
  border-color: var(--text-muted);
}

.circle.open {
  outline: 2px solid var(--selection);
  outline-offset: 2px;
}

.circle:disabled {
  opacity: 0.5;
}

/* Looks like the context menu: a surface panel with the strong edge and the overlay shadow. */
.panel {
  position: absolute;
  top: calc(100% + var(--space-2));
  left: 0;
  z-index: var(--z-menu);
  width: var(--popover-width);
  padding: var(--space-4);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-md);
  background: var(--surface);
  box-shadow: var(--shadow-overlay);
}

.panel.end {
  right: 0;
  left: auto;
}
</style>
