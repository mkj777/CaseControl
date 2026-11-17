<script setup lang="ts">
// Overflow menu (docs/design-system.md, "Menu button"): an IconButton with the "More" glyph that opens a ContextMenu
// under itself, right aligned to it, for the less common actions of a list row. `label` names the button for screen
// readers and the native tooltip. The menu has the ContextMenu's keyboard handling; Escape returns focus here.
import { ref } from 'vue'
import ContextMenu from './ContextMenu.vue'
import IconButton from './IconButton.vue'
import type { MenuItem } from './menu'

const props = withDefaults(defineProps<{ label: string; items: MenuItem[]; disabled?: boolean }>(), {
  disabled: false,
})

/** Segoe Fluent Icons "More" (horizontal ellipsis). */
const MORE = ''

const at = ref<{ x: number; y: number } | null>(null)
// A press on this button while the menu is open closes it (the menu listens on the window first); the click that
// follows must not open it again.
let closedAt = -1
let swallowClick = false

function onPointerDown() {
  swallowClick = performance.now() - closedAt < 50
}

function onClick(e: MouseEvent) {
  if (swallowClick) {
    swallowClick = false
    return
  }
  if (props.disabled || props.items.length === 0) return
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  const menuMin = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--menu-min-width')) || 0
  // Right edge of the menu under the right edge of the button; the menu keeps itself inside the window.
  at.value = { x: r.right - menuMin, y: r.bottom + 4 }
}

function onClose() {
  at.value = null
  closedAt = performance.now()
}
</script>

<template>
  <IconButton
    :label="label"
    :glyph="MORE"
    :disabled="disabled"
    aria-haspopup="menu"
    :aria-expanded="at !== null"
    @pointerdown="onPointerDown"
    @click="onClick"
  />
  <ContextMenu :at="at" :items="items" @close="onClose" />
</template>
