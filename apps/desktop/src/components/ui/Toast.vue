<script setup lang="ts">
// Short-lived message about an action whose result is not visible where the user is looking (docs/design-system.md,
// "Toast"), e.g. "3 devices released". One at a time, at the bottom right of the window over the content. It
// dismisses itself after `duration` ms or with its close button. A new toast (another `toast.id`) replaces the
// current one and starts the timer again. The live region is always in the DOM so screen readers announce changes.
import { onBeforeUnmount, watch } from 'vue'
import IconButton from './IconButton.vue'

const props = withDefaults(
  defineProps<{
    /** The toast to show; `id` tells a new toast from the same text shown again. */
    toast: { id: number; text: string } | null
    duration?: number
  }>(),
  { duration: 5000 },
)
const emit = defineEmits<{ dismiss: [] }>()

let timer: ReturnType<typeof setTimeout> | undefined

function stop() {
  if (timer !== undefined) clearTimeout(timer)
  timer = undefined
}

watch(
  () => props.toast?.id,
  (id) => {
    stop()
    if (id !== undefined) timer = setTimeout(() => emit('dismiss'), props.duration)
  },
  { immediate: true },
)

onBeforeUnmount(stop)

/** Segoe Fluent Icons "ChromeClose", as on the window's close button. */
const CLOSE = ''
</script>

<template>
  <div class="toast-region" role="status" aria-live="polite">
    <div v-if="toast" :key="toast.id" class="toast">
      <span class="text">{{ toast.text }}</span>
      <IconButton label="Dismiss" :glyph="CLOSE" @click="emit('dismiss')" />
    </div>
  </div>
</template>

<style scoped>
/* Fixed to the window corner, over the content; empty (and sizeless) while no toast shows. */
.toast-region {
  position: fixed;
  right: var(--space-6);
  bottom: var(--space-6);
  z-index: var(--z-menu);
}

.toast {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  max-width: var(--dialog-width);
  padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-md);
  color: var(--text);
  background: var(--surface-raised);
  box-shadow: var(--shadow-overlay);
}

.text {
  flex: 1;
  min-width: 0;
}
</style>
