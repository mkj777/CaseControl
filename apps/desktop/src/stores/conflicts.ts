// Shared conflict state (ADR 0001 #31): the latest scan, the start dialog model and whether the dialog is open.
// The shell owns the dialog and the persistent hint; the Conflicts page reads and refreshes the same scan.

import { reactive } from 'vue'
import { errorText, on, startupConflicts, type ConflictProgram, type ConflictScan } from '../ipc'

interface ConflictState {
  /** Latest scan from any source (page scan, start-up check, after an action). */
  scan: ConflictScan | null
  /** The start dialog model: suites that affect a visible device, without "Don't ask again" ones. */
  programs: ConflictProgram[]
  /** The start dialog is open. */
  dialogOpen: boolean
  /** Failure of the start-up check itself. */
  error: string | null
}

export const conflictState = reactive<ConflictState>({
  scan: null,
  programs: [],
  dialogOpen: false,
  error: null,
})

/** The dialog opens on its own at most once per app run; afterwards only from the hint. */
let autoOpened = false

function offer(programs: ConflictProgram[]) {
  conflictState.programs = programs
  if (programs.length > 0 && !autoOpened && !conflictState.dialogOpen) {
    autoOpened = true
    conflictState.dialogOpen = true
  }
}

export function takeScan(scan: ConflictScan) {
  conflictState.scan = scan
  conflictState.programs = scan.startup
}

/** Programs that still start with Windows: drives the persistent hint. */
export const hintPrograms = () => conflictState.programs.filter((p) => p.autostart_on)

export function openDialog() {
  conflictState.dialogOpen = true
}

export function closeDialog() {
  conflictState.dialogOpen = false
}

let started = false

/** Called once by the shell: listens for scans and the start-up event, then asks for the current model itself,
 * since the start-up event may arrive before this listener exists. */
export async function startConflictWatch() {
  if (started) return
  started = true
  await on('conflicts-updated', takeScan)
  await on('startup-conflicts', offer)
  try {
    offer(await startupConflicts())
  } catch (e) {
    conflictState.error = errorText(e)
  }
}
