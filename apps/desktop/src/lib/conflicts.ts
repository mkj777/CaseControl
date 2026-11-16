// Pure helpers for the Conflicts page: readable evidence, grouping by severity and the actions a finding offers.

import type { Evidence, Finding, ServiceState, Severity } from '../ipc'

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

export type FindingAction =
  | { kind: 'process'; pid: number; name: string }
  | { kind: 'service'; name: string; driver: boolean }

/** "End process" for each running process, "Stop service" for each running service or loaded driver. */
export function actionsOf(f: Finding): FindingAction[] {
  const out: FindingAction[] = []
  for (const e of f.evidence) {
    if (e.type === 'process') out.push({ kind: 'process', pid: e.data.pid, name: e.data.name })
    else if ((e.type === 'service' || e.type === 'driver') && e.data.state === 'running')
      out.push({ kind: 'service', name: e.data.name, driver: e.type === 'driver' })
  }
  return out
}
