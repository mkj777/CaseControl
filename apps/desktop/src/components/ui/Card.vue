<script setup lang="ts">
// Static container (docs/design-system.md, "Card and tile"): conflict findings, the preview stage (`padding="lg"`).
// Cards are not nested; stacked cards are --space-4 apart.
withDefaults(defineProps<{ as?: string; padding?: 'md' | 'lg' }>(), { as: 'div', padding: 'md' })
</script>

<template>
  <component :is="as" class="card" :class="padding">
    <slot />
  </component>
</template>

<style scoped>
.card {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  padding: var(--space-6);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  background: var(--surface);
}

.card.lg {
  padding: var(--space-10);
}

.card + .card {
  margin-top: var(--space-4);
}
</style>
