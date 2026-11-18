// Helpers for ContextMenu.

/** One entry of a ContextMenu. */
export interface MenuItem {
  label: string
  action: () => void
  disabled?: boolean
  /** Set for a choice among items (the profile list): the item is a `menuitemradio` and shows a check mark when
   * `true`. Leave it out for plain actions. */
  checked?: boolean
  /** Draw a separator line above this item. */
  separator?: boolean
  /** Native tooltip, e.g. why the item is disabled. */
  hint?: string
}

/**
 * Where a context menu opens for a `contextmenu` event: at the pointer, or, when the event came from the keyboard
 * (Shift+F10 or the Menu key) and its point lies outside the element, at the element's top-left corner.
 */
export function menuPoint(e: MouseEvent): { x: number; y: number } {
  const el = e.currentTarget instanceof Element ? e.currentTarget : null
  const r = el?.getBoundingClientRect()
  if (!r) return { x: e.clientX, y: e.clientY }
  const inside = e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom
  return inside ? { x: e.clientX, y: e.clientY } : { x: r.left + 8, y: r.top + 8 }
}
