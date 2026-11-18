<script setup lang="ts">
// Device view (ADR 0001 #18-#22, #25): a large preview where LEDs are selected, the mode menu, the colour picker
// (static mode) or the mode's controls (hardware modes), zone sizes and the device actions. The preview scales with
// the window and the selection controls sit above it (ADR 0001 #28). Right-click a zone to hide or unhide it; hidden
// zones stay black and are drawn (greyed) only with "Show hidden" on (ADR 0001 #26, #27).
//
// The status badge is the engine's real status. While released the preview shows the device's own lighting (its
// colours if they can be read, else empty LEDs) and the mode menu names its own mode; every control still works and
// takes the device over.
//
// Brightness-only LEDs (zones with `brightness_only`, a fixed white such as the RTX 5080 FE) take no colour: a device
// made only of them shows no colour picker and no LED selection, and its preview is white at the output intensity. On
// a mixed device the colour controls apply to the colour LEDs only. Instead, each visible brightness-only LED gets
// its own level slider (0-100 %, ADR 0001 #41), stored as a grey in the direct colours (`whiteLevel`, mirroring
// `Rgb::white_level`), so it autosaves into the active profile; the device brightness at the top scales them all.
//
// Every colour and mode change goes through one ordered sender, so the last user action always wins. While changes
// are queued or in flight, the page shows its own latest intent instead of the store, so a refresh that raced an
// edit cannot roll the controls back; the intent is dropped once the sender is idle and a fresh reload has landed.
import { computed, ref, watch } from 'vue'
import BrightnessSlider from '../components/BrightnessSlider.vue'
import ColorPicker from '../components/ColorPicker.vue'
import DevicePreview from '../components/DevicePreview.vue'
import ModeControls from '../components/ModeControls.vue'
import {
  Button,
  Card,
  ConfirmDialog,
  ContextMenu,
  EmptyState,
  Field,
  KeyValueList,
  Notice,
  NumberField,
  PageHeader,
  Select,
  Slider,
  StatusBadge,
  Switch,
  menuPoint,
  type MenuItem,
} from '../components/ui'
import {
  errorText,
  releaseDevice,
  resizeZone,
  saveToHardware,
  setDeviceBrightness,
  setDirect,
  setMode,
  type DeviceView,
  type ModeState,
  type Zone,
} from '../ipc'
import { BLACK, levelOf, whiteLevel } from '../lib/color'
import {
  STATUS_LABEL,
  brightnessMode,
  canTakeOver,
  canSaveToHardware,
  currentHardwareMode,
  hardwareModes,
  hasCap,
  kindLabel,
  ledsOfZones,
  ownLighting,
  shownColors,
  stateFor,
  statusOf,
  visibleLeds,
  zoneIndicesNamed,
} from '../lib/device'
import {
  EMPTY,
  blackOut,
  clampSelection,
  paint,
  selectionColor,
  without,
  zoneIndices,
  type Selection,
} from '../lib/selection'
import { latestSender } from '../lib/throttle'
import {
  deviceState,
  echoColors,
  findDevice,
  hiddenZoneNames,
  hideZone,
  isDeviceHidden,
  refreshDevices,
  replaceDevice,
} from '../stores/devices'

const props = defineProps<{ id: string }>()

const device = computed(() => findDevice(props.id))
const desc = computed(() => device.value?.description)
const faulted = computed(() => device.value !== undefined && statusOf(device.value) === 'faulted')
const deviceHidden = computed(() => isDeviceHidden(props.id))
/** Controls are disabled: the device is faulted or hidden. */
const locked = computed(() => faulted.value || deviceHidden.value)

/** Indices of the hidden zones, their LEDs, and the LEDs a colour without a selection goes to. */
const hiddenZones = computed(() =>
  desc.value ? zoneIndicesNamed(desc.value, hiddenZoneNames(props.id)) : new Set<number>(),
)
const hiddenLeds = computed(() => (desc.value ? ledsOfZones(desc.value, hiddenZones.value) : new Set<number>()))
const visibleScope = computed(() => new Set(desc.value ? visibleLeds(desc.value, hiddenZones.value) : []))
/** LEDs of brightness-only zones, which take no colour. */
const fixedLeds = computed(
  () => new Set((desc.value?.zones ?? []).filter((z) => z.brightness_only).flatMap((z) => zoneIndices(z))),
)
/** Every zone with LEDs is brightness-only: the device takes no colour at all. */
const brightnessOnly = computed(() => {
  const lit = (desc.value?.zones ?? []).filter((z) => z.len > 0)
  return lit.length > 0 && lit.every((z) => z.brightness_only)
})
/** The visible LEDs that take a colour. */
const colorScope = computed(() => without(visibleScope.value, fixedLeds.value))
const hasDirect = computed(() => desc.value?.direct_mode != null)
const modes = computed(() => (desc.value ? hardwareModes(desc.value) : []))

const error = ref<string | null>(null)
const notice = ref<string | null>(null)
const busy = ref(false)

// ---- the ordered write path ------------------------------------------------------------------------------------

type Write = { kind: 'direct'; colors: string[] } | { kind: 'mode'; state: ModeState }

/** What the user asked for last, while it may not be reflected in the store yet. */
const intent = ref<Write | null>(null)
let intentToken = 0

function dropIntent() {
  intent.value = null
  intentToken++
}

function fail(e: unknown) {
  error.value = errorText(e)
  dropIntent()
  void refreshDevices()
}

/** After the last write: reload, and drop the local intent if nothing newer happened meanwhile. */
async function settle() {
  const token = intentToken
  await refreshDevices()
  if (token === intentToken && sender.idle()) intent.value = null
}

const sender = latestSender<Write>(
  (w) => (w.kind === 'direct' ? setDirect(props.id, w.colors) : setMode(props.id, w.state)),
  fail,
  () => void settle(),
)

function write(w: Write) {
  error.value = null
  notice.value = null
  intent.value = w
  intentToken++
  if (w.kind === 'direct') echoColors(props.id, w.colors)
  else if (device.value) device.value.setting = { kind: 'mode', state: w.state }
  sender.push(w)
}

/** Colours to paint on: the latest local edit, else the engine's buffer. */
const colors = computed<readonly string[]>(() =>
  intent.value?.kind === 'direct' ? intent.value.colors : (device.value?.colors ?? []),
)

/** What the preview shows: the latest local edit, else what the device shows (`null`: its own, unknown lighting). */
const previewColors = computed<readonly string[] | null>(() => {
  if (intent.value?.kind === 'direct') return intent.value.colors
  return device.value ? shownColors(device.value) : []
})

// ---- selection and colour --------------------------------------------------------------------------------------

const selection = ref<Selection>(EMPTY)
const pickerColor = ref('#FFFFFF')

function syncPicker() {
  if (device.value) pickerColor.value = selectionColor(colors.value, selection.value, '#FFFFFF')
}

function onSelect(sel: Selection) {
  selection.value = sel
  syncPicker()
}

watch(
  () => props.id,
  () => {
    sender.cancel()
    dropIntent()
    selection.value = EMPTY
    error.value = null
    notice.value = null
    syncPicker()
  },
)
watch(
  () => device.value !== undefined,
  (present) => present && syncPicker(),
  { immediate: true },
)

// A zone hidden while some of its LEDs were selected leaves the selection.
watch(hiddenLeds, (off) => {
  selection.value = without(selection.value, off)
})

const selectionText = computed(() => {
  const n = selection.value.size
  if (n === 0) return hiddenZones.value.size > 0 ? 'All visible zones' : 'Whole device'
  return n === 1 ? '1 LED selected' : `${n} LEDs selected`
})

function onColor(hex: string) {
  if (!device.value) return
  pickerColor.value = hex
  // Only colour LEDs take the colour; a selection of brightness-only LEDs alone changes nothing.
  const target = without(selection.value, fixedLeds.value)
  if (selection.value.size > 0 && target.size === 0) return
  const painted = paint(colors.value, target, hex, colorScope.value)
  write({ kind: 'direct', colors: blackOut(painted, hiddenLeds.value) })
}

/** All LEDs white for a device that was never set (an all-black buffer), so a first edit does not look like "off";
 * `null` otherwise. Used by "Static" in the mode menu and by the level sliders. */
function whiteIfNeverSet(d: DeviceView): string[] | null {
  return d.setting === null && d.colors.every((c) => c === BLACK) ? d.colors.map(() => '#FFFFFF') : null
}

// ---- per-LED white levels (ADR 0001 #41) ----------------------------------------------------------------------

/** One slider per LED of the visible brightness-only zones, named after the zone (and the LED in a longer zone). */
const levelRows = computed(() => {
  const d = desc.value
  if (!d) return []
  return d.zones.flatMap((zone, index) =>
    !zone.brightness_only || hiddenZones.value.has(index)
      ? []
      : zoneIndices(zone).map((led) => ({
          led,
          label: zone.len === 1 ? zone.name : `${zone.name}: ${d.leds[led]?.name ?? `LED ${led - zone.start + 1}`}`,
        })),
  )
})

/**
 * What the level sliders show and a level change starts from: the latest local edit; while released, the device's
 * own colours as the preview shows them, with its brightness-only LEDs at the level the preview draws (their colour
 * times the own mode's brightness, `previewIntensity`), since the engine buffer can still hold what CaseControl
 * showed before the release; else white on a device that was never set; else the engine's buffer.
 */
const levelColors = computed<readonly string[]>(() => {
  const d = device.value
  if (!d) return []
  if (intent.value?.kind === 'direct') return intent.value.colors
  const own = d.status.state === 'released' ? shownColors(d) : null
  if (own) {
    const k = previewIntensity.value
    return own.map((c, i) => (fixedLeds.value.has(i) ? whiteLevel(levelOf(c) * k) : c))
  }
  return whiteIfNeverSet(d) ?? d.colors
})

/** Sets one brightness-only LED's level; the other LEDs keep what the sliders and the preview show. */
function onLevel(led: number, percent: number) {
  if (!device.value) return
  const next = [...levelColors.value]
  next[led] = whiteLevel(percent)
  write({ kind: 'direct', colors: blackOut(next, hiddenLeds.value) })
}

// ---- mode menu -------------------------------------------------------------------------------------------------

const STATIC = 'static'
const DEFAULT = ''
const RELEASED = 'released'

/**
 * `'static'`, a hardware mode id, `'released'` while the device shows its own lighting, or `''` while it is driven
 * without a known setting. Any real choice differs from `'released'` and `''`, so it always takes control.
 */
const menuValue = computed(() => {
  const w = intent.value
  if (w) return w.kind === 'direct' ? STATIC : String(w.state.mode)
  const d = device.value
  if (!d) return DEFAULT
  if (d.status.state === 'released') return RELEASED
  const hw = currentHardwareMode(d)
  if (hw !== null) return String(hw)
  return d.setting ? STATIC : DEFAULT
})

/** The menu entry for the device's own lighting, with its mode when that is known. */
const releasedLabel = computed(() => {
  const mode = device.value ? ownLighting(device.value).mode : null
  return mode ? `Own lighting (${mode.name})` : 'Own lighting'
})

/** Colour editing is shown in static mode, and also while the device runs on its own (a colour takes control). */
const editingStatic = computed(() => hasDirect.value && [STATIC, DEFAULT, RELEASED].includes(menuValue.value))
/** The colour picker and LED selection: static editing on a device with colour LEDs. */
const editingColors = computed(() => editingStatic.value && !brightnessOnly.value)

const activeMode = computed(() => modes.value.find((m) => String(m.id) === menuValue.value))
/** The state the mode controls edit: the latest local one while edits are pending, else the device's. */
const activeState = computed(() => {
  const w = intent.value
  if (w?.kind === 'mode' && activeMode.value && w.state.mode === activeMode.value.id) return w.state
  return device.value && activeMode.value ? stateFor(device.value, activeMode.value) : null
})

function sendMode(state: ModeState) {
  write({ kind: 'mode', state })
}

function onMenu(value: string) {
  const d = device.value
  if (!d || value === menuValue.value) return
  if (value === STATIC) {
    // A device that was never set has an all-black buffer; start from white so "Static" does not look like "off".
    const start = whiteIfNeverSet(d) ?? [...colors.value]
    write({ kind: 'direct', colors: blackOut(start, hiddenLeds.value) })
    syncPicker()
    return
  }
  const mode = modes.value.find((m) => String(m.id) === value)
  if (mode) sendMode(stateFor(d, mode))
}

// ---- brightness (ADR 0001 #33) ---------------------------------------------------------------------------------

/** The hardware mode a brightness change acts on (the one being set, driven, or taken over with), if any. */
const dimMode = computed(() => {
  if (intent.value) return activeMode.value ?? null
  return device.value ? brightnessMode(device.value) : null
})
/** In a hardware mode without a brightness parameter the device brightness has no effect. */
const brightnessUnsupported = computed(() => dimMode.value !== null && !hasCap(dimMode.value, 'BRIGHTNESS'))
/** A device the engine cannot take over (no setting, no known own lighting, nothing the default colour fits): there
 * is nothing to dim. */
const nothingToDim = computed(() => intent.value === null && device.value !== undefined && !canTakeOver(device.value))
const brightnessHint = computed(() =>
  nothingToDim.value
    ? 'Set a colour or mode first.'
    : brightnessUnsupported.value
      ? 'This mode has no brightness setting.'
      : undefined,
)

async function sendBrightness(percent: number, persist: boolean) {
  await setDeviceBrightness(props.id, percent)
  if (persist) await refreshDevices()
}

function brightnessFailed(e: unknown) {
  error.value = errorText(e)
  void refreshDevices()
}

// ---- zones -----------------------------------------------------------------------------------------------------

const resizable = computed(() =>
  (desc.value?.zones ?? [])
    .map((zone, index) => ({ zone, index, hidden: hiddenZones.value.has(index) }))
    .filter((z) => z.zone.resizable !== null && (deviceState.showHiddenZones || !z.hidden)),
)

// ---- hiding zones ----------------------------------------------------------------------------------------------

const menuAt = ref<{ x: number; y: number } | null>(null)
const menuItems = ref<MenuItem[]>([])

function onZoneMenu(index: number, e: MouseEvent) {
  const zone = desc.value?.zones[index]
  if (!zone || busy.value) return
  const hidden = hiddenZones.value.has(index)
  menuItems.value = [
    { label: hidden ? 'Unhide zone' : 'Hide zone', action: () => void setZoneHiddenTo(zone.name, !hidden) },
  ]
  menuAt.value = menuPoint(e)
}

async function setZoneHiddenTo(zoneName: string, hidden: boolean) {
  busy.value = true
  error.value = null
  notice.value = null
  try {
    // Send pending colour edits first, so the backend masks the latest buffer.
    await sender.flush()
    dropIntent()
    await hideZone(props.id, zoneName, hidden)
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
    void refreshDevices()
  }
}

/** `len` is already clamped to the zone's range by the NumberField. */
async function onResize(zone: Zone, index: number, len: number) {
  if (!zone.resizable || len === zone.len) return
  busy.value = true
  error.value = null
  try {
    await sender.flush()
    const view = await resizeZone(props.id, index, len)
    dropIntent()
    replaceDevice(view)
    selection.value = clampSelection(selection.value, view.description.leds.length)
  } catch (e) {
    fail(e)
  } finally {
    busy.value = false
  }
}

// ---- actions ---------------------------------------------------------------------------------------------------

const saveable = computed(() => (desc.value ? canSaveToHardware(desc.value) : false))
const confirmSave = ref(false)

async function onRelease() {
  busy.value = true
  error.value = null
  notice.value = null
  try {
    // Release is the newest action: drop queued edits instead of sending them first.
    sender.cancel()
    await sender.flush()
    dropIntent()
    await releaseDevice(props.id)
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
    void refreshDevices()
  }
}

async function onSave() {
  busy.value = true
  error.value = null
  notice.value = null
  try {
    await sender.flush()
    await saveToHardware(props.id)
    notice.value = 'Saved to the device.'
    confirmSave.value = false
  } catch (e) {
    error.value = errorText(e)
    confirmSave.value = false
  } finally {
    busy.value = false
  }
}

const selectAll = () => device.value && onSelect(new Set(colorScope.value))

/**
 * The output factor brightness-only LEDs are drawn at: device x master brightness while driven; while released, the
 * own mode's brightness within its range (the device's own lighting does not follow the app's levels).
 */
const previewIntensity = computed(() => {
  const d = device.value
  if (!d) return 1
  if (d.status.state === 'released') {
    const hw = d.hardware
    const mode = hw ? d.description.modes.find((m) => m.id === hw.mode) : undefined
    if (!hw || hw.brightness === null || !mode || !hasCap(mode, 'BRIGHTNESS')) return 1
    const max = mode.brightness?.end ?? 100
    return max > 0 ? hw.brightness / max : 1
  }
  return (d.brightness / 100) * (deviceState.lighting.master_brightness / 100)
})
</script>

<template>
  <section class="page wide">
    <template v-if="device && desc">
      <PageHeader :title="desc.info.name" :back="{ to: '/lighting', label: 'Lighting' }">
        <template #badge>
          <StatusBadge v-if="deviceHidden" status="hidden">Hidden</StatusBadge>
          <StatusBadge v-else :status="statusOf(device)">{{ STATUS_LABEL[statusOf(device)] }}</StatusBadge>
        </template>
        <template #actions>
          <Switch v-model="deviceState.showHiddenZones" label="Show hidden" compact />
        </template>
        <template #controls>
          <BrightnessSlider
            label="Brightness"
            :value="device.brightness"
            :send="sendBrightness"
            :disabled="locked || brightnessUnsupported || nothingToDim"
            :hint="brightnessHint"
            @error="brightnessFailed"
          />
        </template>
      </PageHeader>

      <Notice v-if="deviceHidden">
        This device is hidden, so CaseControl does not control it.
        <template #details>
          To unhide it, turn on Show hidden on the Lighting page and right-click the device.
        </template>
      </Notice>

      <Notice v-if="device.status.state === 'faulted'" variant="error">
        CaseControl stopped talking to this device after an error: {{ device.status.error }}
        <template #details>
          Close other software that uses it or re-plug it, then press Rescan on the Lighting page to retry.
        </template>
      </Notice>

      <div v-if="editingColors" class="row selection">
        <span>{{ selectionText }}</span>
        <Button variant="ghost" size="sm" :disabled="locked" @click="selectAll">All</Button>
        <Button variant="ghost" size="sm" :disabled="locked" @click="onSelect(EMPTY)">None</Button>
        <span class="muted small">Click an LED, drag across several, Ctrl-click to add. Right-click a zone to hide it.</span>
      </div>

      <Card padding="lg">
        <DevicePreview
          variant="stage"
          :description="desc"
          :colors="previewColors"
          :selection="selection"
          :interactive="editingColors && !locked"
          :hidden-zones="hiddenZones"
          :show-hidden="deviceState.showHiddenZones"
          :intensity="previewIntensity"
          @select="onSelect"
          @zone-menu="onZoneMenu"
        />
      </Card>

      <div class="panel">
        <Field v-slot="{ id }" label="Mode">
          <Select :id="id" :model-value="menuValue" :disabled="locked || busy" @update:model-value="onMenu">
            <option v-if="menuValue === DEFAULT" :value="DEFAULT" disabled>Off</option>
            <option v-if="menuValue === RELEASED" :value="RELEASED" disabled>{{ releasedLabel }}</option>
            <option v-if="hasDirect" :value="STATIC">Static (per LED)</option>
            <option v-for="m in modes" :key="m.id" :value="String(m.id)">{{ m.name }}</option>
          </Select>
        </Field>

        <ColorPicker v-if="editingColors" :model-value="pickerColor" :disabled="locked" @update:model-value="onColor" />

        <div v-if="editingStatic && levelRows.length > 0" class="levels">
          <Field v-for="r in levelRows" :key="r.led" :label="r.label">
            <template #default="{ id }">
              <Slider
                :id="id"
                :model-value="levelOf(levelColors[r.led] ?? BLACK)"
                :min="0"
                :max="100"
                :disabled="locked"
                @update:model-value="onLevel(r.led, $event)"
              />
            </template>
            <template #value>{{ levelOf(levelColors[r.led] ?? BLACK) }} %</template>
          </Field>
        </div>

        <ModeControls
          v-else-if="!editingStatic && activeMode && activeState"
          :mode="activeMode"
          :state="activeState"
          :disabled="locked || busy"
          @update="sendMode"
        />
      </div>

      <Notice v-if="error" variant="error">{{ error }}</Notice>
      <Notice v-if="notice">{{ notice }}</Notice>

      <div v-if="resizable.length > 0" class="section">
        <h2>LED count</h2>
        <div class="zones">
          <Field
            v-for="z in resizable"
            v-slot="{ id }"
            :key="z.index"
            :label="z.zone.name"
            :class="{ 'hidden-zone': z.hidden }"
          >
            <NumberField
              :id="id"
              :model-value="z.zone.len"
              :min="z.zone.resizable?.start ?? 0"
              :max="z.zone.resizable?.end ?? 0"
              :disabled="locked || busy || z.hidden"
              @update:model-value="onResize(z.zone, z.index, $event)"
            />
            <StatusBadge v-if="z.hidden" status="hidden">Hidden</StatusBadge>
          </Field>
        </div>
      </div>

      <div class="section">
        <h2>Device</h2>
        <div class="row">
          <Button :disabled="locked || busy" @click="onRelease">Release</Button>
          <Button v-if="saveable" variant="danger" :disabled="locked || busy" @click="confirmSave = true">
            Save to device
          </Button>
        </div>
        <KeyValueList class="small facts">
          <dt>Kind</dt>
          <dd>{{ kindLabel(desc.info.kind) }}</dd>
          <dt>Vendor</dt>
          <dd>{{ desc.info.vendor }}</dd>
          <dt>Id</dt>
          <dd class="mono">{{ desc.info.id }}</dd>
          <template v-if="desc.info.firmware">
            <dt>Firmware</dt>
            <dd class="mono">{{ desc.info.firmware }}</dd>
          </template>
          <dt>LEDs</dt>
          <dd class="count">{{ desc.leds.length }}</dd>
        </KeyValueList>
      </div>

      <ContextMenu :at="menuAt" :items="menuItems" @close="menuAt = null" />

      <ConfirmDialog
        :open="confirmSave"
        title="Save to device?"
        confirm-label="Save to device"
        danger
        :busy="busy"
        @confirm="onSave"
        @cancel="confirmSave = false"
      >
        <p>This writes the current mode into the device's flash memory, so the device shows it on its own, even without CaseControl.</p>
        <p>Flash memory wears with every write. Save only a setup you want to keep.</p>
      </ConfirmDialog>
    </template>

    <template v-else-if="deviceState.loaded">
      <PageHeader :back="{ to: '/lighting', label: 'Lighting' }" />
      <EmptyState title="Device not found" :level="1">
        This device is not connected any more, or its detector is switched off in Settings.
      </EmptyState>
    </template>
  </section>
</template>

<style scoped>
.selection {
  gap: var(--space-4);
  margin-bottom: var(--space-4);
}

.panel {
  display: flex;
  flex-direction: column;
  gap: var(--space-6);
  margin: var(--space-10) 0;
}

.levels {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.facts {
  margin-top: var(--space-6);
}

.zones {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.hidden-zone {
  opacity: 0.45;
}

.count {
  font-variant-numeric: tabular-nums;
}
</style>
