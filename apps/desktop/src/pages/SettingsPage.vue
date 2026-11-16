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

const checked = (e: Event) => (e.target as HTMLInputElement).checked

function onFrameRate() {
  const fps = Math.min(60, Math.max(1, Math.round(Number(frameRate.value) || 1)))
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
    <header class="page-header">
      <div>
        <h1>Settings</h1>
        <p class="page-description">Changes are saved right away.</p>
      </div>
    </header>

    <p v-if="settings?.save_error" class="notice">{{ settings.save_error }}</p>
    <p v-if="error && error !== settings?.save_error" class="notice">{{ error }}</p>

    <template v-if="settings">
      <div class="group">
        <h2>Startup</h2>
        <label class="setting">
          <input
            type="checkbox"
            :checked="settings.autostart"
            :disabled="locked"
            @change="patch({ autostart: checked($event) })"
          />
          <span>
            Start with Windows
            <span class="hint">CaseControl starts in the tray when you sign in.</span>
          </span>
        </label>
        <label class="setting">
          <input
            type="checkbox"
            :checked="settings.start_minimized"
            :disabled="locked"
            @change="patch({ start_minimized: checked($event) })"
          />
          <span>
            Start minimized
            <span class="hint">Open in the tray without showing this window.</span>
          </span>
        </label>
      </div>

      <div class="group">
        <h2>Updates</h2>
        <div class="setting rate">
          <span>
            Frame rate
            <span class="hint">How often per second a device gets new colours while you edit, 1 to 60.</span>
          </span>
          <span class="row">
            <input v-model.number="frameRate" type="range" min="1" max="60" step="1" aria-label="Frame rate" :disabled="locked" @change="onFrameRate" />
            <input v-model.number="frameRate" type="number" min="1" max="60" aria-label="Frame rate" :disabled="locked" @change="onFrameRate" />
          </span>
        </div>
      </div>

      <div class="group">
        <h2>Device detectors</h2>
        <p class="hint block">Switch a family off to leave its devices alone. CaseControl releases them right away.</p>
        <label v-for="(on, family) in settings.detectors" :key="family" class="setting">
          <input
            type="checkbox"
            :checked="on"
            :disabled="locked"
            @change="patch({ detectors: { [family]: checked($event) } })"
          />
          <span>{{ familyLabel(String(family)) }}</span>
        </label>
      </div>

      <div class="group">
        <h2>Conflicts</h2>
        <label class="setting">
          <input
            type="checkbox"
            :checked="settings.check_openrgb_port"
            :disabled="locked"
            @change="patch({ check_openrgb_port: checked($event) })"
          />
          <span>
            Check for a running OpenRGB server
            <span class="hint">Scans try a local connection to port 6742. This is the only connection CaseControl ever opens.</span>
          </span>
        </label>
      </div>
    </template>

    <div class="group">
      <h2>About</h2>
      <dl v-if="info" class="info">
        <dt>Name</dt>
        <dd>{{ info.name }}</dd>
        <dt>Version</dt>
        <dd>{{ info.version }}</dd>
        <dt>Build</dt>
        <dd>{{ info.profile }}</dd>
      </dl>
      <button type="button" @click="onHide">Hide to tray</button>
    </div>
  </section>
</template>

<style scoped>
.notice {
  margin-bottom: var(--space-6);
}

.group {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  margin-bottom: var(--space-10);
}

.group h2 {
  margin-bottom: 0;
}

.setting {
  display: flex;
  align-items: flex-start;
  gap: var(--space-4);
  max-width: 640px;
  cursor: pointer;
}

.setting input[type='checkbox'] {
  flex: none;
  margin: 3px 0 0;
}

.setting > span {
  display: flex;
  flex-direction: column;
}

.setting.rate {
  flex-direction: column;
  gap: var(--space-3);
  cursor: default;
}

.rate input[type='range'] {
  width: 280px;
}

.rate input[type='number'] {
  width: 80px;
}

.hint {
  font-size: var(--font-size-sm);
  color: var(--color-text-muted);
}

.block {
  max-width: 640px;
}

.info {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: var(--space-2) var(--space-6);
  margin: 0 0 var(--space-4);
}

.info dt {
  color: var(--color-text-muted);
}

.info dd {
  margin: 0;
}

.group > button {
  align-self: flex-start;
}
</style>
