<script setup lang="ts">
// Single-line text field (docs/design-system.md, "Text field"). Every field has a visible label (Field) or, in a
// one-field form, an aria-label plus a placeholder that names it. `mono` for hex values. Other attributes
// (placeholder, maxlength, aria-*) and listeners go to the <input>.
withDefaults(defineProps<{ modelValue: string; mono?: boolean; invalid?: boolean; disabled?: boolean }>(), {
  mono: false,
  invalid: false,
  disabled: false,
})
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
</script>

<template>
  <input
    type="text"
    class="text-field"
    :class="{ mono }"
    :value="modelValue"
    :disabled="disabled"
    :aria-invalid="invalid || undefined"
    @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
  />
</template>

<style scoped>
.text-field {
  min-width: 0;
}

.mono {
  font-family: var(--font-mono);
}
</style>
