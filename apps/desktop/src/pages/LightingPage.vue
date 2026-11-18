<script setup lang="ts">
// Lighting, the devices overview (ADR 0001 #18, renamed from Devices in #37): one large tile per device with a small
// live preview; a tile opens the device view. Tiles fill the window width (ADR 0001 #28). Right-click a tile to hide
// or unhide the device; hidden devices are shown greyed only with "Show hidden" on (ADR 0001 #26, #27). No visible
// title (the sidebar says where you are; the h1 stays for screen readers); the page starts with one toolbar row
// (ADR 0001 #41): the profile dropdown (#40), one takeover button (Take over all / Release all, #33, #35, #37), Show
// hidden and Rescan on the left, All brightness and the fallback colour with its help tooltip on the right. Each tile shows the real status and what
// the device shows: its colours, the name of its own mode while released, empty LEDs when that is unknown.
// While Rescan runs the page is inactive (controls disabled, tiles dimmed and inert, `aria-busy`). Hiding a driven
// device and Release all confirm with a toast, since the change is not visible where the user clicked (ADR 0001 #36).
import { computed, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import BrightnessSlider from '../components/BrightnessSlider.vue'
import ColorButton from '../components/ColorButton.vue'
import DevicePreview from '../components/DevicePreview.vue'
import ProfileMenu from '../components/ProfileMenu.vue'
import {
  Button,
  ContextMenu,
  EmptyState,
  IconButton,
  ListRow,
  Notice,
  PageHeader,
  StatusBadge,
  Switch,
  Tile,
  Tooltip,
  menuPoint,
  type MenuItem,
} from '../components/ui'
import {
  connectAll,
  errorText,
  getSettings,
  releaseAll,
  setDefaultColor,
  setMasterBrightness,
  type DeviceView,
} from '../ipc'
import {
  STATUS_LABEL,
  canTakeOver,
  deviceErrorText,
  familyLabel,
  lightingLabel,
  shownColors,
  statusOf,
  zoneIndicesNamed,
} from '../lib/device'
import {
  deviceState,
  findDevice,
  hiddenZoneNames,
  hideDevice,
  isDeviceHidden,
  refreshDevices,
  rescanDevices,
} from '../stores/devices'
import { latestSender } from '../lib/throttle'
import { releasedText, showToast } from '../stores/toast'

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
  menuAt.value = menuPoint(e)
}

async function toggleHidden(id: string, hidden: boolean) {
  error.value = null
  // Hiding releases a driven device; only that is worth a toast.
  const releases = hidden && findDevice(id)?.status.state === 'active'
  try {
    await hideDevice(id, hidden)
    if (releases) showToast('Device released')
  } catch (e) {
    error.value = errorText(e)
  }
}

// ---- Take over all, All brightness, Release all (ADR 0001 #33, #37) -----------------------------------------

const takingOver = ref(false)
const releasing = ref(false)
/** The lighting controls need a running engine and are off while a rescan runs. */
const engineDown = computed(() => !deviceState.loaded || deviceState.engineError !== null)
const scanning = computed(() => deviceState.scanning)
/** Nothing to release: no device is driven. */
const noneActive = computed(() => !deviceState.devices.some((d) => statusOf(d) === 'active'))
/** Take over all has something to do: a released, visible device the engine says it can take over (with its
 * setting, its own mode or the fallback colour; ADR 0001 #36, #37). */
const canTakeOverAll = computed(() =>
  deviceState.devices.some(
    (d) => statusOf(d) === 'released' && !isDeviceHidden(d.description.info.id) && canTakeOver(d),
  ),
)

// One takeover button (ADR 0001 #41): "Take over all" while a visible device can still be taken over (the rule
// above), else "Release all". A running action keeps its own label until it ends. Every label is laid out in the
// same grid cell and only the current one is visible, so the button keeps the width of the longest.
const TAKEOVER_LABELS = ['Take over all', 'Release all', 'Releasing…'] as const
const takeoverMode = computed<'take' | 'release'>(() => {
  if (takingOver.value) return 'take'
  if (releasing.value) return 'release'
  return canTakeOverAll.value ? 'take' : 'release'
})
const takeoverLabel = computed(() =>
  releasing.value ? 'Releasing…' : takeoverMode.value === 'take' ? 'Take over all' : 'Release all',
)
const takeoverDisabled = computed(
  () =>
    engineDown.value ||
    scanning.value ||
    takingOver.value ||
    releasing.value ||
    (takeoverMode.value === 'release' && noneActive.value),
)

function onTakeover() {
  if (takeoverMode.value === 'take') void onTakeOverAll()
  else void onReleaseAll()
}

async function sendMaster(percent: number, persist: boolean) {
  deviceState.lighting = await setMasterBrightness(percent, persist)
}

function lightingFailed(e: unknown) {
  error.value = errorText(e)
  void refreshDevices()
}

async function onTakeOverAll() {
  error.value = null
  takingOver.value = true
  try {
    deviceState.lighting = await connectAll()
  } catch (e) {
    lightingFailed(e)
  } finally {
    takingOver.value = false
  }
}

// ---- Fallback colour (ADR 0001 #36, #37; the `default_color` setting) -----------------------------------------
// Works like the brightness sliders: while the picker is dragged the value goes live to the engine through a
// coalescing sender (not saved); the final value of a gesture is saved once. It only feeds later takeovers; devices
// already driven keep their colours. Its errors are its own Notice, so a colour change clears only those.

/** Segoe Fluent Icons "Help" (a question mark). */
const HELP = ''
const FALLBACK_HELP =
  'Take over all gives CaseControl control of every device. A device with nothing else selected shows this colour.'

/** Shown in the circle and the picker; follows the pointer while the sends catch up. */
const fallbackColor = ref('#FFFFFF')
const colorError = ref<string | null>(null)
/** Not loaded yet, or settings cannot be saved this session (the error shows on the Settings page). */
const colorUnavailable = ref(true)
const colorLocked = computed(() => colorUnavailable.value || scanning.value)

onMounted(async () => {
  try {
    const s = await getSettings()
    fallbackColor.value = s.default_color
    colorUnavailable.value = s.save_error !== null
  } catch (e) {
    colorError.value = errorText(e)
  }
})

async function colorFailed(e: unknown) {
  colorError.value = errorText(e)
  try {
    fallbackColor.value = (await getSettings()).default_color
  } catch {
    // keep the error shown above
  }
}

const colorSender = latestSender<string>((hex) => setDefaultColor(hex, false), colorFailed)

function onFallbackColor(hex: string) {
  colorError.value = null
  fallbackColor.value = hex
  colorSender.push(hex)
}

async function onFallbackColorCommit(hex: string) {
  colorError.value = null
  fallbackColor.value = hex
  // The final value replaces whatever live value is still queued.
  colorSender.cancel()
  try {
    await colorSender.flush()
    fallbackColor.value = await setDefaultColor(hex, true)
  } catch (e) {
    await colorFailed(e)
  }
}

async function onReleaseAll() {
  error.value = null
  releasing.value = true
  try {
    const report = await releaseAll()
    if (report.released > 0) showToast(releasedText(report.released))
    if (report.failed.length > 0) {
      const names = report.failed.map((r) => `${findDevice(r.id)?.description.info.name ?? r.id}: ${r.error}`)
      error.value = `Some devices could not be released. ${names.join('; ')}`
    }
  } catch (e) {
    error.value = errorText(e)
  } finally {
    releasing.value = false
    void refreshDevices()
  }
}
</script>

<template>
  <section class="page wide" :aria-busy="scanning">
    <PageHeader title="Lighting" title-hidden wrap-controls flex-actions>
      <!-- ADR 0001 #41: the left group (slot `controls`) holds the profile dropdown, the takeover button, Show hidden
           and Rescan; the right group (slot `actions`, the flexible one) All brightness and the fallback colour with
           its "?", whose popover and tooltip open towards the left so they stay inside the page. -->
      <template #controls>
        <ProfileMenu :disabled="scanning" @error="(m) => (error = m)" />
        <Button :disabled="takeoverDisabled" @click="onTakeover">
          <span class="flip">
            <span v-for="l in TAKEOVER_LABELS" :key="l" :class="{ off: l !== takeoverLabel }">{{ l }}</span>
          </span>
        </Button>
        <Switch v-model="deviceState.showHiddenDevices" label="Show hidden" compact :disabled="scanning" />
        <Button :disabled="scanning" @click="rescanDevices">
          {{ scanning ? 'Scanning…' : 'Rescan' }}
        </Button>
      </template>
      <template #actions>
        <BrightnessSlider
          label="All brightness"
          :value="deviceState.lighting.master_brightness"
          :send="sendMaster"
          :disabled="engineDown || scanning || releasing"
          @error="lightingFailed"
        />
        <ColorButton
          :model-value="fallbackColor"
          label="Fallback colour"
          align="end"
          :disabled="colorLocked"
          @update:model-value="onFallbackColor"
          @change="onFallbackColorCommit"
        />
        <Tooltip v-slot="{ describedBy }" :text="FALLBACK_HELP" align="end">
          <IconButton label="Help" :glyph="HELP" no-title :aria-describedby="describedBy" />
        </Tooltip>
      </template>
    </PageHeader>

    <Notice v-if="deviceState.engineError" variant="error">
      {{ deviceState.engineError }}
      <template #details>Restart CaseControl to try again. Details are in the log file.</template>
    </Notice>
    <Notice v-if="deviceState.error" variant="error">Could not load the device list: {{ deviceState.error }}</Notice>
    <Notice v-if="error" variant="error">{{ error }}</Notice>
    <Notice v-if="colorError" variant="error">{{ colorError }}</Notice>

    <div v-if="tiles.length > 0" class="grid" :class="{ busy: scanning }" :inert="scanning">
      <Tile
        v-for="t in tiles"
        :key="t.id"
        :to="deviceLink(t.id)"
        :hidden="t.hidden"
        :name="t.d.description.info.name"
        :meta="t.hidden ? undefined : (lightingLabel(t.d) ?? undefined)"
        @contextmenu="openMenu($event, t.id, t.hidden)"
      >
        <template #badge>
          <StatusBadge v-if="t.hidden" status="hidden">Hidden</StatusBadge>
          <StatusBadge v-else :status="statusOf(t.d)">{{ STATUS_LABEL[statusOf(t.d)] }}</StatusBadge>
        </template>
        <DevicePreview
          :description="t.d.description"
          :colors="shownColors(t.d)"
          :hidden-zones="hiddenZonesOf(t.d)"
        />
      </Tile>
    </div>

    <EmptyState v-else-if="allHidden" title="All devices are hidden">
      Turn on Show hidden to see them. Right-click a device to unhide it.
    </EmptyState>

    <EmptyState v-else-if="deviceState.loaded && !deviceState.engineError" title="No devices detected">
      Other RGB software can hold a device so CaseControl cannot reach it. Close it and rescan, or see
      <RouterLink to="/conflicts">Conflicts</RouterLink> for what is running.
      <template #action>
        <Button variant="primary" :disabled="scanning" @click="rescanDevices">
          {{ scanning ? 'Scanning…' : 'Rescan' }}
        </Button>
      </template>
    </EmptyState>

    <ContextMenu :at="menuAt" :items="menuItems" @close="menuAt = null" />

    <div v-if="deviceState.failures.length > 0" class="section">
      <h2>Found but not started</h2>
      <ul>
        <ListRow v-for="(f, i) in deviceState.failures" :key="i" :name="familyLabel(f.family)">
          <span class="muted selectable">{{ deviceErrorText(f.error) }}</span>
        </ListRow>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(var(--tile-min), 1fr));
  gap: var(--space-6);
}

/* The takeover button's labels share one grid cell; hidden ones keep their width but are not read or seen. */
.flip {
  display: inline-grid;
}

.flip > span {
  grid-area: 1 / 1;
  text-align: center;
}

.flip > .off {
  visibility: hidden;
}

/* Rescan running: the tiles are inert (no clicks, no focus, no context menu) and dimmed like other disabled
   controls (the 50 % of Switch and ColorPicker). */
.grid.busy {
  opacity: 0.5;
}
</style>
