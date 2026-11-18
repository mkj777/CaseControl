<script setup lang="ts">
// The profile dropdown of the Lighting toolbar (ADR 0001 #40). There is always exactly one active profile and every
// device change is saved into it, so there is no Save. The button shows the active profile's name; its menu lists the
// profiles with the active one checked (choosing another switches to it), then New profile, Duplicate, Rename and
// Delete for the active profile. Rename opens a small dialog; Delete confirms first and is disabled for the last
// profile. The menu is the ContextMenu, opened under the button (docs/design-system.md, "Select and menu").
import { computed, onBeforeUnmount, onMounted, ref, useId } from 'vue'
import { Button, ConfirmDialog, ContextMenu, Dialog, Notice, TextField, type MenuItem } from './ui'
import {
  applyProfile,
  createProfile,
  deleteProfile,
  duplicateProfile,
  errorText,
  listProfiles,
  on,
  renameProfile,
  type ProfileList,
} from '../ipc'

withDefaults(defineProps<{ disabled?: boolean }>(), { disabled: false })
/** A failed profile action (`null` clears it when the next one starts). */
const emit = defineEmits<{ error: [message: string | null] }>()

const list = ref<ProfileList>({ names: [], active: null })
const busy = ref(false)

const sameName = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase()
const active = computed(() => list.value.active)
const onlyOne = computed(() => list.value.names.length <= 1)

let unlisten: (() => void) | null = null
onMounted(async () => {
  unlisten = await on('profiles-changed', (l) => (list.value = l))
  try {
    list.value = await listProfiles()
  } catch (e) {
    emit('error', errorText(e))
  }
})
onBeforeUnmount(() => unlisten?.())

/** Runs a profile action; the list comes back from it or with `profiles-changed`. */
async function run(f: () => Promise<ProfileList | void>): Promise<boolean> {
  busy.value = true
  emit('error', null)
  try {
    const l = await f()
    if (l) list.value = l
    return true
  } catch (e) {
    emit('error', errorText(e))
    return false
  } finally {
    busy.value = false
  }
}

async function switchTo(name: string) {
  if (active.value !== null && sameName(active.value, name)) return
  await run(async () => {
    const r = await applyProfile(name)
    if (r.failed.length > 0) {
      throw new Error(`Some devices could not be set. ${r.failed.map((f) => `${f.id}: ${f.error}`).join('; ')}`)
    }
  })
}

// ---- menu --------------------------------------------------------------------------------------------------------

const at = ref<{ x: number; y: number } | null>(null)
// As in MenuButton: a press on the button while the menu is open closes it (the menu listens on the window first);
// the click that follows must not open it again.
let closedAt = -1
let swallowClick = false

function onPointerDown() {
  swallowClick = performance.now() - closedAt < 50
}

function onClick(e: MouseEvent) {
  if (swallowClick) {
    swallowClick = false
    return
  }
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  at.value = { x: r.left, y: r.bottom + 4 }
}

function onClose() {
  at.value = null
  closedAt = performance.now()
}

/** Segoe Fluent Icons "ChevronDown". */
const CHEVRON = ''

const LAST_PROFILE = 'The last profile cannot be deleted.'

const items = computed<MenuItem[]>(() => [
  ...list.value.names.map((name) => ({
    label: name,
    checked: active.value !== null && sameName(active.value, name),
    action: () => void switchTo(name),
  })),
  { label: 'New profile', separator: true, action: () => void run(createProfile) },
  {
    label: 'Duplicate',
    disabled: active.value === null,
    action: () => {
      const name = active.value
      if (name !== null) void run(() => duplicateProfile(name))
    },
  },
  { label: 'Rename', disabled: active.value === null, action: startRename },
  {
    label: 'Delete',
    disabled: active.value === null || onlyOne.value,
    hint: onlyOne.value ? LAST_PROFILE : undefined,
    action: () => (deleting.value = active.value),
  },
])

// ---- rename ------------------------------------------------------------------------------------------------------

const formId = useId()
const renaming = ref<string | null>(null)
const renameTo = ref('')
const renameError = ref<string | null>(null)

function startRename() {
  renaming.value = active.value
  renameTo.value = active.value ?? ''
  renameError.value = null
}

async function onRename() {
  const from = renaming.value
  const to = renameTo.value.trim()
  if (!from || !to) return
  if (to === from) {
    renaming.value = null
    return
  }
  busy.value = true
  renameError.value = null
  try {
    list.value = await renameProfile(from, to)
    renaming.value = null
  } catch (e) {
    renameError.value = errorText(e)
  } finally {
    busy.value = false
  }
}

// ---- delete ------------------------------------------------------------------------------------------------------

const deleting = ref<string | null>(null)
/** The profile that becomes active when the active one is deleted: the first other one in list order. */
const nextActive = computed(() => list.value.names.find((n) => deleting.value === null || !sameName(n, deleting.value)))

async function onDelete() {
  const name = deleting.value
  if (!name) return
  await run(() => deleteProfile(name))
  deleting.value = null
}
</script>

<template>
  <Button
    class="profile-button"
    :disabled="disabled || busy"
    aria-haspopup="menu"
    :aria-expanded="at !== null"
    :aria-label="`Profile: ${active ?? 'none'}`"
    :title="active ?? undefined"
    @pointerdown="onPointerDown"
    @click="onClick"
  >
    <span class="name">{{ active ?? 'Profile' }}</span>
    <span class="chevron" aria-hidden="true">{{ CHEVRON }}</span>
  </Button>
  <ContextMenu :at="at" :items="items" @close="onClose" />

  <Dialog :open="renaming !== null" title="Rename profile" :busy="busy" @close="renaming = null">
    <form :id="formId" class="rename" @submit.prevent="onRename">
      <TextField
        v-model="renameTo"
        data-autofocus
        maxlength="64"
        aria-label="Profile name"
        placeholder="Profile name"
        :invalid="renameError !== null"
      />
    </form>
    <Notice v-if="renameError" variant="error">{{ renameError }}</Notice>
    <template #actions>
      <Button :disabled="busy" @click="renaming = null">Cancel</Button>
      <Button type="submit" variant="primary" :form="formId" :disabled="busy || !renameTo.trim()">Rename</Button>
    </template>
  </Dialog>

  <ConfirmDialog
    :open="deleting !== null"
    title="Delete profile?"
    confirm-label="Delete"
    danger
    :busy="busy"
    @confirm="onDelete"
    @cancel="deleting = null"
  >
    <p>"{{ deleting }}" will be deleted. CaseControl switches to "{{ nextActive }}".</p>
  </ConfirmDialog>
</template>

<style scoped>
.profile-button {
  gap: var(--space-2);
  max-width: var(--toolbar-control-width);
}

.name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.chevron {
  flex: none;
  font-family: var(--font-icon);
  font-size: 10px;
  color: var(--text-muted);
}

.rename {
  display: flex;
  flex-direction: column;
}

.rename :deep(.text-field) {
  width: 100%;
}
</style>
