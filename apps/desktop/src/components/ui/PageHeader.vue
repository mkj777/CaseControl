<script setup lang="ts">
// Page header (docs/design-system.md, "Page header"). List pages: title, description, actions (slot `actions`, at
// most two, secondary). Detail pages: back link, title with a status badge (slot `badge`), metadata line. Slot
// `controls` holds page-wide controls in a row under the title (brightness, All off; ADR 0001 #33).
import { RouterLink, type RouteLocationRaw } from 'vue-router'

withDefaults(
  defineProps<{
    /** Left out only when the page body is an EmptyState with its own h1 ("Device not found"). */
    title?: string
    description?: string
    /** Metadata line under the title on detail pages ("Kind · Vendor"). */
    meta?: string
    back?: { to: RouteLocationRaw; label: string }
  }>(),
  { title: undefined, description: undefined, meta: undefined, back: undefined },
)
</script>

<template>
  <div class="page-header" :class="{ bare: !title }">
    <RouterLink v-if="back" :to="back.to" class="back">← {{ back.label }}</RouterLink>
    <header v-if="title" class="head">
      <div class="text">
        <div class="title-row">
          <h1>{{ title }}</h1>
          <slot name="badge" />
        </div>
        <p v-if="meta" class="line">{{ meta }}</p>
        <p v-if="description" class="line">{{ description }}</p>
      </div>
      <div v-if="$slots.actions" class="actions"><slot name="actions" /></div>
    </header>
    <div v-if="title && $slots.controls" class="controls"><slot name="controls" /></div>
  </div>
</template>

<style scoped>
.page-header {
  margin-bottom: var(--space-10);
}

.back {
  display: inline-block;
  margin-bottom: var(--space-6);
  color: var(--text-muted);
  text-decoration: none;
  transition: color var(--transition);
}

.back:hover {
  color: var(--text);
}

.bare .back {
  margin-bottom: 0;
}

.head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-6);
}

.title-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}

.title-row h1 {
  margin: 0;
}

.line {
  max-width: var(--measure);
  color: var(--text-muted);
}

.actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--space-5);
}

.controls {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-4) var(--space-8);
  margin-top: var(--space-6);
}
</style>
