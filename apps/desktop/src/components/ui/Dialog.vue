<script setup lang="ts">
// Modal dialog shell (docs/design-system.md, "Dialog"). On open, focus moves to the element marked `data-autofocus`
// (the safe choice), else to the first focusable one; Tab stays inside; Escape and a backdrop click emit `close`
// unless `busy`; on close, focus returns to the element that had it before. Body in the default slot, buttons in
// the `actions` slot (Cancel first). Never `window.confirm` or `alert`.
import { nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'

const props = withDefaults(
  defineProps<{ open: boolean; title: string; role?: 'dialog' | 'alertdialog'; busy?: boolean }>(),
  { role: 'dialog', busy: false },
)
const emit = defineEmits<{ close: [] }>()

const titleId = `${useId()}-title`
const panel = ref<HTMLElement | null>(null)
let opener: HTMLElement | null = null

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), ' +
  '[tabindex]:not([tabindex="-1"])'

function focusables(): HTMLElement[] {
  return panel.value ? [...panel.value.querySelectorAll<HTMLElement>(FOCUSABLE)] : []
}

watch(
  () => props.open,
  async (open) => {
    if (open) {
      opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
      window.addEventListener('keydown', onKey, true)
      await nextTick()
      const first = panel.value?.querySelector<HTMLElement>('[data-autofocus]') ?? focusables()[0] ?? panel.value
      first?.focus()
    } else {
      window.removeEventListener('keydown', onKey, true)
      restoreFocus()
    }
  },
  { immediate: true },
)

function restoreFocus() {
  const el = opener
  opener = null
  if (el?.isConnected) el.focus()
}

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKey, true)
  restoreFocus()
})

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    if (!props.busy) emit('close')
    return
  }
  if (e.key !== 'Tab') return
  const items = focusables()
  const first = items[0]
  const last = items.at(-1)
  if (!first || !last) {
    e.preventDefault()
    return
  }
  const active = document.activeElement
  if (e.shiftKey && (active === first || !panel.value?.contains(active))) {
    e.preventDefault()
    last.focus()
  } else if (!e.shiftKey && (active === last || !panel.value?.contains(active))) {
    e.preventDefault()
    first.focus()
  }
}

function onBackdrop() {
  if (!props.busy) emit('close')
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="backdrop" @click.self="onBackdrop">
      <div ref="panel" class="dialog" :role="role" aria-modal="true" :aria-labelledby="titleId" tabindex="-1">
        <h2 :id="titleId" class="title">{{ title }}</h2>
        <div class="body"><slot /></div>
        <div v-if="$slots.actions" class="actions"><slot name="actions" /></div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  z-index: var(--z-dialog);
  display: grid;
  place-items: center;
  padding: var(--space-6);
  background: var(--backdrop);
}

.dialog {
  width: min(var(--dialog-width), 100%);
  max-height: 100%;
  overflow-y: auto;
  padding: var(--space-6);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  background: var(--surface);
  box-shadow: var(--shadow-overlay);
}

.title {
  margin-bottom: var(--space-4);
}

.body {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  color: var(--text-muted);
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-3);
  margin-top: var(--space-6);
}
</style>
