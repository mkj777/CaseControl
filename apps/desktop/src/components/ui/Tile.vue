<script setup lang="ts">
// A card that is a link (docs/design-system.md, "Card and tile"): the whole tile opens `to`. Head with name,
// metadata and a badge (slot `badge`), the default slot is pinned to the bottom (the compact preview).
// Hidden (ADR 0001 #27): greyed, not a link, but still focusable so its context menu opens from the keyboard.
import { RouterLink, type RouteLocationRaw } from 'vue-router'

withDefaults(defineProps<{ to: RouteLocationRaw; name: string; meta?: string; hidden?: boolean }>(), {
  meta: undefined,
  hidden: false,
})
</script>

<template>
  <div v-if="hidden" class="tile hidden" tabindex="0" aria-disabled="true">
    <div class="content">
      <div class="head">
        <div>
          <div class="name">{{ name }}</div>
          <div v-if="meta" class="meta">{{ meta }}</div>
        </div>
        <slot name="badge" />
      </div>
      <div class="foot"><slot /></div>
    </div>
  </div>
  <RouterLink v-else :to="to" class="tile">
    <div class="content">
      <div class="head">
        <div>
          <div class="name">{{ name }}</div>
          <div v-if="meta" class="meta">{{ meta }}</div>
        </div>
        <slot name="badge" />
      </div>
      <div class="foot"><slot /></div>
    </div>
  </RouterLink>
</template>

<style scoped>
.tile {
  display: flex;
  min-height: 220px;
  padding: var(--space-6);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  color: inherit;
  text-decoration: none;
  background: var(--surface);
  transition:
    border-color var(--transition),
    background-color var(--transition);
}

.tile:not(.hidden):hover {
  border-color: var(--border-strong);
  background: var(--surface-raised);
}

.content {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: var(--space-6);
  min-width: 0;
}

.hidden .content {
  opacity: 0.45;
}

.head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: var(--space-4);
}

.name {
  font-size: var(--text-lg);
  font-weight: var(--weight-strong);
  line-height: var(--leading-tight);
}

.meta {
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.foot {
  margin-top: auto;
}
</style>
