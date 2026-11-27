<script setup lang="ts">
// Settings: startup, hardware, update rate, device detectors and information about this build. Every change is saved
// and applied right away.
//
// ADR 0001 #42: "Apply colours before sign-in" (Startup) and "Save colours to the devices" (Hardware) have their own
// commands because one asks for a Windows permission prompt and the other writes to devices. Each of the two switches
// is busy (disabled, `aria-busy`) only while its own command runs; the other controls stay usable. After "Save colours
// to the devices" was switched, the line under it names the devices that were not saved or restored. The fallback colour of a takeover lives in the Lighting toolbar, and the OpenRGB port check has
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
  setApplyBeforeSignIn,
  setSaveToDevices,
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

type OwnSwitch = 'before-sign-in' | 'save-to-devices'
/** Which of the two switches with their own command (a permission prompt, device writes) are running now. */
const busy = ref<Record<OwnSwitch, boolean>>({ 'before-sign-in': false, 'save-to-devices': false })
/** A switch is off only while its own command runs, or for the whole session when settings cannot be saved. */
function switchLocked(which: OwnSwitch): boolean {
  return busy.value[which] || settings.value?.save_error != null
}

/** Runs one of the two switches with their own command; errors show in the page's error notice as the backend
 * gives them. */
async function runSwitch(which: OwnSwitch, on: boolean) {
  busy.value[which] = true
  error.value = null
  try {
    settings.value = which === 'before-sign-in' ? await setApplyBeforeSignIn(on) : await setSaveToDevices(on)
  } catch (e) {
    error.value = errorText(e)
    // Reload so the switch shows what is actually stored.
    try {
      settings.value = await getSettings()
    } catch {
      // keep the error shown above
    }
  } finally {
    busy.value[which] = false
  }
}

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
        <Switch
          :model-value="settings.apply_before_sign_in"
          label="Apply colours before sign-in"
          hint="Needs administrator rights to turn on or off."
          :disabled="switchLocked('before-sign-in')"
          :aria-busy="busy['before-sign-in']"
          @update:model-value="runSwitch('before-sign-in', $event)"
        />
      </div>

      <div class="group">
        <h2>Hardware</h2>
        <Switch
          :model-value="settings.keep_colors_on_sign_out"
          label="Keep colours when signing out"
          :disabled="locked"
          @update:model-value="patch({ keep_colors_on_sign_out: $event })"
        />
        <div class="with-note">
          <Switch
            :model-value="settings.save_to_devices"
            label="Save colours to the devices"
            hint="Devices that can store lighting show it from power-on. Writes their memory at most once per session."
            :disabled="switchLocked('save-to-devices')"
            :aria-busy="busy['save-to-devices']"
            @update:model-value="runSwitch('save-to-devices', $event)"
          />
          <p v-if="settings.save_to_devices_note" class="note" aria-live="polite">
            {{ settings.save_to_devices_note }}
          </p>
        </div>
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

.with-note {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  max-width: var(--row-max-width);
}

.note {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--text-muted);
  /* Device names and reasons can be copied (ADR 0001 #32). */
  user-select: text;
  cursor: text;
}

.update-actions {
  gap: var(--space-4);
}

/* A disabled button gets no pointer events in WebView2; let the pointer reach the Tooltip anchor around it. */
.off-tip :deep(button:disabled) {
  pointer-events: none;
}
</style>
