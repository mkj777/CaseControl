<script setup lang="ts">
// Whole-number field with its range shown next to it (docs/design-system.md, "Number field"). Commits on change or
// Enter, not per keystroke. Out-of-range input is clamped to the range on commit (decision 12); input that is not
// a number goes back to the current value. Attributes (id, aria-label, aria-describedby) go to the <input>.
import { computed, ref, useAttrs, useId, watch } from 'vue'

defineOptions({ inheritAttrs: false })

const props = withDefaults(defineProps<{ modelValue: number; min: number; max: number; disabled?: boolean }>(), {
  disabled: false,
})
const emit = defineEmits<{ 'update:modelValue': [value: number] }>()

const attrs = useAttrs()
const hintId = `${useId()}-range`
const describedBy = computed(() => [attrs['aria-describedby'], hintId].filter(Boolean).join(' '))
const width = computed(() => `${Math.max(String(props.min).length, String(props.max).length) + 4}ch`)

const text = ref(String(props.modelValue))
let dirty = false
watch(
  () => props.modelValue,
  (v) => {
    text.value = String(v)
    dirty = false
  },
)

function onInput(e: Event) {
  text.value = (e.target as HTMLInputElement).value
  dirty = true
}

function commit() {
  if (!dirty) return
  dirty = false
  const raw = text.value.trim()
  const n = Math.round(Number(raw))
  if (raw === '' || !Number.isFinite(n)) {
    text.value = String(props.modelValue)
    return
  }
  const clamped = Math.min(props.max, Math.max(props.min, n))
  text.value = String(clamped)
  if (clamped !== props.modelValue) emit('update:modelValue', clamped)
}
</script>

<template>
  <span class="number-field">
    <input
      v-bind="attrs"
      :value="text"
      type="number"
      :min="min"
      :max="max"
      step="1"
      :disabled="disabled"
      :aria-describedby="describedBy"
      :style="{ width }"
      @input="onInput"
      @change="commit"
      @keydown.enter="commit"
    />
    <span :id="hintId" class="range">{{ min }} to {{ max }}</span>
  </span>
</template>

<style scoped>
.number-field {
  display: inline-flex;
  align-items: center;
  gap: var(--space-3);
}

.range {
  font-size: var(--text-sm);
  white-space: nowrap;
  color: var(--text-muted);
}
</style>
