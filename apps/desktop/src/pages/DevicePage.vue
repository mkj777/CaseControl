<script setup lang="ts">
// Device view (ADR 0001 #18-#22, #25): a large preview where LEDs are selected, the mode menu, the colour picker
// (static mode) or the mode's controls (hardware modes), zone sizes and the device actions.
//
// Every colour and mode change goes through one ordered sender, so the last user action always wins. While changes
// are queued or in flight, the page shows its own latest intent instead of the store, so a refresh that raced an
// edit cannot roll the controls back; the intent is dropped once the sender is idle and a fresh reload has landed.
import { computed, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import ColorPicker from '../components/ColorPicker.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import DevicePreview from '../components/DevicePreview.vue'
import ModeControls from '../components/ModeControls.vue'
import {
  errorText,
  releaseDevice,
  resizeZone,
  saveToHardware,
  setDirect,
  setMode,
  type ModeState,
  type Zone,
} from '../ipc'
import { BLACK } from '../lib/color'
import {
  STATUS_LABEL,
  canSaveToHardware,
  currentHardwareMode,
  hardwareModes,
  kindLabel,
  stateFor,
  statusOf,
} from '../lib/device'
import { EMPTY, clampSelection, paint, range, selectionColor, type Selection } from '../lib/selection'
import { latestSender } from '../lib/throttle'
import { deviceState, echoColors, findDevice, refreshDevices, replaceDevice } from '../stores/devices'

const props = defineProps<{ id: string }>()

const device = computed(() => findDevice(props.id))
const desc = computed(() => device.value?.description)
const faulted = computed(() => device.value !== undefined && statusOf(device.value) === 'faulted')
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

/** Colours to show and paint on: the latest local edit, else the engine's buffer. */
const colors = computed<readonly string[]>(() =>
  intent.value?.kind === 'direct' ? intent.value.colors : (device.value?.colors ?? []),
)

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

const selectionText = computed(() => {
  const n = selection.value.size
  if (n === 0) return 'Whole device'
  return n === 1 ? '1 LED selected' : `${n} LEDs selected`
})

function onColor(hex: string) {
  if (!device.value) return
  pickerColor.value = hex
  write({ kind: 'direct', colors: paint(colors.value, selection.value, hex) })
}

// ---- mode menu -------------------------------------------------------------------------------------------------

const STATIC = 'static'
const DEFAULT = ''
const RELEASED = 'released'

/**
 * `'static'`, a hardware mode id, `'released'` after a release, or `''` while the device shows its own hardware
 * mode and was never set. Any real choice differs from `'released'` and `''`, so it always takes control.
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

/** Colour editing is shown in static mode, and also while the device runs on its own (a colour takes control). */
const editingStatic = computed(() => hasDirect.value && [STATIC, DEFAULT, RELEASED].includes(menuValue.value))

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
    const fresh = d.setting === null && d.colors.every((c) => c === BLACK)
    write({ kind: 'direct', colors: fresh ? d.colors.map(() => '#FFFFFF') : [...colors.value] })
    syncPicker()
    return
  }
  const mode = modes.value.find((m) => String(m.id) === value)
  if (mode) sendMode(stateFor(d, mode))
}

// ---- zones -----------------------------------------------------------------------------------------------------

const resizable = computed(() =>
  (desc.value?.zones ?? []).map((zone, index) => ({ zone, index })).filter((z) => z.zone.resizable !== null),
)

async function onResize(zone: Zone, index: number, raw: string) {
  const r = zone.resizable
  const len = Math.round(Number(raw))
  if (!r || !Number.isFinite(len) || len === zone.len) return
  if (len < r.start || len > r.end) {
    error.value = `${zone.name} takes ${r.start} to ${r.end} LEDs.`
    return
  }
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
    notice.value = 'The device is back in its own hardware mode until you change something here.'
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

const selectAll = () => device.value && onSelect(new Set(range(0, device.value.description.leds.length)))
</script>

<template>
  <section class="page">
    <RouterLink to="/devices" class="back">← Devices</RouterLink>

    <template v-if="device && desc">
      <header class="head">
        <div>
          <h1>{{ desc.info.name }}</h1>
          <p class="muted">{{ kindLabel(desc.info.kind) }} · {{ desc.info.vendor }} · {{ STATUS_LABEL[statusOf(device)] }}</p>
        </div>
      </header>

      <div v-if="device.status.state === 'faulted'" class="notice">
        <p>CaseControl stopped talking to this device after an error: {{ device.status.error }}</p>
        <p class="muted small">
          Close other software that uses it or re-plug it, then press Rescan on the Devices page to retry.
        </p>
      </div>

      <div class="stage">
        <DevicePreview
          :description="desc"
          :colors="colors"
          :selection="selection"
          :interactive="editingStatic && !faulted"
          @select="onSelect"
        />
      </div>

      <div class="panel">
        <label class="mode row">
          <span class="muted">Mode</span>
          <select
            :value="menuValue"
            :disabled="faulted || busy"
            @change="onMenu(($event.target as HTMLSelectElement).value)"
          >
            <option v-if="menuValue === DEFAULT" :value="DEFAULT" disabled>Device default</option>
            <option v-if="menuValue === RELEASED" :value="RELEASED" disabled>Hardware (released)</option>
            <option v-if="hasDirect" :value="STATIC">Static (per LED)</option>
            <option v-for="m in modes" :key="m.id" :value="String(m.id)">{{ m.name }}</option>
          </select>
        </label>

        <p v-if="menuValue === DEFAULT || menuValue === RELEASED" class="muted">
          The device shows its own hardware mode until you pick a colour or a mode here.
        </p>

        <template v-if="editingStatic">
          <div class="row selection">
            <span>{{ selectionText }}</span>
            <button type="button" class="link" :disabled="faulted" @click="selectAll">All</button>
            <button type="button" class="link" :disabled="faulted" @click="onSelect(EMPTY)">None</button>
            <span class="muted small">Click an LED, drag across several, Ctrl-click to add.</span>
          </div>
          <ColorPicker :model-value="pickerColor" :disabled="faulted" @update:model-value="onColor" />
        </template>

        <ModeControls
          v-else-if="activeMode && activeState"
          :mode="activeMode"
          :state="activeState"
          :disabled="faulted || busy"
          @update="sendMode"
        />
      </div>

      <p v-if="error" class="notice">{{ error }}</p>
      <p v-if="notice" class="notice">{{ notice }}</p>

      <div v-if="resizable.length > 0" class="section">
        <h2>LED count</h2>
        <p class="muted block">Set how many LEDs are connected to each header.</p>
        <div class="zones">
          <label v-for="z in resizable" :key="z.index" class="zone row">
            <span class="zone-name">{{ z.zone.name }}</span>
            <input
              type="number"
              :min="z.zone.resizable?.start"
              :max="z.zone.resizable?.end"
              :value="z.zone.len"
              :disabled="faulted || busy"
              @change="onResize(z.zone, z.index, ($event.target as HTMLInputElement).value)"
            />
            <span class="muted small">{{ z.zone.resizable?.start }} to {{ z.zone.resizable?.end }}</span>
          </label>
        </div>
      </div>

      <div class="section">
        <h2>Device</h2>
        <div class="row">
          <button type="button" :disabled="faulted || busy" @click="onRelease">Release</button>
          <button v-if="saveable" type="button" :disabled="faulted || busy" @click="confirmSave = true">
            Save to device
          </button>
        </div>
        <p class="muted small block">
          Release hands the device back to its own hardware mode. CaseControl does this for every device when it quits.
        </p>
        <dl class="details small">
          <dt>Id</dt>
          <dd>{{ desc.info.id }}</dd>
          <template v-if="desc.info.firmware">
            <dt>Firmware</dt>
            <dd>{{ desc.info.firmware }}</dd>
          </template>
          <dt>LEDs</dt>
          <dd>{{ desc.leds.length }}</dd>
        </dl>
      </div>

      <ConfirmDialog
        :open="confirmSave"
        title="Save to device?"
        confirm-label="Save to device"
        :busy="busy"
        @confirm="onSave"
        @cancel="confirmSave = false"
      >
        <p>This writes the current mode into the device's flash memory, so the device shows it on its own, even without CaseControl.</p>
        <p>Flash memory wears with every write. Save only a setup you want to keep.</p>
      </ConfirmDialog>
    </template>

    <div v-else-if="deviceState.loaded" class="gone">
      <h1>Device not found</h1>
      <p class="muted">This device is not connected any more, or its detector is switched off in Settings.</p>
    </div>
  </section>
</template>

<style scoped>
.back {
  display: inline-block;
  margin-bottom: var(--space-6);
  color: var(--color-text-muted);
  text-decoration: none;
}

.back:hover {
  color: var(--color-text);
}

.head {
  margin-bottom: var(--space-8);
}

.notice {
  margin-bottom: var(--space-6);
}

.stage {
  padding: var(--space-8);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-lg);
  background: var(--color-surface);
}

.panel {
  display: flex;
  flex-direction: column;
  gap: var(--space-6);
  margin: var(--space-8) 0;
}

.mode select {
  min-width: 240px;
}

.selection {
  gap: var(--space-4);
}

.block {
  margin: var(--space-3) 0 var(--space-5);
}

.zones {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.zone-name {
  min-width: 140px;
}

.zone input {
  width: 96px;
}

.details {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: var(--space-1) var(--space-6);
  margin: var(--space-6) 0 0;
  color: var(--color-text-muted);
}

.details dd {
  margin: 0;
  font-family: ui-monospace, Consolas, monospace;
}

.gone {
  padding: var(--space-8) 0;
}
</style>
