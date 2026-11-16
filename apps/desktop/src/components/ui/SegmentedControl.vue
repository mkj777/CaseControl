<script setup lang="ts" generic="T extends string">
// 2 to 4 short, mutually exclusive options visible at once (docs/design-system.md, "Segmented control"). A radio
// group: Tab reaches the selected segment, arrow keys move the selection. Five or more options belong in a Select.
import { ref } from 'vue'

const props = withDefaults(
  defineProps<{ modelValue: T; options: readonly { value: T; label: string }[]; disabled?: boolean }>(),
  { disabled: false },
)
const emit = defineEmits<{ 'update:modelValue': [value: T] }>()

const buttons = ref<HTMLButtonElement[]>([])

function select(value: T) {
  if (!props.disabled && value !== props.modelValue) emit('update:modelValue', value)
}

function onKey(e: KeyboardEvent, index: number) {
  const n = props.options.length
  const step: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }
  let next: number | null = null
  if (e.key in step) next = (index + (step[e.key] ?? 0) + n) % n
  else if (e.key === 'Home') next = 0
  else if (e.key === 'End') next = n - 1
  if (next === null) return
  e.preventDefault()
  const option = props.options[next]
  if (!option) return
  select(option.value)
  buttons.value[next]?.focus()
}

/** The selected segment is the tab stop; with no match the first one is. */
const tabStop = (value: T, index: number) =>
  value === props.modelValue || (index === 0 && !props.options.some((o) => o.value === props.modelValue))
</script>

<template>
  <div class="segmented" role="radiogroup" :aria-disabled="disabled || undefined">
    <button
      v-for="(o, i) in options"
      :key="o.value"
      ref="buttons"
      type="button"
      role="radio"
      class="segment"
      :class="{ selected: o.value === modelValue }"
      :aria-checked="o.value === modelValue"
      :tabindex="tabStop(o.value, i) ? 0 : -1"
      :disabled="disabled"
      @click="select(o.value)"
      @keydown="onKey($event, i)"
    >
      {{ o.label }}
    </button>
  </div>
</template>

<style scoped>
.segmented {
  display: inline-grid;
  grid-auto-columns: 1fr;
  grid-auto-flow: column;
  height: var(--control-height);
  padding: 2px;
  border: 1px solid var(--field-border);
  border-radius: var(--radius-md);
  background: var(--field-bg);
}

.segment {
  height: 100%;
  padding: 0 var(--space-3);
  border: 0;
  border-radius: calc(var(--radius-md) - 3px);
  color: var(--text-muted);
  background: none;
}

.segment:hover:not(:disabled) {
  color: var(--text);
  background: none;
}

.segment.selected,
.segment.selected:hover:not(:disabled) {
  color: var(--text);
  background: var(--surface-hover);
}

.segment:disabled {
  color: var(--text-disabled);
}

.segment:focus-visible {
  outline-offset: 0;
}
</style>
