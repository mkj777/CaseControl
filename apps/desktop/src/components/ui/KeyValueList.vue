<script setup lang="ts">
// Key-value list (docs/design-system.md, "Key-value list"): a <dl> grid of muted terms and values. The slot holds
// <dt>/<dd> pairs; give a <dd> the class `mono` for ids, hex values and firmware strings.
</script>

<template>
  <dl class="kv"><slot /></dl>
</template>

<style scoped>
.kv {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: var(--space-2) var(--space-6);
  margin: 0;
}

.kv :slotted(dt) {
  color: var(--text-muted);
}

.kv :slotted(dd) {
  margin: 0;
  min-width: 0;
  overflow-wrap: anywhere;
}

.kv :slotted(dd.mono) {
  font-family: var(--font-mono);
}
</style>
