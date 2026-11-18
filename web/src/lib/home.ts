// Page shown when the interface is opened at its root, remembered by the
// browser: a screen kept on the video wall opens on it. Only the first load
// goes there; the « Overview » menu still leads to the overview.

import { useState } from 'react'
import { BASE_PATH } from './build'
import { t } from './i18n'

/** Pages every account can open (no administrator page). */
export const HOME_PAGES = [
  { to: '/', label: t("Vue d'ensemble", 'Overview') },
  { to: '/mur', label: t("Mur d'écrans", 'Video wall') },
  { to: '/events', label: t('Événements', 'Events') },
  { to: '/images', label: t('Images', 'Pictures') },
  { to: '/playback', label: t('Lecture', 'Playback') },
] as const

const KEY = 'rustnvr.home'

function storedHome(): string {
  try {
    const v = window.localStorage.getItem(KEY)
    return HOME_PAGES.some((p) => p.to === v) && v ? v : '/'
  } catch {
    return '/'
  }
}

/** On the first load of the root, replaces it with the chosen page. */
export function openHomePage() {
  const root = BASE_PATH + '/'
  if (window.location.pathname !== root && window.location.pathname !== BASE_PATH) return
  if (window.location.search || window.location.hash) return
  const home = storedHome()
  if (home !== '/') window.history.replaceState(null, '', BASE_PATH + home)
}

/** The home page choice, saved when changed. */
export function useHomePage(): [string, (to: string) => void] {
  const [home, setHome] = useState(storedHome)
  return [
    home,
    (to) => {
      try {
        window.localStorage.setItem(KEY, to)
      } catch {
        // Private window: the choice lasts until the page is closed.
      }
      setHome(to)
    },
  ]
}
