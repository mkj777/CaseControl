<script setup lang="ts">
// The shell: sidebar navigation and the page area. The window has no Windows title bar (ADR 0001 #29): the app name
// sits at the top of the sidebar, the window controls at the top right of the page area, and that top strip drags
// the window (double-click toggles maximise; both are handled by Tauri's drag-region script).
import { onMounted } from 'vue'
import { RouterLink, RouterView, useRoute } from 'vue-router'
import WindowControls from './components/WindowControls.vue'
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
        <RouterView />
      </main>
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
  gap: var(--space-4);
  padding: 0 var(--space-4) var(--space-6);
  background: var(--surface);
  border-right: 1px solid var(--border);
}

.brand {
  display: flex;
  align-items: center;
  flex: none;
  height: var(--titlebar-height);
  /* Reach the sidebar's edges so the whole strip drags the window. */
  margin: 0 calc(-1 * var(--space-4));
  padding: 0 calc(var(--space-4) + var(--space-3));
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
