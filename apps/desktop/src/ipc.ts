// Typed wrappers around the Tauri commands and events of `src-tauri`. The types mirror the serde JSON of the Rust
// side exactly; `commands.rs` has a test (`device_list_json_shape`) that pins the device shapes.
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// ---- cc-core ---------------------------------------------------------------------------------------------------

/** `Rgb`: upper-case `"#RRGGBB"`. */
export type Rgb = string

/** `DeviceId`: e.g. `"hid:1462:7d75"`, `"nvapi:10de:2c02:10de:2095"`. */
export type DeviceId = string

/** `std::ops::RangeInclusive`. */
export interface RangeInclusive {
  start: number
  end: number
}

export type Bus = 'Hid' | 'NvApi' | 'Smbus'

export type DeviceKind =
  | 'Motherboard'
  | 'Dram'
  | 'Gpu'
  | 'Cooler'
  | 'LedStrip'
  | 'Keyboard'
  | 'Mouse'
  | 'Mousemat'
  | 'Headset'
  | 'HeadsetStand'
  | 'Gamepad'
  | 'Light'
  | 'Speaker'
  | 'Virtual'
  | 'Storage'
  | 'Case'
  | 'Microphone'
  | 'Accessory'
  | 'Keypad'
  | 'Laptop'
  | 'Monitor'
  | 'Unknown'

export interface DeviceInfo {
  id: DeviceId
  name: string
  vendor: string
  kind: DeviceKind
  bus: Bus
  location: string
  serial: string | null
  firmware: string | null
}

/** `ZoneKind` (externally tagged): `map` cells are zone-relative LED indices or `null`. */
export type ZoneKind =
  | 'Single'
  | 'Linear'
  | { Matrix: { width: number; height: number; map: (number | null)[] } }

export interface Zone {
  name: string
  kind: ZoneKind
  start: number
  len: number
  resizable: RangeInclusive | null
}

export interface Led {
  name: string
}

export type ColorMode = 'None' | 'PerLed' | 'ModeSpecific' | 'Random'

export type Direction = 'Left' | 'Right' | 'Up' | 'Down' | 'Horizontal' | 'Vertical'

/** `ModeCaps` flag names; the JSON is the names joined with `" | "` (empty string for none). */
export type ModeCap =
  | 'SPEED'
  | 'DIRECTION_LR'
  | 'DIRECTION_UD'
  | 'DIRECTION_HV'
  | 'BRIGHTNESS'
  | 'PER_LED_COLOR'
  | 'MODE_SPECIFIC_COLOR'
  | 'RANDOM_COLOR'
  | 'SAVE_MANUAL'
  | 'SAVE_AUTOMATIC'

export interface Mode {
  id: number
  name: string
  caps: string
  color_mode: ColorMode
  speed: RangeInclusive | null
  brightness: RangeInclusive | null
  colors: RangeInclusive | null
}

export interface ModeState {
  mode: number
  speed: number | null
  brightness: number | null
  direction: Direction | null
  colors: Rgb[]
}

export interface DeviceDescription {
  info: DeviceInfo
  zones: Zone[]
  leds: Led[]
  modes: Mode[]
  direct_mode: number | null
  /** Milliseconds. */
  keepalive: number | null
}

/** `DeviceError` (externally tagged). */
export type DeviceError =
  | 'Unsupported'
  | 'Disconnected'
  | 'Timeout'
  | { InvalidArgument: string }
  | { Transport: string }
  | { Protocol: string }
  | { Faulted: string }

// ---- cc-config / cc-engine -------------------------------------------------------------------------------------

export type DeviceSetting = { kind: 'direct'; colors: Rgb[] } | { kind: 'mode'; state: ModeState }

export type DeviceStatus = { state: 'active' } | { state: 'released' } | { state: 'faulted'; error: string }

export interface DeviceView {
  description: DeviceDescription
  status: DeviceStatus
  setting: DeviceSetting | null
  /** The engine's colour buffer, one entry per LED. */
  colors: Rgb[]
  /** Family id of the backend that detected the device (e.g. `"wooting"`). */
  family: string
}

export interface DetectFailure {
  family: string
  location: string
  error: DeviceError
}

export interface ApplyFailure {
  id: DeviceId
  error: string
}

export interface ApplyReport {
  applied: DeviceId[]
  missing: DeviceId[]
  failed: ApplyFailure[]
}

// ---- cc-conflicts ----------------------------------------------------------------------------------------------

export type Severity = 'info' | 'low' | 'medium' | 'high'
export type Category = 'rgb_suite' | 'vendor_rgb' | 'hardware_monitor' | 'fan_control' | 'os_feature' | 'driver'
export type Access = 'hid' | 'smbus' | 'superio' | 'kernel_driver' | 'lamparray' | 'gpu_i2c'
export type ServiceState = 'running' | 'stopped' | { other: number }
export type ServiceKind = 'win32' | 'driver'
export type Hive = 'hklm_native' | 'hklm_wow6432' | 'hkcu'

export interface DynamicLighting {
  ambient_enabled: boolean
  foreground_controls: boolean
  providers: string[]
  device_count: number
}

/** `Evidence` (adjacently tagged with `type` / `data`). */
export type Evidence =
  | { type: 'process'; data: { pid: number; name: string; path: string | null } }
  | { type: 'service'; data: { name: string; state: ServiceState; kind: ServiceKind } }
  | { type: 'driver'; data: { name: string; state: ServiceState; path: string | null } }
  | { type: 'installed'; data: { display_name: string; version: string | null; hive: Hive } }
  | { type: 'path'; data: string }
  | { type: 'dynamic_lighting'; data: DynamicLighting }
  | { type: 'open_rgb_port' }

export interface AffectedDevice {
  id: DeviceId
  name: string
}

/** `FindingView`: a `cc_conflicts::Finding` plus the detected devices it affects. */
export interface Finding {
  entry_id: string
  name: string
  vendor: string
  category: Category
  severity: Severity
  verified: boolean
  access: Access[]
  affects: string[]
  note: string
  running: boolean
  installed: boolean
  evidence: Evidence[]
  affected_devices: AffectedDevice[]
}

export interface ConflictScan {
  findings: Finding[]
  errors: string[]
}

// ---- app -------------------------------------------------------------------------------------------------------

/** Mirrors `AppInfo` in `src-tauri/src/commands.rs`. */
export interface AppInfo {
  name: string
  version: string
  profile: 'debug' | 'release'
}

export interface DeviceList {
  devices: DeviceView[]
  failures: DetectFailure[]
  engine_error: string | null
}

export interface ReleaseResult {
  id: DeviceId
  error: string | null
}

export interface ProfileList {
  names: string[]
  active: string | null
}

export interface SettingsView {
  autostart: boolean
  start_minimized: boolean
  frame_rate: number
  /** Detector switch per family id. */
  detectors: Record<string, boolean>
  check_openrgb_port: boolean
  last_profile: string | null
  /** Why settings are not saved this session (unreadable settings file); changes are refused while set. */
  save_error: string | null
}

export interface SettingsPatch {
  autostart?: boolean
  start_minimized?: boolean
  frame_rate?: number
  detectors?: Record<string, boolean>
  check_openrgb_port?: boolean
}

// ---- commands --------------------------------------------------------------------------------------------------

export const appInfo = () => invoke<AppInfo>('app_info')
export const showWindow = () => invoke<void>('show_window')
export const hideToTray = () => invoke<void>('hide_to_tray')
export const quit = () => invoke<void>('quit')

export const listDevices = () => invoke<DeviceList>('list_devices')
export const setDirect = (id: DeviceId, colors: Rgb[]) => invoke<void>('set_direct', { id, colors })
export const setZone = (id: DeviceId, zone: number, colors: Rgb[]) => invoke<void>('set_zone', { id, zone, colors })
export const setMode = (id: DeviceId, state: ModeState) => invoke<void>('set_mode', { id, state })
export const resizeZone = (id: DeviceId, zone: number, len: number) =>
  invoke<DeviceView>('resize_zone', { id, zone, len })
export const saveToHardware = (id: DeviceId) => invoke<void>('save_to_hardware', { id })
export const releaseDevice = (id: DeviceId) => invoke<void>('release_device', { id })
export const releaseAll = () => invoke<ReleaseResult[]>('release_all')
export const rescan = () => invoke<DeviceList>('rescan')

export const listProfiles = () => invoke<ProfileList>('list_profiles')
export const saveProfile = (name: string) => invoke<ProfileList>('save_profile', { name })
export const applyProfile = (name: string) => invoke<ApplyReport>('apply_profile', { name })
export const renameProfile = (from: string, to: string) => invoke<ProfileList>('rename_profile', { from, to })
export const deleteProfile = (name: string) => invoke<ProfileList>('delete_profile', { name })

export const scanConflicts = () => invoke<ConflictScan>('scan_conflicts')
export const terminateProcess = (pid: number, finding: string) =>
  invoke<void>('terminate_process', { pid, finding })
export const stopService = (name: string, finding: string) => invoke<void>('stop_service', { name, finding })

export const getSettings = () => invoke<SettingsView>('get_settings')
export const setSettings = (patch: SettingsPatch) => invoke<SettingsView>('set_settings', { patch })

// ---- events ----------------------------------------------------------------------------------------------------

export interface EventPayloads {
  'device-added': DeviceInfo
  'device-removed': { id: DeviceId }
  'device-faulted': { id: DeviceId; error: string }
  'device-state-changed': { id: DeviceId; state: ModeState }
  /** Tray actions changed device colours or status. */
  'devices-changed': null
  'profiles-changed': ProfileList
  'conflicts-updated': ConflictScan
}

export function on<K extends keyof EventPayloads>(
  event: K,
  handler: (payload: EventPayloads[K]) => void,
): Promise<UnlistenFn> {
  return listen<EventPayloads[K]>(event, (e) => handler(e.payload))
}

/** The message of a rejected command (commands reject with a string). */
export function errorText(e: unknown): string {
  return typeof e === 'string' ? e : e instanceof Error ? e.message : String(e)
}
