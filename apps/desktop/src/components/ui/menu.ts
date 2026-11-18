// Helpers for ContextMenu.

/** One entry of a ContextMenu. */
export interface MenuItem {
  label: string
  action: () => void
  disabled?: boolean
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
