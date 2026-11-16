// Shared device state for the Devices overview and the device view. Refreshes from `list_devices` and on every
// engine event, so hot-plug, faults and tray actions show up without a manual reload.

import { reactive } from 'vue'
import { errorText, listDevices, on, rescan, type DetectFailure, type DeviceId, type DeviceList, type DeviceView } from '../ipc'

interface DeviceState {
  devices: DeviceView[]
  failures: DetectFailure[]
  engineError: string | null
  /** Last failure of `list_devices` itself. */
  error: string | null
  loaded: boolean
  scanning: boolean
}

export const deviceState = reactive<DeviceState>({
  devices: [],
  failures: [],
  engineError: null,
  error: null,
  loaded: false,
  scanning: false,
})

function take(list: DeviceList) {
  deviceState.devices = list.devices
  deviceState.failures = list.failures
  deviceState.engineError = list.engine_error
  deviceState.error = null
  deviceState.loaded = true
}

let pending: Promise<void> | null = null
let again = false

/** Reloads the device list; overlapping calls collapse into one extra reload. */
export function refreshDevices(): Promise<void> {
  if (pending) {
    again = true
    return pending
  }
  pending = listDevices()
    .then(take, (e) => {
      deviceState.error = errorText(e)
      deviceState.loaded = true
    })
    .finally(() => {
      pending = null
      if (again) {
        again = false
        void refreshDevices()
      }
    })
  return pending
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
