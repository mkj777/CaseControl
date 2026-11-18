// Shared device state for the Lighting overview and the device view. Refreshes from `list_devices` and on every
// engine event, so hot-plug, faults and tray actions show up without a manual reload.

import { reactive } from 'vue'
import {
  errorText,
  listDevices,
  on,
  rescan,
  setDeviceHidden,
  setZoneHidden,
  type DetectFailure,
  type DeviceId,
  type DeviceList,
  type DeviceView,
  type HiddenEntry,
  type LightingView,
} from '../ipc'

interface DeviceState {
  devices: DeviceView[]
  failures: DetectFailure[]
  engineError: string | null
  /** What the user hid, by device id. */
  hidden: Record<DeviceId, HiddenEntry>
  /** Master brightness (ADR 0001 #33). */
  lighting: LightingView
  /** Session-only "Show hidden" toggles (ADR 0001 #27): overview (devices) and device view (zones). */
  showHiddenDevices: boolean
  showHiddenZones: boolean
  /** Last failure of `list_devices` itself. */
  error: string | null
  loaded: boolean
  scanning: boolean
}

export const deviceState = reactive<DeviceState>({
  devices: [],
  failures: [],
  engineError: null,
  hidden: {},
  lighting: { master_brightness: 100 },
  showHiddenDevices: false,
  showHiddenZones: false,
  error: null,
  loaded: false,
  scanning: false,
})

function take(list: DeviceList) {
  deviceState.devices = list.devices
  deviceState.failures = list.failures
  deviceState.engineError = list.engine_error
  deviceState.hidden = list.hidden
  deviceState.lighting = list.lighting
  deviceState.error = null
  deviceState.loaded = true
}

let pending: Promise<void> | null = null
let next: Promise<void> | null = null

function load(): Promise<void> {
  return listDevices().then(take, (e) => {
    deviceState.error = errorText(e)
    deviceState.loaded = true
  })
}

/**
 * Reloads the device list. The returned promise resolves after a load that started no earlier than this call, so
 * its data reflects every command that completed before the call. Overlapping calls share one follow-up load.
 */
export function refreshDevices(): Promise<void> {
  if (!pending) {
    pending = load().finally(() => {
      pending = null
    })
    return pending
  }
  if (!next) {
    next = pending.then(() => {
      next = null
      return refreshDevices()
    })
  }
  return next
}

export async function rescanDevices(): Promise<void> {
  deviceState.scanning = true
  try {
    take(await rescan())
  } catch (e) {
    deviceState.error = errorText(e)
  } finally {
    deviceState.scanning = false
  }
}

export const isDeviceHidden = (id: DeviceId): boolean => deviceState.hidden[id]?.device ?? false

export const hiddenZoneNames = (id: DeviceId): readonly string[] => deviceState.hidden[id]?.zones ?? []

/** Hides or unhides a device; the backend releases a device it hides. Rejects with the error text. */
export async function hideDevice(id: DeviceId, hidden: boolean): Promise<void> {
  take(await setDeviceHidden(id, hidden))
}

/** Hides or unhides a zone by name; the backend switches a hidden zone off in direct colour. */
export async function hideZone(id: DeviceId, zoneName: string, hidden: boolean): Promise<void> {
  take(await setZoneHidden(id, zoneName, hidden))
}

export function findDevice(id: DeviceId): DeviceView | undefined {
  return deviceState.devices.find((d) => d.description.info.id === id)
}

/** Replaces one device's view (e.g. after a zone resize returned the new layout). */
export function replaceDevice(view: DeviceView) {
  const i = deviceState.devices.findIndex((d) => d.description.info.id === view.description.info.id)
  if (i >= 0) deviceState.devices[i] = view
}

/** Local echo of a colour edit so the preview follows the pointer before the engine answers. */
export function echoColors(id: DeviceId, colors: string[]) {
  const d = findDevice(id)
  if (!d) return
  d.colors = colors
  d.setting = { kind: 'direct', colors }
  if (d.status.state === 'released') d.status = { state: 'active' }
}

let listening = false

/** Subscribes to the engine events once for the app's lifetime. */
export function listenForDeviceEvents() {
  if (listening) return
  listening = true
  const refresh = () => void refreshDevices()
  for (const ev of [
    'device-added',
    'device-removed',
    'device-faulted',
    'device-state-changed',
    'devices-changed',
    'profiles-changed',
  ] as const) {
    void on(ev, refresh)
  }
}
