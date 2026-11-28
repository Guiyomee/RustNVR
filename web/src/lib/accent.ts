// Accent color of the interface (buttons, selection, links), chosen in
// Paramètres → Général and remembered by the browser, like the theme. A
// preset or any color: each is adjusted for the theme so that text in that
// color keeps 4.5:1 on the cards, and the text drawn on it (`on-accent`) is
// black or white, whichever reads better.

import { useState } from 'react'

const KEY = 'rustnvr.accent'

/** A preset: its value in the dark and in the light theme. */
export interface AccentPreset {
  id: string
  label: string
  dark: string
  light: string
}

export const ACCENT_PRESETS: AccentPreset[] = [
  { id: 'copper', label: 'Cuivre', dark: '#e08a4e', light: '#a9521f' },
  { id: 'blue', label: 'Bleu', dark: '#5b9cf2', light: '#2563eb' },
  { id: 'teal', label: 'Turquoise', dark: '#2fc4b2', light: '#0f766e' },
  { id: 'green', label: 'Vert', dark: '#4cc38a', light: '#15803d' },
  { id: 'violet', label: 'Violet', dark: '#a78bfa', light: '#7c3aed' },
  { id: 'rose', label: 'Rose', dark: '#f27aaf', light: '#be185d' },
]

/** The stored choice: a preset id, or a `#rrggbb` color. */
export function storedAccent(): string {
  try {
    const v = window.localStorage.getItem(KEY)
    if (v && (ACCENT_PRESETS.some((p) => p.id === v) || /^#[0-9a-f]{6}$/i.test(v))) return v
  } catch {
    // Storage refused: the default.
  }
  return 'copper'
}

type Rgb = [number, number, number]

function parse(hex: string): Rgb {
  const n = Number.parseInt(hex.slice(1), 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

function hex(c: Rgb): string {
  return `#${c.map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0')).join('')}`
}

function luminance([r, g, b]: Rgb): number {
  const lin = (v: number) => {
    const s = v / 255
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

function contrast(a: Rgb, b: Rgb): number {
  const la = luminance(a)
  const lb = luminance(b)
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

/** `c` moved toward `to` until it reaches 4.5:1 against `surface`. */
function readable(c: Rgb, surface: Rgb, to: Rgb): Rgb {
  let out = c
  for (let i = 0; i < 20 && contrast(out, surface) < 4.5; i++) {
    out = [out[0] + (to[0] - out[0]) * 0.12, out[1] + (to[1] - out[1]) * 0.12, out[2] + (to[2] - out[2]) * 0.12]
  }
  return out
}

const CARD_DARK: Rgb = [0x14, 0x17, 0x1b]
const CARD_LIGHT: Rgb = [0xff, 0xff, 0xff]

/** The accent and the color of text on it, for `choice` in a theme. */
export function accentColors(choice: string, light: boolean): { accent: string; on: string } {
  const preset = ACCENT_PRESETS.find((p) => p.id === choice)
  const base = parse(preset ? (light ? preset.light : preset.dark) : choice)
  const accent = light ? readable(base, CARD_LIGHT, [0, 0, 0]) : readable(base, CARD_DARK, [255, 255, 255])
  const dark: Rgb = [0x1a, 0x10, 0x07]
  const on = contrast(accent, dark) >= contrast(accent, [255, 255, 255]) ? hex(dark) : '#ffffff'
  return { accent: hex(accent), on }
}

/** Sets the accent variables on the page, for the theme in effect. */
export function applyAccent(choice: string = storedAccent()) {
  const root = document.documentElement
  const { accent, on } = accentColors(choice, root.dataset.theme === 'light')
  root.style.setProperty('--color-accent', accent)
  root.style.setProperty('--color-on-accent', on)
}

/** The accent choice, saved and applied when changed. */
export function useAccent(): [string, (c: string) => void] {
  const [choice, setChoice] = useState<string>(storedAccent)
  return [
    choice,
    (c) => {
      try {
        window.localStorage.setItem(KEY, c)
      } catch {
        // Kept for this page only.
      }
      setChoice(c)
      applyAccent(c)
    },
  ]
}
