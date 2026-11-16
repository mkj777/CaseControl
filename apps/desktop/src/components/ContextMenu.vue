<script setup lang="ts">
// A small in-app context menu at the pointer (ADR 0001 #26, #27); the browser's own menu is suppressed app-wide.
// Closes on Escape, on a click elsewhere, on scroll, on resize and when the window loses focus.
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'

export interface MenuItem {
  label: string
  action: () => void
}

const props = defineProps<{
  /** Where to open (client coordinates), or `null` when closed. */
  at: { x: number; y: number } | null
  items: MenuItem[]
}>()
const emit = defineEmits<{ close: [] }>()

const menu = ref<HTMLElement | null>(null)
const pos = ref({ x: 0, y: 0 })

function close() {
  emit('close')
}

function onPointerDown(e: PointerEvent) {
  if (menu.value && !menu.value.contains(e.target as Node)) close()
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault()
    close()
  }
}

function listen(on: boolean) {
  if (on) {
    window.addEventListener('pointerdown', onPointerDown, true)
    window.addEventListener('keydown', onKey, true)
    window.addEventListener('scroll', close, true)
    window.addEventListener('resize', close)
    window.addEventListener('blur', close)
  } else {
    window.removeEventListener('pointerdown', onPointerDown, true)
    window.removeEventListener('keydown', onKey, true)
    window.removeEventListener('scroll', close, true)
    window.removeEventListener('resize', close)
    window.removeEventListener('blur', close)
  }
}

watch(
  () => props.at,
  async (at) => {
    listen(false)
    if (!at) return
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
    el.querySelector<HTMLButtonElement>('button')?.focus()
    listen(true)
  },
)

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
      role="menu"
      :style="{ left: `${pos.x}px`, top: `${pos.y}px` }"
      @contextmenu.prevent
    >
      <button v-for="item in items" :key="item.label" type="button" role="menuitem" @click="run(item)">
        {{ item.label }}
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.menu {
  position: fixed;
  z-index: 20;
  display: flex;
  flex-direction: column;
  min-width: 160px;
  padding: var(--space-1);
  border: 1px solid var(--color-border-strong);
  border-radius: var(--radius);
  background: var(--color-surface-raised);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
}

.menu button {
  padding: var(--space-2) var(--space-3);
  border: 0;
  border-radius: 6px;
  text-align: left;
  background: none;
}

.menu button:hover,
.menu button:focus-visible {
  outline: none;
  background: var(--color-surface-hover);
}
</style>
