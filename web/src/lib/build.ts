/**
 * Build of the public demo (`npm run build:demo`, published on GitHub Pages):
 * fictional data only, never the API. The interface served by RustNVR has no
 * demo mode.
 */
export const DEMO_SITE: boolean = import.meta.env.VITE_DEMO_SITE === '1'

/** Where the interface lives: `/` when served by RustNVR, `/RustNVR/` for the demo. */
export const BASE_PATH: string = import.meta.env.BASE_URL.replace(/\/$/, '')
