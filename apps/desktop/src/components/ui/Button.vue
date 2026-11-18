<script setup lang="ts">
// Button (docs/design-system.md, "Button"). `secondary` for almost everything, one `primary` per view, `ghost` for
// low-weight row actions, `danger` for actions that end, stop, write flash or delete (always behind a ConfirmDialog).
// Busy: the caller puts what is happening into the label ("Scanning…") and disables the button; no spinner.
withDefaults(
  defineProps<{
    variant?: 'secondary' | 'primary' | 'ghost' | 'danger'
    size?: 'md' | 'sm'
    type?: 'button' | 'submit'
    disabled?: boolean
  }>(),
  { variant: 'secondary', size: 'md', type: 'button', disabled: false },
)
</script>

<template>
  <button :type="type" class="btn" :class="[variant, size]" :disabled="disabled">
    <slot />
  </button>
</template>

<style scoped>
.btn.sm {
  height: var(--control-height-sm);
  padding: 0 var(--space-3);
}

.primary {
  border-color: var(--accent);
  color: var(--accent-text);
  background: var(--accent);
}

.primary:hover:not(:disabled) {
  border-color: var(--accent-hover);
  background: var(--accent-hover);
}

.primary:active:not(:disabled) {
  background: var(--accent);
}

.primary:disabled {
  border-color: var(--border);
  color: var(--text-disabled);
  background: var(--surface-raised);
}

.ghost {
  border-color: transparent;
  color: var(--text-muted);
  background: none;
}

.ghost:hover:not(:disabled),
.ghost:active:not(:disabled) {
  border-color: transparent;
  color: var(--text);
  background: none;
}

.ghost:disabled {
  color: var(--text-disabled);
}

.danger {
  color: var(--danger);
}

.danger:hover:not(:disabled) {
  border-color: var(--danger);
}

.danger:disabled {
  color: var(--text-disabled);
}
</style>
