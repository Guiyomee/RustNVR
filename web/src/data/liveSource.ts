// Data source backed by the real RustNVR API.

import { ApiError, UnavailableError, describeError } from '../api/client'
import { getCameras, getCapabilities, getEvents, getRecordings, getStorage, getSystem, wsUrl } from '../api/endpoints'
import { LiveSocket } from '../api/ws'
import { LiveStore } from '../state/liveStore'
import type { EventKind } from '../types/api'
import type { DataSource } from './source'
import { t } from '../lib/i18n'
import { objectLabel } from '../lib/objects'

/** Provider labels (COCO classes) behind each object kind. */
const OBJECT_KINDS: Partial<Record<EventKind, string[]>> = {
  person: ['person'],
  vehicle: ['car', 'truck', 'bus', 'motorcycle', 'bicycle'],
  animal: ['dog', 'cat', 'bird', 'horse', 'sheep', 'cow', 'bear'],
}

const POLL_MS = 10_000

export function createLiveSource(): DataSource {
  const store = new LiveStore()

  async function refresh(): Promise<void> {
    try {
      const [system, cameras] = await Promise.all([getSystem(), getCameras()])
      store.set({ system, error: null, authRequired: false })
      store.setCameras(cameras)
    } catch (e) {
      store.set({
        error: describeError(e),
        authRequired: e instanceof ApiError && e.kind === 'auth',
      })
    }
  }

  return {
    mode: 'live',
    store,
    start() {
      void refresh()
      let poll: number | null = null
      const startPolling = () => {
        if (poll === null) poll = window.setInterval(() => void refresh(), POLL_MS)
      }
      const stopPolling = () => {
        if (poll !== null) window.clearInterval(poll)
        poll = null
      }
      const socket = new LiveSocket(wsUrl(), {
        onStatus: (status) => {
          if (status === 'open') {
            stopPolling()
            store.set({ connection: 'open', error: null })
          } else if (status === 'reconnecting') {
            // Keep data fresh over HTTP while the WebSocket is down.
            startPolling()
            store.set({ connection: 'polling' })
          } else {
            store.set({ connection: status })
          }
        },
        onMessage: (msg) => {
          switch (msg.type) {
            case 'system': {
              const previous = store.getState().system
              const before = previous?.low_space.join(', ') ?? ''
              store.set({ system: msg.system })
              const low = msg.system.low_space.join(', ')
              const problems = (s?: { name: string; problem: string }[]) => new Map((s ?? []).map((p) => [p.name, p.problem]))
              const was = problems(previous?.storage_problems)
              const now = problems(msg.system.storage_problems)
              for (const [name, problem] of now) {
                if (was.get(name) !== problem) store.notify('error', t(`Stockage dégradé : ${name} — ${problem}`, `Storage degraded: ${name} — ${problem}`))
              }
              for (const name of was.keys()) {
                if (!now.has(name)) store.notify('info', t(`Stockage rétabli : ${name}`, `Storage back: ${name}`))
              }
              if (low && low !== before) store.notify('warning', t(`Espace disque faible : ${low}`, `Low disk space: ${low}`))
              break
            }
            case 'camera':
              store.upsertCamera(msg.camera)
              break
            case 'cameras':
              store.setCameras(msg.cameras)
              break
            case 'segment':
              store.addSegment(msg.recording)
              break
            case 'event':
              store.upsertEvent(msg.event)
              break
            case 'recorder_error':
              store.notify('error', t(`Erreur d'écriture : ${msg.message}`, `Write error: ${msg.message}`), msg.camera)
              break
            case 'crossing': {
              // Only crossings checked by the AI: lines counting anything
              // moving would flood the bell.
              const c = msg.crossing
              if (c.object) {
                const name = store.getState().cameras.find((x) => x.id === c.camera)?.name ?? c.camera
                store.notify('info', t(`${name} : ${objectLabel(c.object)} — ligne « ${c.line} » franchie`, `${name}: ${objectLabel(c.object)} — line « ${c.line} » crossed`), c.camera)
              }
              break
            }
          }
        },
      })
      socket.start()
      return () => {
        socket.stop()
        stopPolling()
      }
    },
    capabilities: getCapabilities,
    recordings: getRecordings,
    storage: getStorage,
    events: async (q) => {
      // The server only produces motion events; « person », « vehicle » and
      // « animal » select those whose picture contains such an object
      // (optional AI provider).
      if (q.kinds?.length === 0) return []
      try {
        // Objects and zones are searched by the server: a whole month fits.
        const kinds = q.kinds
        const objects = !kinds || kinds.includes('motion') ? null : kinds.flatMap((k) => OBJECT_KINDS[k] ?? [])
        return await getEvents({ camera: q.camera, fromMs: q.fromMs, toMs: q.toMs, limit: 1000, objects, zones: q.zones ?? null })
      } catch (e) {
        if (e instanceof ApiError && e.status === 503) {
          throw new UnavailableError('GET /api/v1/events', t("Ce serveur n'enregistre pas d'événements (base de données indisponible).", "This server does not record events (database unavailable)."))
        }
        throw e
      }
    },
  }
}
