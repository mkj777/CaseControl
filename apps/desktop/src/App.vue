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
  background: var(--color-surface);
  border-right: 1px solid var(--color-border);
}

.brand {
  display: flex;
  align-items: center;
  flex: none;
  height: var(--titlebar-height);
  /* Reach the sidebar's edges so the whole strip drags the window. */
  margin: 0 calc(-1 * var(--space-4));
  padding: 0 calc(var(--space-4) + var(--space-3));
  font-size: var(--font-size-lg);
  font-weight: 600;
  user-select: none;
  cursor: default;
}

.nav {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.nav-link {
  padding: var(--space-3);
  border-radius: var(--radius);
  color: var(--color-text-muted);
  text-decoration: none;
  transition:
    color var(--transition),
    background var(--transition);
}

.nav-link:hover,
.nav-link.active {
  color: var(--color-text);
  background: var(--color-surface-raised);
}

.main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.titlebar {
  display: flex;
  justify-content: flex-end;
  flex: none;
  height: var(--titlebar-height);
  user-select: none;
}

.content {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--space-4) var(--space-10) var(--space-8);
}
</style>
