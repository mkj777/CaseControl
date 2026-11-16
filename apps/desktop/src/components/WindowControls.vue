<script setup lang="ts">
// Minimise, maximise/restore and close for the undecorated main window (ADR 0001 #29). Close hides to the tray,
// like the Windows close button did. Plain line glyphs, grey hover (no red, ADR 0001 #23).
import { getCurrentWindow } from '@tauri-apps/api/window'
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { hideToTray } from '../ipc'

const win = getCurrentWindow()
const maximized = ref(false)
let unlisten: (() => void) | null = null
let disposed = false

function report(e: unknown) {
  console.warn('window control failed', e)
}

async function syncMaximized() {
  try {
    maximized.value = await win.isMaximized()
  } catch (e) {
    report(e)
  }
}

onMounted(async () => {
  await syncMaximized()
  try {
    const off = await win.onResized(() => void syncMaximized())
    if (disposed) off()
    else unlisten = off
  } catch (e) {
    report(e)
  }
})

onBeforeUnmount(() => {
  disposed = true
  unlisten?.()
})

const minimise = () => win.minimize().catch(report)
const toggleMaximise = () => win.toggleMaximize().catch(report)
const close = () => hideToTray().catch(report)
</script>

<template>
  <div class="controls">
    <button type="button" aria-label="Minimise" title="Minimise" @click="minimise">
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5.5h10" /></svg>
    </button>
    <button
      type="button"
      :aria-label="maximized ? 'Restore' : 'Maximise'"
      :title="maximized ? 'Restore' : 'Maximise'"
      @click="toggleMaximise"
    >
      <svg v-if="maximized" viewBox="0 0 10 10" aria-hidden="true">
        <path d="M2.5 2.5V0.5h7v7h-2" />
        <rect x="0.5" y="2.5" width="7" height="7" />
      </svg>
      <svg v-else viewBox="0 0 10 10" aria-hidden="true"><rect x="0.5" y="0.5" width="9" height="9" /></svg>
    </button>
    <button type="button" aria-label="Close" title="Close" @click="close">
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0.5 0.5l9 9M9.5 0.5l-9 9" /></svg>
    </button>
  </div>
</template>

<style scoped>
.controls {
  display: flex;
  height: 100%;
}

.controls button {
  display: grid;
  place-items: center;
  width: 46px;
  height: 100%;
  padding: 0;
  border: 0;
  border-radius: 0;
  color: var(--color-text-muted);
  background: transparent;
}

.controls button:hover:not(:disabled) {
  border: 0;
  color: var(--color-text);
  background: var(--color-surface-hover);
}

.controls button:active {
  background: var(--color-surface-raised);
}

.controls button:focus-visible {
  outline-offset: -2px;
}

svg {
  width: 10px;
  height: 10px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1;
}
</style>
