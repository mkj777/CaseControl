<script setup lang="ts">
// Minimise, maximise/restore and close for the undecorated main window (ADR 0001 #29). Close hides to the tray,
// like the Windows close button did. Segoe Fluent glyphs, grey hover (no red, ADR 0001 #23).
import { getCurrentWindow } from '@tauri-apps/api/window'
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { hideToTray } from '../ipc'
import { IconButton } from './ui'

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

// Segoe Fluent Icons code points (docs/design-system.md, "App shell").
const GLYPH = { minimise: '', maximise: '', restore: '', close: '' }
</script>

<template>
  <div class="controls">
    <IconButton window label="Minimise" :glyph="GLYPH.minimise" @click="minimise" />
    <IconButton
      window
      :label="maximized ? 'Restore' : 'Maximise'"
      :glyph="maximized ? GLYPH.restore : GLYPH.maximise"
      @click="toggleMaximise"
    />
    <IconButton window label="Close" :glyph="GLYPH.close" @click="close" />
  </div>
</template>

<style scoped>
.controls {
  display: flex;
  height: 100%;
}
</style>
