<script setup lang="ts">
// Page header (docs/design-system.md, "Page header"). Title with an optional status badge (slot `badge`), back link
// on detail pages, at most one description line. Slot `actions` holds the page's view actions (Rescan, Scan, Show
// hidden). Slot `controls` holds page-wide lighting controls (ADR 0001 #33, #35, #37): with it the header gets one
// toolbar row under the title, the `controls` group on the left and the `actions` group on the right, instead of
// putting the actions next to the title. One group is the flexible one: it starts from its narrowest width (a
// brightness slider at --toolbar-control-min-width) and grows into the free space, so the slider gives way before the
// row wraps. That is the left group, or with `flexActions` the right one (Lighting, ADR 0001 #41: profile, takeover,
// Show hidden and Rescan on the left; All brightness and the fallback colour on the right). `titleHidden` keeps the h1 for screen readers only, for a page
// the sidebar already names (Lighting): the toolbar row is then the first thing on the page.
import { RouterLink, type RouteLocationRaw } from 'vue-router'

withDefaults(
  defineProps<{
    /** Left out only when the page body is an EmptyState with its own h1 ("Device not found"). */
    title?: string
    description?: string
    back?: { to: RouteLocationRaw; label: string }
    /** Renders the title as a visually hidden h1: the page keeps its name, the sidebar shows where you are. */
    titleHidden?: boolean
    /** The lighting controls are too wide for the narrowest content (Lighting): below 680 px of toolbar width they
     * take the first row on their own and wrap. */
    wrapControls?: boolean
    /** The right group (slot `actions`) is the flexible one instead of the left: its content sits at the right edge
     * and its slider shrinks before the row wraps; the left group keeps its own width. */
    flexActions?: boolean
  }>(),
  {
    title: undefined,
    description: undefined,
    back: undefined,
    titleHidden: false,
    wrapControls: false,
    flexActions: false,
  },
)
</script>

<template>
  <div class="page-header" :class="{ bare: !title }">
    <RouterLink v-if="back" :to="back.to" class="back">← {{ back.label }}</RouterLink>
    <h1 v-if="title && titleHidden" class="visually-hidden">{{ title }}</h1>
    <header v-if="title && !titleHidden" class="head">
      <div class="text">
        <div class="title-row">
          <h1>{{ title }}</h1>
          <slot name="badge" />
        </div>
        <p v-if="description" class="line">{{ description }}</p>
      </div>
      <div v-if="$slots.actions && !$slots.controls" class="actions"><slot name="actions" /></div>
    </header>
    <div
      v-if="title && ($slots.controls || (titleHidden && $slots.actions))"
      class="toolbar"
      :class="{ first: titleHidden, 'wrap-controls': wrapControls, 'flex-actions': flexActions }"
    >
      <div v-if="$slots.controls" class="group lighting"><slot name="controls" /></div>
      <div v-if="$slots.actions" class="group view"><slot name="actions" /></div>
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

/* One row: lighting controls left, view actions right. The lighting group starts from its narrowest width (the
   brightness slider at --toolbar-control-min-width) and grows into the free space, so the slider gives way before
   the row wraps; only when even that does not fit do the view actions move to a second row. With `wrapControls`, when
   the content is too narrow for the lighting group itself (the media query at the end), the lighting controls take
   the first row on their own and wrap, so nothing runs past the edge. */
.toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4) var(--space-6);
  margin-top: var(--space-6);
}

.toolbar.first {
  margin-top: 0;
}

.group {
  display: flex;
  align-items: center;
  gap: var(--space-4);
}

.lighting {
  flex: 1 1 0;
  min-width: min-content;
}

.view {
  flex: none;
}

/* `flexActions`: the right group is the flexible one. Its base width is its narrowest (slider at its minimum), so the
   row wraps only when even that does not fit; it then takes the free space and keeps its content at the right edge
   (also when it has moved to a row of its own). The left group keeps the width of its content. */
.flex-actions .lighting {
  flex: none;
}

.flex-actions .view {
  flex: 1 1 0;
  justify-content: flex-end;
  min-width: min-content;
}

/* Below 964 px window width the content is narrower than 680 px (sidebar 220 px, page padding 2 x 32 px), too narrow
   for both Lighting toolbar groups side by side: the left group then takes the first row on its own and wraps. Media queries cannot read custom properties, so
   the number lives here and in docs/design-system.md ("Page header") only. */
@media (max-width: 963px) {
  .wrap-controls .lighting {
    flex-basis: 100%;
    flex-wrap: wrap;
  }
}

.visually-hidden {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
  border: 0;
}
</style>
