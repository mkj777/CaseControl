<script setup lang="ts">
// A short text shown next to a trigger on hover and on keyboard focus (docs/design-system.md, "Tooltip"; ADR 0001
// #37). The default slot is the trigger and receives `describedBy`, the id of the tooltip text, for its
// `aria-describedby`; the text is always in the DOM, so screen readers read it with the trigger. It opens below the
// trigger, stays open while the pointer is over the trigger or the tooltip, and Escape closes it (WCAG 1.4.13).
// Text only, no interactive content.
import { ref, useId } from 'vue'

defineProps<{ text: string }>()

const id = useId()
const open = ref(false)
let hovered = false
let focused = false

function sync() {
  open.value = hovered || focused
}

function onHover(on: boolean) {
  hovered = on
  sync()
}

function onFocusOut() {
  focused = false
  sync()
}

function onFocusIn(e: FocusEvent) {
  // Keyboard focus only; a click focuses the trigger too, and hover already covers the pointer.
  focused = (e.target as HTMLElement).matches(':focus-visible')
  sync()
}

function onKey(e: KeyboardEvent) {
  if (e.key !== 'Escape' || !open.value) return
  e.stopPropagation()
  open.value = false
}
</script>

<template>
  <span
    class="tooltip-anchor"
    @pointerenter="onHover(true)"
    @pointerleave="onHover(false)"
    @focusin="onFocusIn"
    @focusout="onFocusOut"
    @keydown="onKey"
  >
    <slot :describedBy="id" />
    <span :id="id" role="tooltip" class="tooltip" :class="{ open }">
      <span class="box">{{ text }}</span>
    </span>
  </span>
</template>

<style scoped>
.tooltip-anchor {
  position: relative;
  display: inline-flex;
}

/* The outer span reaches up to the trigger (its top padding is the gap), so moving the pointer from the trigger
   onto the text does not close it. Hidden with `visibility` (no pointer events, no layout shift);
   aria-describedby still reads hidden text. */
.tooltip {
  position: absolute;
  top: 100%;
  left: 0;
  z-index: var(--z-menu);
  padding-top: var(--space-2);
  visibility: hidden;
}

.tooltip.open {
  visibility: visible;
}

.box {
  display: block;
  width: max-content;
  max-width: var(--tooltip-max-width);
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-md);
  font-size: var(--text-sm);
  line-height: var(--leading);
  color: var(--text);
  background: var(--surface-raised);
  box-shadow: var(--shadow-overlay);
  user-select: none;
}
</style>
