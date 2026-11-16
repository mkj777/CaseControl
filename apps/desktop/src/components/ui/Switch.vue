<script setup lang="ts">
// Toggle switch for settings that apply at once (docs/design-system.md, "Switch"): label and hint on the left,
// switch on the right; the whole row is the click target and Space toggles. `compact` sits inline (page headers)
// instead of filling a 640 px settings row.
import { useId } from 'vue'

const props = withDefaults(
  defineProps<{ modelValue: boolean; label: string; hint?: string; disabled?: boolean; compact?: boolean }>(),
  { hint: undefined, disabled: false, compact: false },
)
const emit = defineEmits<{ 'update:modelValue': [value: boolean] }>()

const id = useId()

function toggle() {
  if (!props.disabled) emit('update:modelValue', !props.modelValue)
}
</script>

<template>
  <button
    type="button"
    role="switch"
    class="switch"
    :class="{ compact, on: modelValue }"
    :aria-checked="modelValue"
    :aria-labelledby="`${id}-label`"
    :aria-describedby="hint ? `${id}-hint` : undefined"
    :disabled="disabled"
    @click="toggle"
  >
    <span class="text">
      <span :id="`${id}-label`" class="label">{{ label }}</span>
      <span v-if="hint" :id="`${id}-hint`" class="hint">{{ hint }}</span>
    </span>
    <span class="track" aria-hidden="true"><span class="thumb" /></span>
  </button>
</template>

<style scoped>
.switch {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-6);
  width: 100%;
  max-width: var(--row-max-width);
  height: auto;
  padding: 0;
  border: 0;
  border-radius: var(--radius-md);
  text-align: left;
  white-space: normal;
  color: var(--text);
  background: none;
}

.switch:hover:not(:disabled),
.switch:active:not(:disabled) {
  background: none;
}

.switch.compact {
  display: inline-flex;
  align-items: center;
  gap: var(--space-3);
  width: auto;
}

.compact .label {
  color: var(--text-muted);
}

.text {
  display: flex;
  flex-direction: column;
}

.hint {
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.switch:disabled .label {
  color: var(--text-disabled);
}

.track {
  position: relative;
  flex: none;
  width: var(--switch-width);
  height: var(--switch-height);
  margin-top: 1px;
  border: 1px solid var(--field-border-hover);
  border-radius: var(--radius-full);
  background: var(--surface-raised);
  transition:
    border-color var(--transition),
    background-color var(--transition);
}

.compact .track {
  margin-top: 0;
}

.thumb {
  position: absolute;
  top: 50%;
  left: 3px;
  width: calc(var(--switch-height) - 8px);
  height: calc(var(--switch-height) - 8px);
  border-radius: var(--radius-full);
  background: var(--text-muted);
  transform: translateY(-50%);
  transition:
    left var(--transition),
    background-color var(--transition);
}

.on .track {
  border-color: var(--accent);
  background: var(--accent);
}

.on .thumb {
  left: calc(var(--switch-width) - var(--switch-height) + 3px);
  background: var(--accent-text);
}

.switch:disabled .track {
  opacity: 0.5;
}
</style>
