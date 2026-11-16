<script setup lang="ts">
// Page-level notice (docs/design-system.md, "Notice and inline error"). The default slot is the first line in
// --text; the `details` slot is smaller muted text below. `warning` and `error` add a coloured 3 px left edge; the
// text itself stays neutral. Success messages use `info`.
withDefaults(defineProps<{ variant?: 'info' | 'warning' | 'error' }>(), { variant: 'info' })
</script>

<template>
  <div class="notice" :class="variant" :role="variant === 'error' ? 'alert' : 'status'">
    <p class="main"><slot /></p>
    <div v-if="$slots.details" class="details"><slot name="details" /></div>
  </div>
</template>

<style scoped>
.notice {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  max-width: calc(var(--measure) + 2 * var(--space-5));
  margin-bottom: var(--space-6);
  padding: var(--space-4) var(--space-5);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-md);
  color: var(--text);
  background: var(--surface);
}

/* Stacked notices sit closer together. */
.notice:has(+ .notice) {
  margin-bottom: var(--space-3);
}

.warning {
  border-left: 3px solid var(--warning);
}

.error {
  border-left: 3px solid var(--error);
}

.details {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  font-size: var(--text-sm);
  color: var(--text-muted);
}
</style>
