<script setup lang="ts">
// One row of a plain list (docs/design-system.md, "List row"); use inside a <ul>. Name on the left followed by the
// default slot (a badge, a muted detail), actions on the right (slot `actions`, `sm` buttons: one secondary, the
// rest ghost). Leave `name` out and put an inline form into the default slot for the editing state. Slot `details`
// is shown below the row, inside its divider (an expandable row, e.g. a conflict finding); the page decides when and
// passes `detailsId` for the toggle's `aria-controls`. `stackActions` makes the row adapt to its own width: when the
// row is narrower than 640 px the actions move to their own line under the name instead of squeezing it (a conflict
// finding with a long "End process …" label); by default the actions stay on the right at their width.
withDefaults(defineProps<{ name?: string; detailsId?: string; stackActions?: boolean }>(), {
  name: undefined,
  detailsId: undefined,
  stackActions: false,
})
</script>

<template>
  <li class="list-row" :class="{ stack: stackActions }">
    <div class="line">
      <div class="main">
        <span v-if="name" class="name">{{ name }}</span>
        <slot />
      </div>
      <div v-if="$slots.actions" class="actions"><slot name="actions" /></div>
    </div>
    <div v-if="$slots.details" :id="detailsId" class="details"><slot name="details" /></div>
  </li>
</template>

<style scoped>
.list-row {
  display: flex;
  flex-direction: column;
  justify-content: center;
  min-height: 72px;
  padding: var(--space-4) var(--space-2);
  border-bottom: 1px solid var(--border);
}

.list-row:first-child {
  border-top: 1px solid var(--border);
}

.line {
  display: flex;
  align-items: center;
  gap: var(--space-6);
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

/* Opt-in: the row is a size container; below 640 px (a text page at windows narrower than about 940 px) the actions
   take their own line under the name, left aligned, and wrap if they still do not fit. Custom properties cannot be
   used in container queries, so the width is written out. */
.list-row.stack {
  container: list-row / inline-size;
}

@container list-row (max-width: 639px) {
  .line {
    flex-wrap: wrap;
    row-gap: var(--space-3);
  }

  .actions {
    flex: 1 1 100%;
    flex-wrap: wrap;
  }
}

.details {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  padding: var(--space-4) 0 var(--space-2);
}
</style>
