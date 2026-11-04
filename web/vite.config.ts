import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Node global, typed locally to avoid depending on @types/node.
declare const process: { env: Record<string, string | undefined> }

// In development the interface runs on Vite's server and forwards API and
// WebSocket calls to a running `rustnvr` (default http://127.0.0.1:8080).
// In production, `npm run build` writes static files to `dist/`, served by RustNVR.
const backend = process.env.RUSTNVR_URL ?? 'http://127.0.0.1:8080'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      '/api': { target: backend, changeOrigin: false, ws: true },
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: false,
    chunkSizeWarningLimit: 300,
  },
})
