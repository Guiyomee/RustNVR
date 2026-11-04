// Theme of the interface, remembered by the browser: dark (default), light,
// or that of the system. Applied as `<html data-theme="…">` (see index.css).

import { useEffect, useState } from 'react'

export type ThemeChoice = 'dark' | 'light' | 'system'

const KEY = 'rustnvr.theme'
const media = () => window.matchMedia('(prefers-color-scheme: light)')

export function storedTheme(): ThemeChoice {
  const v = window.localStorage.getItem(KEY)
  return v === 'light' || v === 'system' ? v : 'dark'
}

/** Sets `data-theme` on the page for `choice`. */
export function applyTheme(choice: ThemeChoice = storedTheme()) {
  const light = choice === 'light' || (choice === 'system' && media().matches)
  document.documentElement.dataset.theme = light ? 'light' : 'dark'
}

/** Applies the stored theme now and follows the system when asked to. */
export function initTheme() {
  applyTheme()
  media().addEventListener('change', () => storedTheme() === 'system' && applyTheme())
}

/** The theme choice, saved and applied when changed. */
export function useTheme(): [ThemeChoice, (c: ThemeChoice) => void] {
  const [choice, setChoice] = useState<ThemeChoice>(storedTheme)
  useEffect(() => applyTheme(choice), [choice])
  return [
    choice,
    (c) => {
      window.localStorage.setItem(KEY, c)
      setChoice(c)
    },
  ]
}
