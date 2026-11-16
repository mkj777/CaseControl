<script setup lang="ts">
// The shell: sidebar navigation and the page area. The window has no Windows title bar (ADR 0001 #29): the app name
// sits at the top of the sidebar, the window controls at the top right of the page area, and that top strip drags
// the window (double-click toggles maximise; both are handled by Tauri's drag-region script). The shell also owns
// the start dialog for conflicting RGB programs and the hint shown while one of them still starts with Windows
// (ADR 0001 #31).
import { computed, onMounted } from 'vue'
import { RouterLink, RouterView, useRoute } from 'vue-router'
import StartupConflictsDialog from './components/StartupConflictsDialog.vue'
import WindowControls from './components/WindowControls.vue'
import { Button, Notice } from './components/ui'
import { conflictState, hintPrograms, openDialog, startConflictWatch } from './stores/conflicts'
import { listenForDeviceEvents, refreshDevices } from './stores/devices'

const nav = [
  { to: '/devices', label: 'Devices' },
  { to: '/profiles', label: 'Profiles' },
  { to: '/conflicts', label: 'Conflicts' },
  { to: '/settings', label: 'Settings' },
]

const route = useRoute()
const isActive = (to: string) => route.path === to || route.path.startsWith(`${to}/`)

onMounted(() => {
  listenForDeviceEvents()
  void refreshDevices()
  void startConflictWatch()
})

const hint = computed(() => (conflictState.dialogOpen ? [] : hintPrograms()))
const hintNames = computed(() => {
  const names = hint.value.map((p) => p.name)
  return names.length <= 1 ? names.join('') : `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`
})
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand" data-tauri-drag-region="deep">CaseControl</div>
      <nav class="nav">
        <RouterLink
          v-for="item in nav"
          :key="item.to"
          :to="item.to"
          class="nav-link"
          :class="{ active: isActive(item.to) }"
          :aria-current="isActive(item.to) ? 'page' : undefined"
        >
          {{ item.label }}
        </RouterLink>
      </nav>
    </aside>
    <div class="main">
      <header class="titlebar" data-tauri-drag-region="deep">
        <WindowControls />
      </header>
      <main class="content">
        <Notice v-if="hint.length > 0" variant="warning">
          {{ hintNames }} {{ hint.length === 1 ? 'starts' : 'start' }} with Windows and can take over your devices.
          <template #details>
            <div><Button size="sm" @click="openDialog">Review</Button></div>
          </template>
        </Notice>
        <RouterView />
      </main>
      <StartupConflictsDialog />
    </div>
  </div>
</template>

<style scoped>
.shell {
  display: grid;
  grid-template-columns: var(--sidebar-width) 1fr;
  height: 100vh;
}

.sidebar {
  display: flex;
  flex-direction: column;
  /* Brand strip to nav: the section gap of a group, so the name does not crowd the first item. */
  gap: var(--space-6);
  padding: 0 var(--space-3) var(--space-6);
  background: var(--surface);
  border-right: 1px solid var(--border);
}

/* The app name sits below the window's top edge with room above it (--space-8; it does not have to line up with the
   content's title strip), inset by --space-6 like the nav item text below (sidebar --space-3 + item --space-3).
   All sizes are tokens in px, so the proportions hold at every display scaling. */
.brand {
  display: flex;
  align-items: center;
  flex: none;
  min-height: var(--control-height);
  /* Reach the sidebar's edges so the whole block, including the space above, drags the window. */
  margin: 0 calc(-1 * var(--space-3));
  padding: var(--space-8) var(--space-6) 0;
  font-size: var(--text-lg);
  font-weight: var(--weight-strong);
}

.nav {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.nav-link {
  display: flex;
  align-items: center;
  height: var(--control-height);
  padding: 0 var(--space-3);
  border-radius: var(--radius-md);
  color: var(--text-muted);
  text-decoration: none;
  transition:
    color var(--transition),
    background-color var(--transition);
}

.nav-link:hover,
.nav-link.active {
  color: var(--text);
  background: var(--surface-raised);
}

.main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.titlebar {
  position: relative;
  z-index: var(--z-titlebar);
  display: flex;
  justify-content: flex-end;
  flex: none;
  height: var(--titlebar-height);
}

.content {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--page-pad-top) var(--page-pad-x) var(--space-10);
}
</style>
