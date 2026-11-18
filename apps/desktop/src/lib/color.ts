// Pure colour helpers: hex parsing/formatting and RGB <-> HSV, plus the quick swatches (ADR 0001 #20).

export interface RgbTriple {
  r: number
  g: number
  b: number
}

/** Hue in degrees `[0, 360)`, saturation and value in `[0, 1]`. */
export interface Hsv {
  h: number
  s: number
  v: number
}

export const BLACK = '#000000'

/** The quick colours, in display order. "Off" is black, which turns the LEDs off. */
export const SWATCHES: readonly { name: string; hex: string }[] = [
  { name: 'White', hex: '#FFFFFF' },
  { name: 'Red', hex: '#FF0000' },
  { name: 'Orange', hex: '#FF6000' },
  { name: 'Yellow', hex: '#FFD000' },
  { name: 'Green', hex: '#00FF00' },
  { name: 'Cyan', hex: '#00FFFF' },
  { name: 'Blue', hex: '#0000FF' },
  { name: 'Purple', hex: '#8000FF' },
  { name: 'Magenta', hex: '#FF00FF' },
  { name: 'Off', hex: BLACK },
]

/** Parses `#RRGGBB`, `#RGB`, `RRGGBB` or `RGB` (case-insensitive); `null` if invalid. */
export function parseHex(input: string): RgbTriple | null {
  let s = input.trim()
  if (s.startsWith('#')) s = s.slice(1)
  if (/^[0-9a-fA-F]{3}$/.test(s)) s = s.replace(/./g, (c) => c + c)
  if (!/^[0-9a-fA-F]{6}$/.test(s)) return null
  const n = parseInt(s, 16)
  return { r: (n >> 16) & 0xff, g: (n >> 8) & 0xff, b: n & 0xff }
}

const clampByte = (x: number) => Math.min(255, Math.max(0, Math.round(x)))

/** Formats as upper-case `#RRGGBB` (the Rust `Rgb` serialization). */
export function toHex({ r, g, b }: RgbTriple): string {
  return '#' + [r, g, b].map((x) => clampByte(x).toString(16).padStart(2, '0')).join('').toUpperCase()
}

/** Normalizes any accepted hex form to `#RRGGBB`; `null` if invalid. */
export function normalizeHex(input: string): string | null {
  const rgb = parseHex(input)
  return rgb ? toHex(rgb) : null
}

export function rgbToHsv({ r, g, b }: RgbTriple): Hsv {
  const rn = r / 255
  const gn = g / 255
  const bn = b / 255
  const max = Math.max(rn, gn, bn)
  const min = Math.min(rn, gn, bn)
  const d = max - min
  let h = 0
  if (d !== 0) {
    if (max === rn) h = ((gn - bn) / d) % 6
    else if (max === gn) h = (bn - rn) / d + 2
    else h = (rn - gn) / d + 4
    h *= 60
    if (h < 0) h += 360
  }
  return { h, s: max === 0 ? 0 : d / max, v: max }
}

export function hsvToRgb({ h, s, v }: Hsv): RgbTriple {
  const hh = (((h % 360) + 360) % 360) / 60
  const c = v * s
  const x = c * (1 - Math.abs((hh % 2) - 1))
  const m = v - c
  const [r, g, b] =
    hh < 1 ? [c, x, 0] : hh < 2 ? [x, c, 0] : hh < 3 ? [0, c, x] : hh < 4 ? [0, x, c] : hh < 5 ? [x, 0, c] : [c, 0, x]
  return { r: (r + m) * 255, g: (g + m) * 255, b: (b + m) * 255 }
}

export const hexToHsv = (hex: string): Hsv => rgbToHsv(parseHex(hex) ?? { r: 0, g: 0, b: 0 })
export const hsvToHex = (hsv: Hsv): string => toHex(hsvToRgb(hsv))

/** Whether two hex strings denote the same colour. */
export function sameColor(a: string, b: string): boolean {
  return normalizeHex(a) !== null && normalizeHex(a) === normalizeHex(b)
}
