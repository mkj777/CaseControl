<script setup lang="ts">
// Page header (docs/design-system.md, "Page header"). Title with an optional status badge (slot `badge`), back link
// on detail pages, at most one description line. Slot `actions` holds the page's view actions (Rescan, Scan, Show
// hidden). Slot `controls` holds page-wide lighting controls (brightness, All off, Release all; ADR 0001 #33): with
// it the header gets one toolbar row under the title, lighting controls on the left and the actions on the right,
// instead of putting the actions next to the title.
import { RouterLink, type RouteLocationRaw } from 'vue-router'

withDefaults(
  defineProps<{
    /** Left out only when the page body is an EmptyState with its own h1 ("Device not found"). */
    title?: string
    description?: string
    back?: { to: RouteLocationRaw; label: string }
  }>(),
  { title: undefined, description: undefined, back: undefined },
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
        <p v-if="description" class="line">{{ description }}</p>
      </div>
      <div v-if="$slots.actions && !$slots.controls" class="actions"><slot name="actions" /></div>
    </header>
    <div v-if="title && $slots.controls" class="toolbar">
      <div class="group"><slot name="controls" /></div>
      <div v-if="$slots.actions" class="group"><slot name="actions" /></div>
    </div>
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
}

.title-row h1 {
  margin: 0;
}

.line {
  max-width: var(--measure);
  margin-top: var(--space-3);
  color: var(--text-muted);
}

.actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--space-5);
}

/* One row: lighting controls left, view actions right. It wraps only when the window is too narrow. */
.toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4) var(--space-8);
  margin-top: var(--space-6);
}

.group {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-4) var(--space-6);
}
</style>
