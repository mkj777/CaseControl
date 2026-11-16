<script setup lang="ts">
// Settings: startup, update rate, device detectors, the OpenRGB port check and information about this build.
// Every change is saved and applied right away.
import { computed, onMounted, ref } from 'vue'
import {
  appInfo,
  errorText,
  getSettings,
  hideToTray,
  setSettings,
  type AppInfo,
  type SettingsPatch,
  type SettingsView,
} from '../ipc'
import { Button, Field, KeyValueList, Notice, NumberField, PageHeader, Slider, Switch } from '../components/ui'
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

async function onHide() {
  try {
    await hideToTray()
  } catch (e) {
    error.value = errorText(e)
  }
}
</script>

<template>
  <section class="page">
    <PageHeader title="Settings" description="Changes are saved right away." />

    <Notice v-if="settings?.save_error" variant="error">{{ settings.save_error }}</Notice>
    <Notice v-if="error && error !== settings?.save_error" variant="error">{{ error }}</Notice>

    <template v-if="settings">
      <div class="group">
        <h2>Startup</h2>
        <Switch
          :model-value="settings.autostart"
          label="Start with Windows"
          hint="CaseControl starts in the tray when you sign in."
          :disabled="locked"
          @update:model-value="patch({ autostart: $event })"
        />
        <Switch
          :model-value="settings.start_minimized"
          label="Start minimized"
          hint="Open in the tray without showing this window."
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
        <p class="hint">Switch a family off to leave its devices alone. CaseControl releases them right away.</p>
        <Switch
          v-for="(on, family) in settings.detectors"
          :key="family"
          :model-value="on"
          :label="familyLabel(String(family))"
          :disabled="locked"
          @update:model-value="patch({ detectors: { [family]: $event } })"
        />
      </div>

      <div class="group">
        <h2>Conflicts</h2>
        <Switch
          :model-value="settings.check_openrgb_port"
          label="Check for a running OpenRGB server"
          hint="Scans try a local connection to port 6742. This is the only connection CaseControl ever opens."
          :disabled="locked"
          @update:model-value="patch({ check_openrgb_port: $event })"
        />
      </div>
    </template>

    <div class="group">
      <h2>About</h2>
      <KeyValueList v-if="info">
        <dt>Name</dt>
        <dd>{{ info.name }}</dd>
        <dt>Version</dt>
        <dd>{{ info.version }}</dd>
        <dt>Build</dt>
        <dd>{{ info.profile }}</dd>
      </KeyValueList>
      <div>
        <Button @click="onHide">Hide to tray</Button>
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

.hint {
  max-width: var(--row-max-width);
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.rate {
  width: var(--control-max-width);
}
</style>
