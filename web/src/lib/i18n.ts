// Language of the interface: French (default) or English, remembered by the
// browser. Texts are written in place as `t('français', 'english')`; the
// language is read once at startup and changing it reloads the page, so that
// texts defined outside components (menus, lists) follow too.
//
// Messages coming from the server (errors, logs) stay in French.

export type Lang = 'fr' | 'en'

const KEY = 'rustnvr.lang'

function read(): Lang {
  try {
    return window.localStorage.getItem(KEY) === 'en' ? 'en' : 'fr'
  } catch {
    return 'fr'
  }
}

const current: Lang = read()

/** The language in use. */
export function lang(): Lang {
  return current
}

/** Locale for dates and numbers. */
export function locale(): string {
  return current === 'en' ? 'en-GB' : 'fr-FR'
}

/** The text in the language in use. */
export function t(fr: string, en: string): string {
  return current === 'en' ? en : fr
}

/** Saves the language and reloads the page in it. */
export function setLang(next: Lang) {
  if (next === current) return
  window.localStorage.setItem(KEY, next)
  window.location.reload()
}

if (typeof document !== 'undefined') document.documentElement.lang = current
