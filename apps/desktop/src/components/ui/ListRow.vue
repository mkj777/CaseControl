<script setup lang="ts">
// One row of a plain list (docs/design-system.md, "List row"); use inside a <ul>. Name on the left followed by the
// default slot (a badge, a muted detail), actions on the right (slot `actions`, `sm` buttons: one secondary, the
// rest ghost). Leave `name` out and put an inline form into the default slot for the editing state.
withDefaults(defineProps<{ name?: string }>(), { name: undefined })
</script>

<template>
  <li class="list-row">
    <div class="main">
      <span v-if="name" class="name">{{ name }}</span>
      <slot />
    </div>
    <div v-if="$slots.actions" class="actions"><slot name="actions" /></div>
  </li>
</template>

<style scoped>
.list-row {
  display: flex;
  align-items: center;
  gap: var(--space-6);
  min-height: 72px;
  padding: var(--space-4) var(--space-2);
  border-bottom: 1px solid var(--border);
}

.list-row:first-child {
  border-top: 1px solid var(--border);
}

.main {
  display: flex;
  flex: 1;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-4);
  min-width: 0;
}

.name {
  font-size: var(--text-lg);
  line-height: var(--leading-tight);
}

.actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--space-2);
}
</style>
