/**
 * Build of the public demo (`npm run build:demo`, published on GitHub Pages):
 * fictional data only, never the API. The interface served by RustNVR has no
 * demo mode.
 */
export const DEMO_SITE: boolean = import.meta.env.VITE_DEMO_SITE === '1'

/**
 * Where the interface lives, without the final `/`: `''` when served by
 * RustNVR, `/RustNVR` for the demo, and under Home Assistant ingress the
 * prefix given by the server (`/api/hassio_ingress/<token>`, in
 * `<meta name="rustnvr-base">`).
 */
export const BASE_PATH: string = DEMO_SITE
  ? import.meta.env.BASE_URL.replace(/\/$/, '')
  : (document.querySelector<HTMLMetaElement>('meta[name="rustnvr-base"]')?.content ?? '')

/** An address given by the server (`/api/v1/…`), under the interface's base path. */
export const withBase = (url: string): string => (url.startsWith('/api/') ? BASE_PATH + url : url)
