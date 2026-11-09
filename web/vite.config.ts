import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Node global, typed locally to avoid depending on @types/node.
declare const process: { env: Record<string, string | undefined> }

// node:fs, loaded by name and typed locally (no @types/node).
interface NodeFs {
  rmSync(path: string, options: { recursive: boolean; force: boolean }): void
  copyFileSync(from: string, to: string): void
}
const nodeFs = (): Promise<NodeFs> => import(/* @vite-ignore */ ['node', 'fs'].join(':')) as Promise<NodeFs>

/** The interface served by RustNVR has no demo: its pictures are left out. */
const withoutDemoPictures: Plugin = {
  name: 'rustnvr-without-demo-pictures',
  apply: 'build',
  async closeBundle() {
    ;(await nodeFs()).rmSync('dist/demo', { recursive: true, force: true })
  },
}

/** GitHub Pages answers unknown paths with 404.html: the interface again. */
const pagesFallback: Plugin = {
  name: 'rustnvr-pages-fallback',
  apply: 'build',
  async closeBundle() {
    ;(await nodeFs()).copyFileSync('dist-demo/index.html', 'dist-demo/404.html')
  },
}

// In development the interface runs on Vite's server and forwards API and
// WebSocket calls to a running `rustnvr` (default http://127.0.0.1:8080).
// In production, `npm run build` writes static files to `dist/`, served by RustNVR.
const backend = process.env.RUSTNVR_URL ?? 'http://127.0.0.1:8080'

// `--mode demo` (npm run build:demo, npm run dev:demo): the public demo,
// fictional data only, under /RustNVR/ (GitHub Pages; RUSTNVR_DEMO_BASE to
// change it), written to dist-demo/.
export default defineConfig(({ mode }) => {
  const demo = mode === 'demo'
  return {
    base: demo ? (process.env.RUSTNVR_DEMO_BASE ?? '/RustNVR/') : '/',
    define: { 'import.meta.env.VITE_DEMO_SITE': JSON.stringify(demo ? '1' : '') },
    plugins: [react(), tailwindcss(), demo ? pagesFallback : withoutDemoPictures],
    server: {
      proxy: demo ? undefined : {
        '/api': { target: backend, changeOrigin: false, ws: true },
      },
    },
    build: {
      outDir: demo ? 'dist-demo' : 'dist',
      sourcemap: false,
      chunkSizeWarningLimit: 300,
    },
  }
})
