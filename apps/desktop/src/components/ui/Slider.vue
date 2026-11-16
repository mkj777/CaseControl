<script setup lang="ts">
// Native range input (docs/design-system.md, "Slider"). `update:modelValue` fires while dragging (live, ADR 0001
// #21), `change` once the drag ends. Always show the value next to it (Field's `value` slot or a NumberField).
withDefaults(defineProps<{ modelValue: number; min: number; max: number; step?: number; disabled?: boolean }>(), {
  step: 1,
  disabled: false,
})
const emit = defineEmits<{ 'update:modelValue': [value: number]; change: [value: number] }>()

const valueOf = (e: Event) => Number((e.target as HTMLInputElement).value)
</script>

<template>
  <input
    type="range"
    class="slider"
    :min="min"
    :max="max"
    :step="step"
    :value="modelValue"
    :disabled="disabled"
    @input="emit('update:modelValue', valueOf($event))"
    @change="emit('change', valueOf($event))"
  />
</template>

<style scoped>
.slider {
  max-width: var(--control-max-width);
  margin: 0;
}
</style>
