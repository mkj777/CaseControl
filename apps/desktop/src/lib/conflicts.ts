// Pure helpers for the Conflicts page and the start dialog: readable evidence and autostart items, grouping by
// severity, the close targets a finding offers and readable action results.

import type {
  ActionResult,
  AutostartItem,
  AutostartLocation,
  CloseTarget,
  Evidence,
  Finding,
  ServiceState,
  Severity,
  StartType,
} from '../ipc'

export const SEVERITY_ORDER: readonly Severity[] = ['high', 'medium', 'low', 'info']

export const SEVERITY_LABEL: Record<Severity, string> = {
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  info: 'Info',
}

export function groupBySeverity(findings: readonly Finding[]): { severity: Severity; findings: Finding[] }[] {
  return SEVERITY_ORDER.map((severity) => ({
    severity,
    findings: findings.filter((f) => f.severity === severity),
  })).filter((g) => g.findings.length > 0)
}

function stateText(s: ServiceState): string {
  return typeof s === 'string' ? s : `state ${s.other}`
}

export function evidenceText(e: Evidence): string {
  switch (e.type) {
    case 'process':
      return `Process ${e.data.name} (pid ${e.data.pid}) is running`
    case 'service':
      return `Service ${e.data.name} is ${stateText(e.data.state)}`
    case 'driver':
      return `Driver ${e.data.name} is ${e.data.state === 'running' ? 'loaded' : stateText(e.data.state)}`
    case 'installed':
      return `Installed: ${e.data.display_name}${e.data.version ? ` ${e.data.version}` : ''}`
    case 'path':
      return `Found at ${e.data}`
    case 'dynamic_lighting': {
      const d = e.data
      const providers = d.providers.length > 0 ? `; providers: ${d.providers.join(', ')}` : ''
      return `Windows Dynamic Lighting background control is ${d.ambient_enabled ? 'on' : 'off'}${providers}`
    }
    case 'open_rgb_port':
      return 'An OpenRGB server answers on port 6742'
  }
}

export const closeTargetKey = (t: CloseTarget) => (t.kind === 'process' ? `p${t.pid}` : `s${t.name}`)

export const closeTargetLabel = (t: CloseTarget) =>
  t.kind === 'process' ? `End process ${t.name}` : `Stop service ${t.name}`

export const LOCATION_LABEL: Record<AutostartLocation, string> = {
  hkcu_run: 'Run key, this user',
  hkcu_run_once: 'RunOnce key, this user',
  hklm_run: 'Run key, all users',
  hklm_run_once: 'RunOnce key, all users',
  hklm_run32: 'Run key, all users (32-bit)',
  hklm_run_once32: 'RunOnce key, all users (32-bit)',
  user_startup_folder: 'Startup folder, this user',
  common_startup_folder: 'Startup folder, all users',
  service: 'Service',
}

const START_TYPE_LABEL: Record<StartType, string> = {
  boot: 'starts at boot',
  system: 'starts with the system',
  auto: 'starts automatically',
  manual: 'starts manually',
  disabled: 'disabled',
}

/** "SignalRgb (Run key, this user): on" / "Service OpenRGB: starts automatically". */
export function autostartText(item: AutostartItem): string {
  if (item.location === 'service') {
    return `Service ${item.name}: ${item.start_type ? START_TYPE_LABEL[item.start_type] : 'start type unknown'}`
  }
  return `${item.name} (${LOCATION_LABEL[item.location]}): ${item.enabled ? 'on' : 'off'}`
}

/** What an action did, in past tense for a success and as "Could not …" for a failure. */
export function actionText(r: ActionResult): string {
  const a = r.action
  const done = (() => {
    switch (a.kind) {
      case 'terminate_process':
        return r.ok ? `Ended ${a.name}` : `Could not end ${a.name}`
      case 'stop_service':
        return r.ok ? `Stopped service ${a.name}` : `Could not stop service ${a.name}`
      case 'set_service_start_type':
        return a.start_type === 'manual'
          ? r.ok
            ? `Service ${a.name} now starts manually`
            : `Could not set service ${a.name} to start manually`
          : r.ok
            ? `Service ${a.name} starts automatically again`
            : `Could not set service ${a.name} to start automatically`
      case 'set_startup_approved':
        return a.enabled
          ? r.ok
            ? `${a.name} starts with Windows again`
            : `Could not turn autostart of ${a.name} back on`
          : r.ok
            ? `${a.name} no longer starts with Windows`
            : `Could not turn off autostart of ${a.name}`
    }
  })()
  if (r.unknown) return `${done.replace(/^Could not /, 'Not confirmed yet: ')}`
  return r.ok || !r.error ? done : `${done}: ${r.error}`
}
