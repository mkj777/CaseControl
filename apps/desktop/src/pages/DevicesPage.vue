<script setup lang="ts">
// Devices overview (ADR 0001 #18): one large tile per device with a small live preview; a tile opens the device
// view.
import { RouterLink } from 'vue-router'
import DevicePreview from '../components/DevicePreview.vue'
import { STATUS_LABEL, deviceErrorText, familyLabel, kindLabel, statusOf } from '../lib/device'
import { deviceState, rescanDevices } from '../stores/devices'

const deviceLink = (id: string) => ({ name: 'device', params: { id } })
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <h1>Devices</h1>
        <p class="page-description">Choose a device to set its colours and effects.</p>
      </div>
      <button type="button" :disabled="deviceState.scanning" @click="rescanDevices">
        {{ deviceState.scanning ? 'Scanning…' : 'Rescan' }}
      </button>
    </header>

    <div v-if="deviceState.engineError" class="notice">
      <p>{{ deviceState.engineError }}</p>
      <p class="muted small">Restart CaseControl to try again. Details are in the log file.</p>
    </div>
    <div v-if="deviceState.error" class="notice">
      <p>Could not load the device list: {{ deviceState.error }}</p>
    </div>

    <div v-if="deviceState.devices.length > 0" class="grid">
      <RouterLink
        v-for="d in deviceState.devices"
        :key="d.description.info.id"
        :to="deviceLink(d.description.info.id)"
        class="tile"
      >
        <div class="tile-head">
          <div>
            <div class="name">{{ d.description.info.name }}</div>
            <div class="muted small">{{ kindLabel(d.description.info.kind) }} · {{ d.description.info.vendor }}</div>
          </div>
          <span class="status small" :class="statusOf(d)">{{ STATUS_LABEL[statusOf(d)] }}</span>
        </div>
        <div class="tile-preview">
          <DevicePreview :description="d.description" :colors="d.colors" />
        </div>
      </RouterLink>
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
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: var(--space-6);
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

.tile:hover {
  border-color: var(--color-border-strong);
  background: var(--color-surface-raised);
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
