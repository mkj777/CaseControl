<script setup lang="ts">
// Devices overview (ADR 0001 #18): one large tile per device with a small live preview; a tile opens the device
// view. Tiles fill the window width (ADR 0001 #28). Right-click a tile to hide or unhide the device; hidden devices
// are shown greyed only with "Show hidden" on (ADR 0001 #26, #27). No visible title (the sidebar says where you are;
// the h1 stays for screen readers); the page starts with one toolbar row: All brightness, Connect all and Release
// all (ADR 0001 #33, #35) on the left, Show hidden and Rescan on the right. Each tile shows the real status and what
// the device shows: its colours, the name of its own mode while released, empty LEDs when that is unknown.
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import BrightnessSlider from '../components/BrightnessSlider.vue'
import DevicePreview from '../components/DevicePreview.vue'
import {
  Button,
  ContextMenu,
  EmptyState,
  ListRow,
  Notice,
  PageHeader,
  StatusBadge,
  Switch,
  Tile,
  menuPoint,
  type MenuItem,
} from '../components/ui'
import { connectAll, errorText, releaseAll, setMasterBrightness, type DeviceView } from '../ipc'
import {
  STATUS_LABEL,
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
  try {
    await hideDevice(id, hidden)
  } catch (e) {
    error.value = errorText(e)
  }
}

// ---- All brightness, Connect all, Release all (ADR 0001 #33) ---------------------------------------------------

const connecting = ref(false)
const releasing = ref(false)
/** The lighting controls need a running engine. */
const engineDown = computed(() => !deviceState.loaded || deviceState.engineError !== null)
/** Nothing to release: no device is driven. */
const noneActive = computed(() => !deviceState.devices.some((d) => statusOf(d) === 'active'))
/** Connect all has something to do: a released, visible device has a setting or a known own mode to be taken over
 * with (one without either would stay released, so it does not count). */
const canConnect = computed(() =>
  deviceState.devices.some(
    (d) =>
      statusOf(d) === 'released' &&
      !isDeviceHidden(d.description.info.id) &&
      (d.setting !== null || d.hardware !== null),
  ),
)

async function sendMaster(percent: number, persist: boolean) {
  deviceState.lighting = await setMasterBrightness(percent, persist)
}

function lightingFailed(e: unknown) {
  error.value = errorText(e)
  void refreshDevices()
}

async function onConnectAll() {
  error.value = null
  connecting.value = true
  try {
    deviceState.lighting = await connectAll()
  } catch (e) {
    lightingFailed(e)
  } finally {
    connecting.value = false
  }
}

async function onReleaseAll() {
  error.value = null
  releasing.value = true
  try {
    const failed = (await releaseAll()).filter((r) => r.error !== null)
    if (failed.length > 0) {
      const names = failed.map((r) => `${findDevice(r.id)?.description.info.name ?? r.id}: ${r.error}`)
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
  <section class="page wide">
    <PageHeader title="Devices" title-hidden>
      <template #actions>
        <Switch v-model="deviceState.showHiddenDevices" label="Show hidden" compact />
        <Button :disabled="deviceState.scanning" @click="rescanDevices">
          {{ deviceState.scanning ? 'Scanning…' : 'Rescan' }}
        </Button>
      </template>
      <template #controls>
        <BrightnessSlider
          label="All brightness"
          :value="deviceState.lighting.master_brightness"
          :send="sendMaster"
          :disabled="engineDown"
          @error="lightingFailed"
        />
        <Button :disabled="engineDown || connecting || releasing || !canConnect" @click="onConnectAll">
          Connect all
        </Button>
        <Button :disabled="engineDown || connecting || releasing || noneActive" @click="onReleaseAll">
          {{ releasing ? 'Releasing…' : 'Release all' }}
        </Button>
      </template>
    </PageHeader>

    <Notice v-if="deviceState.engineError" variant="error">
      {{ deviceState.engineError }}
      <template #details>Restart CaseControl to try again. Details are in the log file.</template>
    </Notice>
    <Notice v-if="deviceState.error" variant="error">Could not load the device list: {{ deviceState.error }}</Notice>
    <Notice v-if="error" variant="error">{{ error }}</Notice>

    <div v-if="tiles.length > 0" class="grid">
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
        <Button variant="primary" :disabled="deviceState.scanning" @click="rescanDevices">
          {{ deviceState.scanning ? 'Scanning…' : 'Rescan' }}
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
</style>
