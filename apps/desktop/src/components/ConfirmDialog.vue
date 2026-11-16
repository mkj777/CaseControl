<script setup lang="ts">
// In-app confirmation (never `window.confirm`). The body slot says what will happen.
import { nextTick, ref, watch } from 'vue'

const props = withDefaults(
  defineProps<{ open: boolean; title: string; confirmLabel?: string; busy?: boolean }>(),
  { confirmLabel: 'Confirm', busy: false },
)
const emit = defineEmits<{ confirm: []; cancel: [] }>()

const cancelButton = ref<HTMLButtonElement | null>(null)

watch(
  () => props.open,
  async (open) => {
    if (!open) return
    await nextTick()
    // Focus the safe choice first.
    cancelButton.value?.focus()
  },
)

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape' && !props.busy) emit('cancel')
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="backdrop" @keydown="onKey" @click.self="!busy && emit('cancel')">
      <div class="dialog" role="alertdialog" aria-modal="true" :aria-label="title">
        <h2>{{ title }}</h2>
        <div class="body"><slot /></div>
        <div class="actions">
          <button ref="cancelButton" type="button" :disabled="busy" @click="emit('cancel')">Cancel</button>
          <button type="button" class="primary" :disabled="busy" @click="emit('confirm')">{{ confirmLabel }}</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  z-index: 10;
  display: grid;
  place-items: center;
  padding: var(--space-6);
  background: rgba(0, 0, 0, 0.6);
}

.dialog {
  width: min(480px, 100%);
  padding: var(--space-6);
  border: 1px solid var(--color-border-strong);
  border-radius: var(--radius-lg);
  background: var(--color-surface);
}

.body {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  color: var(--color-text-muted);
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-3);
  margin-top: var(--space-6);
}
</style>
