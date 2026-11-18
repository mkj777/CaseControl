<script setup lang="ts">
// In-app context menu at the pointer (docs/design-system.md, "Context menu"; ADR 0001 #26, #27); the browser's own
// menu is suppressed app-wide. Opens clamped inside the window with focus on the first item; arrow keys, Home and
// End move, Enter activates; Escape, a click elsewhere, scroll, resize or leaving the window close it. Focus returns
// to the element that opened it. One level, no submenus. Use `menuPoint(event)` for `at` so Shift+F10 and the
// Menu key open it at the focused element.
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import type { MenuItem } from './menu'

const props = withDefaults(
  defineProps<{
    /** Where to open (client coordinates), or `null` when closed. */
    at: { x: number; y: number } | null
    items: MenuItem[]
    /** Opened from inside a dialog: stack above it. */
    inDialog?: boolean
  }>(),
  { inDialog: false },
)
const emit = defineEmits<{ close: [] }>()

const menu = ref<HTMLElement | null>(null)
const pos = ref({ x: 0, y: 0 })
let opener: HTMLElement | null = null

function close() {
  emit('close')
}

function itemButtons(): HTMLButtonElement[] {
  return menu.value ? [...menu.value.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')] : []
}

function onPointerDown(e: PointerEvent) {
  if (menu.value && !menu.value.contains(e.target as Node)) close()
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    close()
    return
  }
  const items = itemButtons()
  if (items.length === 0) return
  const i = items.indexOf(document.activeElement as HTMLButtonElement)
  let next: number | null = null
  if (e.key === 'ArrowDown') next = i < 0 ? 0 : (i + 1) % items.length
  else if (e.key === 'ArrowUp') next = i < 0 ? items.length - 1 : (i - 1 + items.length) % items.length
  else if (e.key === 'Home') next = 0
  else if (e.key === 'End') next = items.length - 1
  else if (e.key === 'Tab') {
    e.preventDefault()
    close()
    return
  }
  if (next === null) return
  e.preventDefault()
  items[next]?.focus()
}

function listen(on: boolean) {
  const method = on ? 'addEventListener' : 'removeEventListener'
  window[method]('pointerdown', onPointerDown as EventListener, true)
  window[method]('keydown', onKey as EventListener, true)
  window[method]('scroll', close, true)
  window[method]('resize', close)
  window[method]('blur', close)
}

watch(
  () => props.at,
  async (at, before) => {
    listen(false)
    if (!at) {
      if (before) restoreFocus()
      return
    }
    if (!before) opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
    pos.value = { ...at }
    await nextTick()
    const el = menu.value
    if (!el) return
    // Keep the menu inside the window.
    const r = el.getBoundingClientRect()
    pos.value = {
      x: Math.max(4, Math.min(at.x, window.innerWidth - r.width - 4)),
      y: Math.max(4, Math.min(at.y, window.innerHeight - r.height - 4)),
    }
    itemButtons()[0]?.focus()
    listen(true)
  },
)

function restoreFocus() {
  const el = opener
  opener = null
  // Only take focus back if it is still in the menu or nowhere; a click elsewhere keeps its own focus.
  const active = document.activeElement
  if (el?.isConnected && (active === null || active === document.body || menu.value?.contains(active))) el.focus()
}

onBeforeUnmount(() => listen(false))

function run(item: MenuItem) {
  close()
  item.action()
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="at"
      ref="menu"
      class="menu"
      :class="{ 'in-dialog': inDialog }"
      role="menu"
      :style="{ left: `${pos.x}px`, top: `${pos.y}px` }"
      @contextmenu.prevent
    >
      <button
        v-for="item in items"
        :key="item.label"
        type="button"
        role="menuitem"
        class="item"
        tabindex="-1"
        :disabled="item.disabled"
        @click="run(item)"
      >
        {{ item.label }}
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.menu {
  position: fixed;
  z-index: var(--z-menu);
  display: flex;
  flex-direction: column;
  min-width: var(--menu-min-width);
  padding: var(--space-1);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-md);
  background: var(--surface);
  box-shadow: var(--shadow-overlay);
}

.menu.in-dialog {
  z-index: var(--z-dialog-menu);
}

.item {
  justify-content: flex-start;
  height: var(--control-height-sm);
  padding: 0 var(--space-3);
  border: 0;
  border-radius: var(--radius-sm);
  font-size: var(--text-md);
  text-align: left;
  background: none;
}

.item:hover:not(:disabled),
.item:focus-visible {
  outline: none;
  background: var(--surface-hover);
}

.item:disabled {
  color: var(--text-disabled);
}
</style>
