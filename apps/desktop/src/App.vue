<script setup lang="ts">
import { onMounted } from 'vue'
import { RouterLink, RouterView, useRoute } from 'vue-router'
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
      <div class="brand">CaseControl</div>
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
    <main class="content">
      <RouterView />
    </main>
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
  gap: var(--space-6);
  padding: var(--space-6) var(--space-4);
  background: var(--color-surface);
  border-right: 1px solid var(--color-border);
}

.brand {
  padding: 0 var(--space-3);
  font-size: var(--font-size-lg);
  font-weight: 600;
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

.content {
  overflow-y: auto;
  padding: var(--space-8) var(--space-10);
}
</style>
