// Pure LED-selection helpers (ADR 0001 #22). A selection is a set of indices into the device's flat LED buffer.
// Click selects one LED, dragging sweeps several, Ctrl-click adds or removes; an empty selection means "whole
// device".

import type { Zone } from '../ipc'
import { BLACK } from './color'

export type Selection = ReadonlySet<number>

/** What a drag that started on an LED does to the LEDs it passes. */
export type SweepMode = 'add' | 'remove'

export const EMPTY: Selection = new Set<number>()

/** Indices `0..n`. */
export function range(start: number, len: number): number[] {
  return Array.from({ length: Math.max(0, len) }, (_, i) => start + i)
}

/** LED indices of a zone in the device buffer. */
export const zoneIndices = (zone: Zone): number[] => range(zone.start, zone.len)

/**
 * Pointer down on LED `i`. Without `additive` the selection becomes just `i` and the drag adds. With `additive`
 * (Ctrl) `i` is toggled and the drag continues in the same direction (adding or removing).
 */
export function beginSweep(sel: Selection, i: number, additive: boolean): { sel: Selection; mode: SweepMode } {
  if (!additive) return { sel: new Set([i]), mode: 'add' }
  const next = new Set(sel)
  if (next.has(i)) {
    next.delete(i)
    return { sel: next, mode: 'remove' }
  }
  next.add(i)
  return { sel: next, mode: 'add' }
}

/** The drag passes over LED `i`. Returns the same set object when nothing changes. */
export function sweepOver(sel: Selection, i: number, mode: SweepMode): Selection {
  if ((mode === 'add') === sel.has(i)) return sel
  const next = new Set(sel)
  if (mode === 'add') next.add(i)
  else next.delete(i)
  return next
}

/**
 * Click on a zone name: selects the zone's LEDs. With `additive` (Ctrl) the zone is added, or removed if it was
 * entirely selected already.
 */
export function selectZone(sel: Selection, indices: number[], additive: boolean): Selection {
  if (!additive) return new Set(indices)
  const next = new Set(sel)
  const all = indices.length > 0 && indices.every((i) => next.has(i))
  for (const i of indices) {
    if (all) next.delete(i)
    else next.add(i)
  }
  return next
}

/** Drops indices that no longer exist (after a zone resize). */
export function clampSelection(sel: Selection, ledCount: number): Selection {
  return new Set([...sel].filter((i) => i >= 0 && i < ledCount))
}

/**
 * The buffer with `color` written to the selected LEDs. When nothing is selected, it goes to the LEDs in `scope`
 * (the visible ones), or to every LED when there is no scope.
 */
export function paint(colors: readonly string[], sel: Selection, color: string, scope?: Selection): string[] {
  const target = sel.size > 0 ? sel : scope
  if (!target) return colors.map(() => color)
  return colors.map((c, i) => (target.has(i) ? color : c))
}

/** The buffer with the LEDs in `off` set to black (hidden zones). */
export function blackOut(colors: readonly string[], off: Selection): string[] {
  if (off.size === 0) return [...colors]
  return colors.map((c, i) => (off.has(i) ? BLACK : c))
}

/** The selection without the LEDs in `drop`; the same object when nothing changes. */
export function without(sel: Selection, drop: Selection): Selection {
  if (![...sel].some((i) => drop.has(i))) return sel
  return new Set([...sel].filter((i) => !drop.has(i)))
}

/** The colour the picker starts from: the first selected LED's, or the first LED's when nothing is selected. */
export function selectionColor(colors: readonly string[], sel: Selection, fallback: string): string {
  const first = sel.size > 0 ? Math.min(...sel) : 0
  return colors[first] ?? fallback
}
