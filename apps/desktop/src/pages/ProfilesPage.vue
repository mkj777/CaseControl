<script setup lang="ts">
// Profiles: what every device shows, saved under a name. The active profile is the one applied last; it is applied
// again when CaseControl starts (ADR 0001 #24).
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import {
  applyProfile,
  deleteProfile,
  errorText,
  listProfiles,
  on,
  renameProfile,
  saveProfile,
  type ApplyReport,
  type ProfileList,
} from '../ipc'
import { deviceState } from '../stores/devices'

const list = ref<ProfileList>({ names: [], active: null })
const loaded = ref(false)
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const busy = ref(false)

const sameName = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase()
const isActive = (name: string) => list.value.active !== null && sameName(list.value.active, name)

async function run<T>(f: () => Promise<T>): Promise<T | undefined> {
  busy.value = true
  error.value = null
  message.value = null
  try {
    return await f()
  } catch (e) {
    error.value = errorText(e)
    return undefined
  } finally {
    busy.value = false
  }
}

async function load() {
  const l = await run(listProfiles)
  if (l) list.value = l
  loaded.value = true
}

let unlisten: (() => void) | null = null
onMounted(async () => {
  await load()
  unlisten = await on('profiles-changed', (l) => (list.value = l))
})
onBeforeUnmount(() => unlisten?.())

// ---- save current as ---------------------------------------------------------------------------------------------

const newName = ref('')
const overwriteName = ref<string | null>(null)
const hasDevices = computed(() => deviceState.devices.length > 0)

function onSaveAs() {
  const name = newName.value.trim()
  if (!name) return
  const existing = list.value.names.find((n) => sameName(n, name))
  if (existing) overwriteName.value = existing
  else void doSave(name)
}

async function doSave(name: string) {
  overwriteName.value = null
  const l = await run(() => saveProfile(name))
  if (l) {
    list.value = l
    newName.value = ''
    message.value = `Saved "${name}".`
  }
}

// ---- apply -------------------------------------------------------------------------------------------------------

function describe(name: string, r: ApplyReport): string {
  const parts = [`Applied "${name}".`]
  if (r.missing.length > 0) {
    parts.push(
      r.missing.length === 1
        ? '1 device in it is not connected; it gets its setting when it appears.'
        : `${r.missing.length} devices in it are not connected; they get their settings when they appear.`,
    )
  }
  for (const f of r.failed) parts.push(`${f.id}: ${f.error}`)
  return parts.join(' ')
}

async function onApply(name: string) {
  const r = await run(() => applyProfile(name))
  if (r) message.value = describe(name, r)
}

// ---- rename ------------------------------------------------------------------------------------------------------

const renaming = ref<string | null>(null)
const renameTo = ref('')

function startRename(name: string) {
  renaming.value = name
  renameTo.value = name
}

async function onRename() {
  const from = renaming.value
  const to = renameTo.value.trim()
  if (!from || !to) return
  if (to === from) {
    renaming.value = null
    return
  }
  const l = await run(() => renameProfile(from, to))
  if (l) {
    list.value = l
    renaming.value = null
  }
}

// ---- delete ------------------------------------------------------------------------------------------------------

const deleting = ref<string | null>(null)

async function onDelete() {
  const name = deleting.value
  if (!name) return
  const l = await run(() => deleteProfile(name))
  deleting.value = null
  if (l) list.value = l
}
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <h1>Profiles</h1>
        <p class="page-description">
          A profile remembers what every device shows. The active profile is applied again when CaseControl starts.
        </p>
      </div>
    </header>

    <form class="save row" @submit.prevent="onSaveAs">
      <input v-model="newName" type="text" placeholder="Profile name" maxlength="64" aria-label="Profile name" />
      <button type="submit" class="primary" :disabled="busy || !newName.trim() || !hasDevices">Save current as…</button>
      <span v-if="!hasDevices" class="muted small">Connect a device first.</span>
    </form>

    <p v-if="error" class="notice">{{ error }}</p>
    <p v-if="message" class="notice">{{ message }}</p>

    <ul v-if="list.names.length > 0" class="list">
      <li v-for="name in list.names" :key="name" class="item" :class="{ active: isActive(name) }">
        <template v-if="renaming === name">
          <form class="row grow" @submit.prevent="onRename">
            <input v-model="renameTo" type="text" maxlength="64" aria-label="New name" />
            <button type="submit" class="primary" :disabled="busy || !renameTo.trim()">Rename</button>
            <button type="button" :disabled="busy" @click="renaming = null">Cancel</button>
          </form>
        </template>
        <template v-else>
          <div class="grow">
            <span class="name">{{ name }}</span>
            <span v-if="isActive(name)" class="marker small">Active</span>
          </div>
          <div class="row">
            <button type="button" :disabled="busy" @click="onApply(name)">Apply</button>
            <button type="button" class="link" :disabled="busy" @click="startRename(name)">Rename</button>
            <button type="button" class="link" :disabled="busy" @click="deleting = name">Delete</button>
          </div>
        </template>
      </li>
    </ul>
    <p v-else-if="loaded" class="empty muted">
      No profiles yet. Set up your devices, then save the result here under a name.
    </p>

    <ConfirmDialog
      :open="deleting !== null"
      title="Delete profile?"
      confirm-label="Delete"
      :busy="busy"
      @confirm="onDelete"
      @cancel="deleting = null"
    >
      <p>"{{ deleting }}" will be deleted. Devices keep showing what they show now.</p>
    </ConfirmDialog>

    <ConfirmDialog
      :open="overwriteName !== null"
      title="Replace profile?"
      confirm-label="Replace"
      :busy="busy"
      @confirm="overwriteName && doSave(overwriteName)"
      @cancel="overwriteName = null"
    >
      <p>A profile named "{{ overwriteName }}" exists. It will be replaced with what the devices show now.</p>
    </ConfirmDialog>
  </section>
</template>

<style scoped>
.save {
  margin-bottom: var(--space-8);
}

.save input {
  width: 280px;
}

.notice {
  margin-bottom: var(--space-6);
}

.list {
  display: flex;
  flex-direction: column;
  margin: 0;
  padding: 0;
  list-style: none;
  border-top: 1px solid var(--color-border);
}

.item {
  display: flex;
  align-items: center;
  gap: var(--space-6);
  min-height: 72px;
  padding: var(--space-4) var(--space-2);
  border-bottom: 1px solid var(--color-border);
}

.grow {
  flex: 1;
  display: flex;
  align-items: center;
  gap: var(--space-4);
}

.name {
  font-size: var(--font-size-lg);
}

.marker {
  padding: 2px var(--space-2);
  border: 1px solid var(--color-text);
  border-radius: 999px;
}

.row .link {
  margin-left: var(--space-2);
}

.empty {
  padding: var(--space-8) 0;
}
</style>
