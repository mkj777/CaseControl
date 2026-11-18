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
  /** The zone's LEDs show a fixed colour and only take a brightness (a colour counts as its highest channel). */
  brightness_only?: boolean
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

/** `active`: CaseControl drives the device. `released`: it shows its own lighting (also before it was ever driven). */
export type DeviceStatus = { state: 'active' } | { state: 'released' } | { state: 'faulted'; error: string }

export interface DeviceView {
  description: DeviceDescription
  status: DeviceStatus
  setting: DeviceSetting | null
  /** The engine's colour buffer, one entry per LED (seeded with the device's own colours where they are known). */
  colors: Rgb[]
  /** What the device shows on its own, as its driver read it; `null` when that is unknown. */
  hardware: ModeState | null
  /** Family id of the backend that detected the device (e.g. `"wooting"`). */
  family: string
  /** Device brightness in percent (ADR 0001 #33); `colors` and `setting` are unscaled. */
  brightness: number
  /** What a takeover (brightness, Take over all) drives the device with, as the engine picks it: what it drives it
   * with now, else its last setting, its own lighting, or the fallback colour. `null` when it cannot be taken over.
   * Does not consider whether the device is hidden. */
  takeover: Takeover | null
}

/** What a takeover drives a device with. */
export interface Takeover {
  /** The hardware mode it uses, or `null` for direct colours. */
  mode_id: number | null
}

/** Master brightness (ADR 0001 #33). */
export interface LightingView {
  /** Percent, multiplied into every device. */
  master_brightness: number
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

/** Result of Release all. */
export interface ReleaseReport {
  /** How many devices were driven and are released now. */
  released: number
  /** Devices whose release failed. */
  failed: ApplyFailure[]
}

// ---- cc-conflicts ----------------------------------------------------------------------------------------------

export type Severity = 'info' | 'low' | 'medium' | 'high'
export type Category = 'rgb_suite' | 'vendor_rgb' | 'hardware_monitor' | 'fan_control' | 'os_feature' | 'driver'
export type Access = 'hid' | 'smbus' | 'superio' | 'kernel_driver' | 'lamparray' | 'gpu_i2c'
export type ServiceState = 'running' | 'stopped' | { other: number }
export type ServiceKind = 'win32' | 'driver'
export type Hive = 'hklm_native' | 'hklm_wow6432' | 'hkcu'
/** Suites may appear in the start dialog; vendor tools only on the Conflicts page (ADR 0001 #31). */
export type ConflictClass = 'suite' | 'vendor_tool'
export type StartType = 'boot' | 'system' | 'auto' | 'manual' | 'disabled'
export type AutostartLocation =
  | 'hkcu_run'
  | 'hkcu_run_once'
  | 'hklm_run'
  | 'hklm_run_once'
  | 'hklm_run32'
  | 'hklm_run_once32'
  | 'user_startup_folder'
  | 'common_startup_folder'
  | 'service'

/** How a program starts with Windows: a Run / RunOnce value, a Startup folder item or a service start type. */
export interface AutostartItem {
  location: AutostartLocation
  /** Value name, Startup folder file name or service name. */
  name: string
  /** Command line or shortcut target; `null` for services. */
  command: string | null
  /** Starts with Windows now. */
  enabled: boolean
  /** Services only. */
  start_type: StartType | null
  /** CaseControl can switch it off and on (not RunOnce, not disabled services). */
  toggleable: boolean
  /** Changing it shows a Windows permission prompt. */
  needs_admin: boolean
}

export interface DynamicLighting {
  ambient_enabled: boolean
  foreground_controls: boolean
  providers: string[]
  device_count: number
}

/** `Evidence` (adjacently tagged with `type` / `data`). */
export type Evidence =
  | { type: 'process'; data: { pid: number; name: string; path: string | null; session: number | null } }
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
  class: ConflictClass
  severity: Severity
  verified: boolean
  access: Access[]
  affects: string[]
  note: string
  running: boolean
  installed: boolean
  evidence: Evidence[]
  autostart: AutostartItem[]
  affected_devices: AffectedDevice[]
  /** What End process / Stop service can end (processes of this Windows session, running services). */
  close_targets: CloseTarget[]
  /** "Don't ask again" is set: it never shows in the start dialog. */
  dont_ask: boolean
  /** An autostart item is on and can be turned off. */
  autostart_on: boolean
  /** An autostart item is off and can be turned back on. */
  can_reenable: boolean
}

/** Something "Close now" / End process / Stop service ends. */
export type CloseTarget = { kind: 'process'; pid: number; name: string } | { kind: 'service'; name: string }

/** One program of the start dialog (`startup::ConflictProgram`). */
export interface ConflictProgram {
  entry_id: string
  name: string
  vendor: string
  note: string
  /** Our visible devices it affects. */
  affected_devices: AffectedDevice[]
  running: boolean
  close_targets: CloseTarget[]
  /** The autostart items that are on; "Turn off autostart" switches them off. */
  autostart: AutostartItem[]
  autostart_on: boolean
  /** Proceeding may show a Windows permission prompt. */
  needs_admin: boolean
}

export interface ConflictScan {
  findings: Finding[]
  errors: string[]
  /** The start dialog model for this scan (empty: nothing to ask). */
  startup: ConflictProgram[]
}

/** One change CaseControl made (or tried) to other software. */
export type ConflictAction =
  | { kind: 'terminate_process'; pid: number; name: string }
  | { kind: 'stop_service'; name: string }
  | { kind: 'set_service_start_type'; name: string; start_type: StartType }
  | { kind: 'set_startup_approved'; location: AutostartLocation; name: string; enabled: boolean }

export interface ActionResult {
  entry_id: string
  action: ConflictAction
  ok: boolean
  error: string | null
  /** Not confirmed yet (the elevated helper was still working); `ok` is false, a scan shows the real state. */
  unknown: boolean
}

export interface ActionReport {
  results: ActionResult[]
  /** Programs of the request that could not be acted on. */
  refused: { entry_id: string; reason: string }[]
  /** The scan afterwards (also sent as `conflicts-updated`). */
  scan: ConflictScan
}

export interface ResolveRequest {
  items: { entry_id: string; close: boolean; disable_autostart: boolean }[]
}

// ---- app -------------------------------------------------------------------------------------------------------

/** Mirrors `AppInfo` in `src-tauri/src/commands.rs`. */
export interface AppInfo {
  name: string
  version: string
  profile: 'debug' | 'release'
}

/** What the user hid of one device (ADR 0001 #26). */
export interface HiddenEntry {
  /** The whole device is hidden (not controlled). */
  device: boolean
  /** Names of the hidden zones (kept black in direct colour). */
  zones: string[]
}

export interface DeviceList {
  devices: DeviceView[]
  failures: DetectFailure[]
  engine_error: string | null
  /** Devices with something hidden, by id; devices with nothing hidden are absent. */
  hidden: Record<DeviceId, HiddenEntry>
  /** Master brightness (the saved value while the engine is not running). */
  lighting: LightingView
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
  /** The colour a takeover shows on a device with neither a stored setting nor a known own mode (ADR 0001 #36). */
  default_color: Rgb
  /** Why settings are not saved this session (unreadable settings file); changes are refused while set. */
  save_error: string | null
}

export interface SettingsPatch {
  autostart?: boolean
  start_minimized?: boolean
  frame_rate?: number
  detectors?: Record<string, boolean>
  check_openrgb_port?: boolean
  default_color?: Rgb
}

/** What the updater is doing or did last (`updater::UpdateStatus`, ADR 0001 #41). `ready` is shown while an update
 * is pending and nothing runs. */
export type UpdateStatus =
  /** A build without updates; `reason` is the text to show. */
  | { state: 'disabled'; reason: string }
  | { state: 'idle' }
  | { state: 'checking' }
  | { state: 'up_to_date' }
  | { state: 'downloading'; version: string }
  | { state: 'ready'; version: string }
  | { state: 'installing'; version: string }
  | { state: 'failed'; error: string }

/** `updater::UpdateView`: the status plus the version of a downloaded update waiting to be installed. */
export interface UpdateView {
  status: UpdateStatus
  /** "Update now" is possible while this is set. */
  pending: string | null
  /** Grows with every status change: keep the view with the highest `seq` (events and command replies race). */
  seq: number
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
export const releaseAll = () => invoke<ReleaseReport>('release_all')
/** Every value is saved to the active profile (coalesced by the backend; ADR 0001 #40). */
export const setDeviceBrightness = (id: DeviceId, percent: number) =>
  invoke<void>('set_device_brightness', { id, percent })
export const setMasterBrightness = (percent: number, persist: boolean) =>
  invoke<LightingView>('set_master_brightness', { percent, persist })
/** Take over all (`connect_all`), the counterpart of Release all: takes control of every released device that is
 * neither hidden nor faulted (its last setting, else its own mode, else the fallback colour, the `default_color`
 * setting). */
export const connectAll = () => invoke<LightingView>('connect_all')
export const rescan = () => invoke<DeviceList>('rescan')
export const setDeviceHidden = (id: DeviceId, hidden: boolean) =>
  invoke<DeviceList>('set_device_hidden', { id, hidden })
export const setZoneHidden = (id: DeviceId, zoneName: string, hidden: boolean) =>
  invoke<DeviceList>('set_zone_hidden', { id, zoneName, hidden })

// Profiles (ADR 0001 #40): there is always exactly one active profile, and every device change is saved into it.
export const listProfiles = () => invoke<ProfileList>('list_profiles')
/** Switches to profile `name`: applies it and makes it active. */
export const applyProfile = (name: string) => invoke<ApplyReport>('apply_profile', { name })
/** Creates an empty "Profile N" and switches to it. */
export const createProfile = () => invoke<ProfileList>('create_profile')
/** Copies profile `name` as "<name> copy" and switches to the copy. */
export const duplicateProfile = (name: string) => invoke<ProfileList>('duplicate_profile', { name })
export const renameProfile = (from: string, to: string) => invoke<ProfileList>('rename_profile', { from, to })
/** Refused for the last profile; deleting the active one switches to the first remaining. */
export const deleteProfile = (name: string) => invoke<ProfileList>('delete_profile', { name })

export const scanConflicts = () => invoke<ConflictScan>('scan_conflicts')
/** The start dialog model from a fresh scan (also sends `conflicts-updated`). */
export const startupConflicts = () => invoke<ConflictProgram[]>('startup_conflicts')
/** The start dialog's Proceed; at most one Windows permission prompt. */
export const resolveConflicts = (request: ResolveRequest) => invoke<ActionReport>('resolve_conflicts', { request })
/** End process / Stop service on the Conflicts page; at most one Windows permission prompt. */
export const closeProgram = (entry: string, targets: CloseTarget[]) =>
  invoke<ActionReport>('close_program', { entry, targets })
/** Turn off (`false`) or re-enable (`true`) a program's autostart. */
export const setAutostart = (entry: string, enabled: boolean) =>
  invoke<ActionReport>('set_autostart', { entry, enabled })
export const setDontAsk = (entry: string, dontAsk: boolean) => invoke<ConflictScan>('set_dont_ask', { entry, dontAsk })

export const getSettings = () => invoke<SettingsView>('get_settings')
export const setSettings = (patch: SettingsPatch) => invoke<SettingsView>('set_settings', { patch })
/** The takeover fallback colour, the `default_color` setting (ADR 0001 #36, #37): `persist = false` while the picker
 * is dragged (used, not saved), `true` for the final value (saved). Resolves with the colour now in effect. */
export const setDefaultColor = (color: Rgb, persist: boolean) =>
  invoke<Rgb>('set_default_color', { color, persist })

// Updates (ADR 0001 #39, #41); release builds only, the commands reject in debug builds.
export const updateStatus = () => invoke<UpdateView>('update_status')
/** Checks now and downloads a newer release; resolves when the check has finished (progress: `update-status`). A
 * check already running is not started again (resolves at once with the current status). */
export const checkForUpdates = () => invoke<UpdateView>('check_for_updates')
/** Releases the devices, installs the pending update and restarts the app; resolves only if nothing was installed. */
export const installUpdate = () => invoke<UpdateView>('install_update')

// ---- events ----------------------------------------------------------------------------------------------------

export interface EventPayloads {
  'device-added': DeviceInfo
  'device-removed': { id: DeviceId }
  'device-faulted': { id: DeviceId; error: string }
  /** Taken under control (`active`), handed back, or switched to a hardware mode (`state`). */
  'device-state-changed': { id: DeviceId; active: boolean; state: ModeState | null }
  /** Tray actions changed device colours or status. */
  'devices-changed': null
  'profiles-changed': ProfileList
  'conflicts-updated': ConflictScan
  /** Sent once after start when the start dialog has programs to ask about. */
  'startup-conflicts': ConflictProgram[]
  /** The updater's status changed. */
  'update-status': UpdateView
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
