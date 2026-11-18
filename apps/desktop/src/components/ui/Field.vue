<script setup lang="ts">
// A labelled control (docs/design-system.md, "Field"): label, control, optional hint and inline error. `row` puts
// the label in a 120 px column, the control in up to 320 px and the `value` slot (a readout) in 48 px; `stacked`
// puts the label above; `inline` sizes the label to its text and the control to --toolbar-control-width (down to
// --toolbar-control-min-width when the toolbar row is tight), for a toolbar. The default slot receives `id` (for the control), `describedBy` and `invalid` to wire up.
// `group` is for a set of controls (colour chips): the label then names a role="group" instead of one control.
import { computed, useId } from 'vue'

const props = withDefaults(
  defineProps<{
    label: string
    hint?: string
    error?: string | null
    layout?: 'row' | 'stacked' | 'inline'
    group?: boolean
  }>(),
  { hint: undefined, error: null, layout: 'row', group: false },
)

const id = useId()
const labelId = `${id}-label`
const hintId = `${id}-hint`
const errorId = `${id}-error`
const invalid = computed(() => !!props.error)
const describedBy = computed(
  () => [props.hint ? hintId : null, props.error ? errorId : null].filter(Boolean).join(' ') || undefined,
)
</script>

<template>
  <div
    class="field"
    :class="layout"
    :role="group ? 'group' : undefined"
    :aria-labelledby="group ? labelId : undefined"
    :aria-describedby="group ? describedBy : undefined"
  >
    <span v-if="group" :id="labelId" class="label">{{ label }}</span>
    <label v-else :id="labelId" :for="id" class="label">{{ label }}</label>
    <div class="control">
      <slot :id="id" :describedBy="describedBy" :invalid="invalid" />
    </div>
    <div v-if="$slots.value" class="value"><slot name="value" /></div>
    <p v-if="hint" :id="hintId" class="hint">{{ hint }}</p>
    <p v-if="error" :id="errorId" class="error">{{ error }}</p>
  </div>
</template>

<style scoped>
.field {
  display: grid;
  align-items: center;
  column-gap: var(--space-4);
  row-gap: var(--space-2);
}

.row {
  grid-template-columns: 120px minmax(0, var(--control-max-width)) 48px;
}

.stacked {
  grid-template-columns: minmax(0, 1fr);
  max-width: var(--row-max-width);
}

.inline {
  grid-template-columns: auto minmax(var(--toolbar-control-min-width), var(--toolbar-control-width)) auto;
  column-gap: var(--space-3);
}

.label {
  color: var(--text-muted);
}

.control {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  min-width: 0;
}

.row .control > :deep(select),
.row .control > :deep(input[type='range']),
.inline .control > :deep(input[type='range']) {
  width: 100%;
}

.value {
  font-variant-numeric: tabular-nums;
  color: var(--text-muted);
}

.hint,
.error {
  font-size: var(--text-sm);
}

.row .hint,
.row .error,
.inline .hint,
.inline .error {
  grid-column: 2 / 4;
}

.hint {
  color: var(--text-muted);
}

.error {
  color: var(--error);
  /* Error text can be copied (ADR 0001 #32). */
  user-select: text;
  cursor: text;
}
</style>
