// Pure helpers over the device model: labels, mode capabilities and default mode states.

import type { DeviceDescription, DeviceError, DeviceKind, DeviceView, Direction, Mode, ModeCap, ModeState, Zone } from '../ipc'
import { BLACK } from './color'

export type StatusName = 'active' | 'released' | 'faulted'

export function statusOf(view: DeviceView): StatusName {
  return view.status.state
}

export const STATUS_LABEL: Record<StatusName, string> = {
  active: 'Active',
  released: 'Released',
  faulted: 'Faulted',
}

const KIND_LABEL: Partial<Record<DeviceKind, string>> = {
  Dram: 'Memory',
  Gpu: 'Graphics card',
  LedStrip: 'LED strip',
  Mousemat: 'Mouse mat',
  HeadsetStand: 'Headset stand',
  Unknown: 'Device',
}

export function kindLabel(kind: DeviceKind): string {
  return KIND_LABEL[kind] ?? kind
}

export const FAMILY_LABEL: Record<string, string> = {
  msi_mystic_light: 'MSI Mystic Light',
  wooting: 'Wooting',
  logitech: 'Logitech',
  nvidia_illumination: 'NVIDIA Illumination',
}

export const familyLabel = (id: string): string => FAMILY_LABEL[id] ?? id

/** The capability names in a `ModeCaps` JSON string (`"SPEED | BRIGHTNESS"`). */
export function capsOf(mode: Mode): Set<ModeCap> {
  return new Set(
    mode.caps
      .split('|')
      .map((c) => c.trim())
      .filter((c) => c.length > 0) as ModeCap[],
  )
}

export const hasCap = (mode: Mode, cap: ModeCap): boolean => capsOf(mode).has(cap)

/** Directions a mode accepts, in a stable order. */
export function directionsOf(mode: Mode): Direction[] {
  const caps = capsOf(mode)
  const out: Direction[] = []
  if (caps.has('DIRECTION_LR')) out.push('Left', 'Right')
  if (caps.has('DIRECTION_UD')) out.push('Up', 'Down')
  if (caps.has('DIRECTION_HV')) out.push('Horizontal', 'Vertical')
  return out
}

/** Lower and upper bound of a range that may be declared in either order. */
export function bounds(r: { start: number; end: number }): [number, number] {
  return [Math.min(r.start, r.end), Math.max(r.start, r.end)]
}

/** The hardware modes offered next to "Static (per LED)": every mode except the direct mode. */
export function hardwareModes(desc: DeviceDescription): Mode[] {
  return desc.modes.filter((m) => m.id !== desc.direct_mode)
}

/** Id of the hardware mode the device shows, or `null` for direct colours / no setting. */
export function currentHardwareMode(view: DeviceView): number | null {
  const s = view.setting
  if (s?.kind !== 'mode' || s.state.mode === view.description.direct_mode) return null
  return s.state.mode
}

/** Whether any mode can be saved to the device's flash on request. */
export function canSaveToHardware(desc: DeviceDescription): boolean {
  return desc.modes.some((m) => hasCap(m, 'SAVE_MANUAL'))
}

/**
 * A valid starting state for `mode`: speed in the middle of its range, brightness at the top, the first direction,
 * and as few mode colours as allowed (at least one when allowed), taken from `seed` and padded with white. Per-LED
 * modes get the current buffer.
 */
export function defaultModeState(mode: Mode, ledColors: readonly string[], seed: readonly string[] = []): ModeState {
  const caps = capsOf(mode)
  let speed: number | null = null
  if (caps.has('SPEED') && mode.speed) {
    const [lo, hi] = bounds(mode.speed)
    speed = Math.round((lo + hi) / 2)
  }
  let brightness: number | null = null
  if (caps.has('BRIGHTNESS') && mode.brightness) brightness = bounds(mode.brightness)[1]
  const direction = directionsOf(mode)[0] ?? null
  let colors: string[] = []
  if (mode.color_mode === 'PerLed') colors = [...ledColors]
  else if (mode.color_mode === 'ModeSpecific' && mode.colors) {
    const [lo, hi] = bounds(mode.colors)
    const n = Math.min(hi, Math.max(lo, 1))
    colors = Array.from({ length: n }, (_, i) => seed[i] ?? '#FFFFFF')
  }
  return { mode: mode.id, speed, brightness, direction, colors }
}

/** The mode state the device shows if it is `mode`, else a default one. */
export function stateFor(view: DeviceView, mode: Mode): ModeState {
  const s = view.setting
  if (s?.kind === 'mode' && s.state.mode === mode.id) return s.state
  const seed = view.colors.filter((c) => c !== BLACK).slice(0, 1)
  return defaultModeState(mode, view.colors, seed)
}

export interface MatrixCell {
  key: number
  /** Device-buffer LED index, or `null` for an empty cell. */
  led: number | null
}

/** Row-major cells of a matrix zone with zone-relative indices turned into device-buffer indices. */
export function matrixCells(zone: Zone): { width: number; cells: MatrixCell[] } | null {
  if (typeof zone.kind === 'string') return null
  const { width, map } = zone.kind.Matrix
  return {
    width,
    cells: map.map((rel, key) => ({ key, led: rel === null || rel >= zone.len ? null : zone.start + rel })),
  }
}

/** Indices of the zones whose names are in `names` (hidden zones). */
export function zoneIndicesNamed(desc: DeviceDescription, names: readonly string[]): Set<number> {
  const wanted = new Set(names)
  const out = new Set<number>()
  desc.zones.forEach((z, i) => {
    if (wanted.has(z.name)) out.add(i)
  })
  return out
}

/** Device-buffer LED indices of the zones in `zones`. */
export function ledsOfZones(desc: DeviceDescription, zones: ReadonlySet<number>): Set<number> {
  const out = new Set<number>()
  desc.zones.forEach((z, i) => {
    if (!zones.has(i)) return
    for (let led = z.start; led < z.start + z.len; led++) out.add(led)
  })
  return out
}

/** Device-buffer LED indices outside the zones in `hiddenZones`, in order. */
export function visibleLeds(desc: DeviceDescription, hiddenZones: ReadonlySet<number>): number[] {
  const hidden = ledsOfZones(desc, hiddenZones)
  return desc.leds.map((_, i) => i).filter((i) => !hidden.has(i))
}

/** Readable text of a driver error. */
export function deviceErrorText(e: DeviceError): string {
  if (e === 'Unsupported') return 'Not supported by this device'
  if (e === 'Disconnected') return 'Device disconnected'
  if (e === 'Timeout') return 'Device did not answer'
  const [kind, text] = Object.entries(e)[0] ?? ['Error', '']
  const label: Record<string, string> = {
    InvalidArgument: 'Invalid argument',
    Transport: 'Transport error',
    Protocol: 'Protocol error',
    Faulted: 'Faulted',
  }
  return `${label[kind] ?? kind}: ${text}`
}
