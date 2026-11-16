<script setup lang="ts">
// Profiles: what every device shows, saved under a name. The active profile is the one applied last; it is applied
// again when CaseControl starts (ADR 0001 #24).
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import {
  Button,
  ConfirmDialog,
  EmptyState,
  ListRow,
  Notice,
  PageHeader,
  StatusBadge,
  TextField,
} from '../components/ui'
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
    <PageHeader
      title="Profiles"
      description="A profile remembers what every device shows. The active profile is applied again when CaseControl starts."
    />

    <form class="save row" @submit.prevent="onSaveAs">
      <TextField v-model="newName" class="name-input" placeholder="Profile name" maxlength="64" aria-label="Profile name" />
      <Button type="submit" variant="primary" :disabled="busy || !newName.trim() || !hasDevices">Save current as…</Button>
      <span v-if="!hasDevices" class="muted small">Connect a device first.</span>
    </form>

    <Notice v-if="error" variant="error">{{ error }}</Notice>
    <Notice v-if="message">{{ message }}</Notice>

    <ul v-if="list.names.length > 0">
      <template v-for="name in list.names" :key="name">
        <ListRow v-if="renaming === name">
          <form class="row rename" @submit.prevent="onRename">
            <TextField v-model="renameTo" class="name-input" maxlength="64" aria-label="New name" />
            <Button type="submit" size="sm" :disabled="busy || !renameTo.trim()">Rename</Button>
            <Button variant="ghost" size="sm" :disabled="busy" @click="renaming = null">Cancel</Button>
          </form>
        </ListRow>
        <ListRow v-else :name="name">
          <StatusBadge v-if="isActive(name)" status="current">Active</StatusBadge>
          <template #actions>
            <Button size="sm" :disabled="busy" @click="onApply(name)">Apply</Button>
            <Button variant="ghost" size="sm" :disabled="busy" @click="startRename(name)">Rename</Button>
            <Button variant="danger" size="sm" :disabled="busy" @click="deleting = name">Delete</Button>
          </template>
        </ListRow>
      </template>
    </ul>
    <EmptyState v-else-if="loaded" title="No profiles yet">
      Set up your devices, then save the result here under a name.
    </EmptyState>

    <ConfirmDialog
      :open="deleting !== null"
      title="Delete profile?"
      confirm-label="Delete"
      danger
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
  margin-bottom: var(--space-10);
}

.name-input {
  width: var(--control-max-width);
}

.rename {
  flex: 1;
}
</style>
