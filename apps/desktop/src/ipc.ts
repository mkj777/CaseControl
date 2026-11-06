import { invoke } from '@tauri-apps/api/core'

/** Mirrors `AppInfo` in `src-tauri/src/commands.rs`. */
export interface AppInfo {
  name: string
  version: string
  profile: 'debug' | 'release'
}

export function appInfo(): Promise<AppInfo> {
  return invoke<AppInfo>('app_info')
}

export function hideToTray(): Promise<void> {
  return invoke('hide_to_tray')
}
