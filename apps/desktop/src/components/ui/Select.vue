<script setup lang="ts">
// Native select styled like a text field (docs/design-system.md, "Select and menu"), for five or more options or
// long labels. Options go into the slot; disabled placeholder options ("Device default") stay as they are.
withDefaults(defineProps<{ modelValue: string; disabled?: boolean }>(), { disabled: false })
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
</script>

<template>
  <select
    class="select"
    :value="modelValue"
    :disabled="disabled"
    @change="emit('update:modelValue', ($event.target as HTMLSelectElement).value)"
  >
    <slot />
  </select>
</template>

<style scoped>
.select {
  min-width: 0;
  max-width: var(--control-max-width);
}
</style>
