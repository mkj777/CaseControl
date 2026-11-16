<script setup lang="ts">
// Devices overview (ADR 0001 #18): one large tile per device with a small live preview; a tile opens the device
// view. Tiles fill the window width (ADR 0001 #28). Right-click a tile to hide or unhide the device; hidden devices
// are shown greyed only with "Show hidden" on (ADR 0001 #26, #27).
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import ContextMenu, { type MenuItem } from '../components/ContextMenu.vue'
import DevicePreview from '../components/DevicePreview.vue'
import { errorText, type DeviceView } from '../ipc'
import { STATUS_LABEL, deviceErrorText, familyLabel, kindLabel, statusOf, zoneIndicesNamed } from '../lib/device'
import { deviceState, hiddenZoneNames, hideDevice, isDeviceHidden, rescanDevices } from '../stores/devices'

const deviceLink = (id: string) => ({ name: 'device', params: { id } })

const tiles = computed(() =>
  deviceState.devices
    .map((d) => ({ d, id: d.description.info.id, hidden: isDeviceHidden(d.description.info.id) }))
    .filter((t) => deviceState.showHiddenDevices || !t.hidden),
)
const allHidden = computed(() => deviceState.devices.length > 0 && tiles.value.length === 0)
const hiddenZonesOf = (d: DeviceView) => zoneIndicesNamed(d.description, hiddenZoneNames(d.description.info.id))

const error = ref<string | null>(null)
const menuAt = ref<{ x: number; y: number } | null>(null)
const menuItems = ref<MenuItem[]>([])

function openMenu(e: MouseEvent, id: string, hidden: boolean) {
  e.preventDefault()
  menuItems.value = [
    {
      label: hidden ? 'Unhide device' : 'Hide device',
      action: () => void toggleHidden(id, !hidden),
    },
  ]
  menuAt.value = { x: e.clientX, y: e.clientY }
}

async function toggleHidden(id: string, hidden: boolean) {
  error.value = null
  try {
    await hideDevice(id, hidden)
  } catch (e) {
    error.value = errorText(e)
  }
}
</script>

<template>
  <section class="page wide">
    <header class="page-header">
      <div>
        <h1>Devices</h1>
        <p class="page-description">Choose a device to set its colours and effects.</p>
      </div>
      <div class="row actions">
        <label class="row toggle">
          <input v-model="deviceState.showHiddenDevices" type="checkbox" />
          <span>Show hidden</span>
        </label>
        <button type="button" :disabled="deviceState.scanning" @click="rescanDevices">
          {{ deviceState.scanning ? 'Scanning…' : 'Rescan' }}
        </button>
      </div>
    </header>

    <div v-if="deviceState.engineError" class="notice">
      <p>{{ deviceState.engineError }}</p>
      <p class="muted small">Restart CaseControl to try again. Details are in the log file.</p>
    </div>
    <div v-if="deviceState.error" class="notice">
      <p>Could not load the device list: {{ deviceState.error }}</p>
    </div>

    <div v-if="error" class="notice">
      <p>{{ error }}</p>
    </div>

    <div v-if="tiles.length > 0" class="grid">
      <component
        :is="t.hidden ? 'div' : RouterLink"
        v-for="t in tiles"
        :key="t.id"
        v-bind="t.hidden ? {} : { to: deviceLink(t.id) }"
        class="tile"
        :class="{ hidden: t.hidden }"
        :aria-disabled="t.hidden ? 'true' : undefined"
        @contextmenu="openMenu($event, t.id, t.hidden)"
      >
        <div class="tile-head">
          <div>
            <div class="name">{{ t.d.description.info.name }}</div>
            <div class="muted small">{{ kindLabel(t.d.description.info.kind) }} · {{ t.d.description.info.vendor }}</div>
          </div>
          <span v-if="t.hidden" class="status small">Hidden</span>
          <span v-else class="status small" :class="statusOf(t.d)">{{ STATUS_LABEL[statusOf(t.d)] }}</span>
        </div>
        <div class="tile-preview">
          <DevicePreview :description="t.d.description" :colors="t.d.colors" :hidden-zones="hiddenZonesOf(t.d)" />
        </div>
      </component>
    </div>

    <div v-else-if="allHidden" class="empty">
      <h2>All devices are hidden</h2>
      <p class="muted">Turn on Show hidden to see them. Right-click a device to unhide it.</p>
    </div>

    <div v-else-if="deviceState.loaded && !deviceState.engineError" class="empty">
      <h2>No devices detected</h2>
      <p class="muted">
        Other RGB software can hold a device so CaseControl cannot reach it. Close it and rescan, or see
        <RouterLink to="/conflicts">Conflicts</RouterLink> for what is running.
      </p>
      <button type="button" class="primary" :disabled="deviceState.scanning" @click="rescanDevices">
        {{ deviceState.scanning ? 'Scanning…' : 'Rescan' }}
      </button>
    </div>

    <ContextMenu :at="menuAt" :items="menuItems" @close="menuAt = null" />

    <div v-if="deviceState.failures.length > 0" class="section">
      <h2>Found but not started</h2>
      <ul class="failures">
        <li v-for="(f, i) in deviceState.failures" :key="i">
          <span>{{ familyLabel(f.family) }}</span>
          <span class="muted">{{ deviceErrorText(f.error) }}</span>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: var(--space-6);
}

.actions {
  gap: var(--space-5);
}

.toggle {
  gap: var(--space-2);
  color: var(--color-text-muted);
  cursor: pointer;
}

.notice {
  margin-bottom: var(--space-8);
}

.tile {
  display: flex;
  flex-direction: column;
  gap: var(--space-6);
  min-height: 220px;
  padding: var(--space-6);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-lg);
  color: inherit;
  text-decoration: none;
  background: var(--color-surface);
  transition:
    border-color var(--transition),
    background var(--transition);
}

.tile:not(.hidden):hover {
  border-color: var(--color-border-strong);
  background: var(--color-surface-raised);
}

.tile.hidden {
  opacity: 0.45;
  cursor: default;
}

.tile-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: var(--space-4);
}

.name {
  font-size: var(--font-size-lg);
  font-weight: 600;
}

.status {
  flex: none;
  padding: 2px var(--space-2);
  border: 1px solid var(--color-border-strong);
  border-radius: 999px;
  color: var(--color-text-muted);
}

.status.active {
  color: var(--color-text);
}

.status.faulted {
  border-color: var(--color-text);
  color: var(--color-text);
}

.tile-preview {
  margin-top: auto;
}

.empty {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: var(--space-4);
  max-width: 56ch;
  padding: var(--space-10) 0;
}

.empty h2 {
  margin: 0;
}

.failures {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin: 0;
  padding: 0;
  list-style: none;
}

.failures li {
  display: flex;
  gap: var(--space-4);
}
</style>
