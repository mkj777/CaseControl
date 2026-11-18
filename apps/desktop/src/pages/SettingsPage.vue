<script setup lang="ts">
// Settings: startup, update rate, device detectors and information about this build. Every change is saved and
// applied right away. The fallback colour of a takeover lives in the Lighting toolbar, and the OpenRGB port check has
// no switch here; it keeps its stored value and default (ADR 0001 #37).
//
// App updates (ADR 0001 #41) sit in About, next to the version, without a heading of their own: the updater's status
// line (read on mount, then followed through the `update-status` event; the view with the highest `seq` wins),
// "Check for updates" (checks and downloads now) and "Update now" (only while a downloaded update is pending:
// releases the devices, installs it and restarts the app). Debug builds have no updates; both buttons are disabled
// there and the check button shows the backend's reason in a tooltip.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import {
  appInfo,
  checkForUpdates,
  errorText,
  getSettings,
  installUpdate,
  on,
  setSettings,
  updateStatus,
  type AppInfo,
  type SettingsPatch,
  type SettingsView,
  type UpdateView,
} from '../ipc'
import {
  Button,
  Field,
  KeyValueList,
  Notice,
  NumberField,
  PageHeader,
  Slider,
  Switch,
  Tooltip,
} from '../components/ui'
import { familyLabel } from '../lib/device'

const settings = ref<SettingsView | null>(null)
const info = ref<AppInfo | null>(null)
const error = ref<string | null>(null)
const saving = ref(false)
const frameRate = ref(30)
/** Controls are off while saving, or for the whole session when settings cannot be saved. */
const locked = computed(() => saving.value || settings.value?.save_error != null)

onMounted(async () => {
  try {
    const [s, i] = await Promise.all([getSettings(), appInfo()])
    settings.value = s
    frameRate.value = s.frame_rate
    info.value = i
  } catch (e) {
    error.value = errorText(e)
  }
})

async function patch(p: SettingsPatch) {
  saving.value = true
  error.value = null
  try {
    settings.value = await setSettings(p)
    frameRate.value = settings.value.frame_rate
  } catch (e) {
    error.value = errorText(e)
    // Reload so the controls show what is actually stored.
    try {
      settings.value = await getSettings()
      frameRate.value = settings.value.frame_rate
    } catch {
      // keep the error shown above
    }
  } finally {
    saving.value = false
  }
}

/** Sends the frame rate once a drag ends or the number is committed (NumberField clamps it to 1 to 60). */
function onFrameRate(fps: number) {
  frameRate.value = fps
  if (settings.value && fps !== settings.value.frame_rate) void patch({ frame_rate: fps })
}

// ---- Updates (ADR 0001 #41) -----------------------------------------------------------------------------------

const update = ref<UpdateView | null>(null)
const updateError = ref<string | null>(null)
/** "Update now" was clicked and has not come back (on success the app ends and restarts). */
const installing = ref(false)

/** Takes a view unless a newer one (higher `seq`, e.g. from an `update-status` event) is already shown, so a command
 * reply taken before the latest change cannot roll the status back. */
function takeUpdate(v: UpdateView) {
  if (!update.value || v.seq >= update.value.seq) update.value = v
}

/** Why updates are off in this build (the backend's text), or `null` when they are on. */
const offReason = computed(() => (update.value?.status.state === 'disabled' ? update.value.status.reason : null))
/** A check, a download or an install is running. */
const updateBusy = computed(() => {
  const state = update.value?.status.state
  return installing.value || state === 'checking' || state === 'downloading' || state === 'installing'
})
const canCheck = computed(() => update.value !== null && offReason.value === null && !updateBusy.value)
const canInstall = computed(() => update.value?.pending != null && offReason.value === null && !updateBusy.value)

const updateLine = computed(() => {
  const s = update.value?.status
  if (!s) return ''
  switch (s.state) {
    case 'disabled':
      return s.reason
    case 'idle':
      return 'Not checked yet'
    case 'checking':
      return 'Checking for updates…'
    case 'up_to_date':
      return 'Up to date'
    case 'downloading':
      return `Downloading version ${s.version}…`
    case 'ready':
      return `Version ${s.version} is ready`
    case 'installing':
      return `Installing version ${s.version}…`
    case 'failed':
      return `Update failed: ${s.error}`
  }
  return ''
})

const checkLabel = computed(() => {
  const state = update.value?.status.state
  return state === 'checking' ? 'Checking…' : state === 'downloading' ? 'Downloading…' : 'Check for updates'
})

let unlistenUpdate: (() => void) | null = null
let unmounted = false
onMounted(async () => {
  const unlisten = await on('update-status', takeUpdate)
  // The page may have gone while the listener was being registered.
  if (unmounted) {
    unlisten()
    return
  }
  unlistenUpdate = unlisten
  try {
    takeUpdate(await updateStatus())
  } catch (e) {
    updateError.value = errorText(e)
  }
})
onBeforeUnmount(() => {
  unmounted = true
  unlistenUpdate?.()
})

async function onCheckUpdates() {
  updateError.value = null
  try {
    takeUpdate(await checkForUpdates())
  } catch (e) {
    updateError.value = errorText(e)
  }
}

async function onInstallUpdate() {
  updateError.value = null
  installing.value = true
  try {
    // Resolves only when nothing was installed; on success the app closes and starts the new version.
    takeUpdate(await installUpdate())
  } catch (e) {
    updateError.value = errorText(e)
    try {
      takeUpdate(await updateStatus())
    } catch {
      // keep the error shown above
    }
  } finally {
    installing.value = false
  }
}
</script>

<template>
  <section class="page">
    <PageHeader title="Settings" />

    <Notice v-if="settings?.save_error" variant="error">{{ settings.save_error }}</Notice>
    <Notice v-if="error && error !== settings?.save_error" variant="error">{{ error }}</Notice>

    <template v-if="settings">
      <div class="group">
        <h2>Startup</h2>
        <Switch
          :model-value="settings.autostart"
          label="Start with Windows"
          :disabled="locked"
          @update:model-value="patch({ autostart: $event })"
        />
        <Switch
          :model-value="settings.start_minimized"
          label="Start minimized"
          :disabled="locked"
          @update:model-value="patch({ start_minimized: $event })"
        />
      </div>

      <div class="group">
        <h2>Updates</h2>
        <Field
          v-slot="{ id, describedBy }"
          label="Frame rate"
          hint="How often per second a device gets new colours while you edit, 1 to 60."
          layout="stacked"
        >
          <Slider
            v-model="frameRate"
            class="rate"
            :min="1"
            :max="60"
            aria-label="Frame rate"
            :aria-describedby="describedBy"
            :disabled="locked"
            @change="onFrameRate"
          />
          <NumberField
            :id="id"
            :model-value="frameRate"
            :min="1"
            :max="60"
            :aria-describedby="describedBy"
            :disabled="locked"
            @update:model-value="onFrameRate"
          />
        </Field>
      </div>

      <div class="group">
        <h2>Device detectors</h2>
        <Switch
          v-for="(on, family) in settings.detectors"
          :key="family"
          :model-value="on"
          :label="familyLabel(String(family))"
          :disabled="locked"
          @update:model-value="patch({ detectors: { [family]: $event } })"
        />
      </div>
    </template>

    <div class="group">
      <h2>About</h2>
      <KeyValueList v-if="info || update">
        <template v-if="info">
          <dt>Name</dt>
          <dd>{{ info.name }}</dd>
          <dt>Version</dt>
          <dd>{{ info.version }}</dd>
          <dt>Build</dt>
          <dd>{{ info.profile }}</dd>
        </template>
        <template v-if="update">
          <dt>Updates</dt>
          <dd :class="{ selectable: update.status.state === 'failed' }" aria-live="polite">{{ updateLine }}</dd>
        </template>
      </KeyValueList>
      <Notice v-if="updateError" variant="error">{{ updateError }}</Notice>
      <div class="row update-actions">
        <Tooltip v-if="offReason" v-slot="{ describedBy }" :text="offReason" class="off-tip">
          <Button disabled :aria-describedby="describedBy">Check for updates</Button>
        </Tooltip>
        <Button v-else :disabled="!canCheck" @click="onCheckUpdates">{{ checkLabel }}</Button>
        <Button :disabled="!canInstall" @click="onInstallUpdate">
          {{ installing || update?.status.state === 'installing' ? 'Installing…' : 'Update now' }}
        </Button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.group {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.group + .group {
  margin-top: var(--space-14);
}

.group h2 {
  margin-bottom: 0;
}

.rate {
  width: var(--control-max-width);
}

.update-actions {
  gap: var(--space-4);
}

/* A disabled button gets no pointer events in WebView2; let the pointer reach the Tooltip anchor around it. */
.off-tip :deep(button:disabled) {
  pointer-events: none;
}
</style>
