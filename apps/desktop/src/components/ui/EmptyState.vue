<script setup lang="ts">
// Why a page is empty and what to do (docs/design-system.md, "Empty state"): a heading, one or two sentences in the
// default slot, at most one action (slot `action`, a primary Button). No illustration, no icon. `level` 1 is for
// a page whose whole content is the empty state ("Device not found"); it keeps the h2 look.
withDefaults(defineProps<{ title: string; level?: 1 | 2 }>(), { level: 2 })
</script>

<template>
  <div class="empty">
    <component :is="level === 1 ? 'h1' : 'h2'" class="title">{{ title }}</component>
    <p class="text"><slot /></p>
    <div v-if="$slots.action"><slot name="action" /></div>
  </div>
</template>

<style scoped>
.empty {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: var(--space-4);
  max-width: 56ch;
  padding: var(--space-14) 0;
}

.title {
  margin: 0;
  font-size: var(--text-lg);
}

.text {
  color: var(--text-muted);
}
</style>
