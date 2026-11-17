// The one toast of the window (docs/design-system.md, "Toast"). Pages call `showToast`; the shell renders it with
// the Toast primitive, which dismisses it after 5 s. A new toast replaces the current one.

import { reactive } from 'vue'

export const toastState = reactive<{ current: { id: number; text: string } | null }>({ current: null })

let nextId = 0

export function showToast(text: string) {
  toastState.current = { id: ++nextId, text }
}

export function dismissToast() {
  toastState.current = null
}

/** "1 device released" / "N devices released". */
export const releasedText = (n: number) => (n === 1 ? '1 device released' : `${n} devices released`)
