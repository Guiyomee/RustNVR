// Data source backed by the real RustNVR API.

import { ApiError, UnavailableError, describeError } from '../api/client'
import { getCameras, getCapabilities, getEvents, getRecordings, getStorage, getSystem, wsUrl } from '../api/endpoints'
import { LiveSocket } from '../api/ws'
import { LiveStore } from '../state/liveStore'
import type { EventKind } from '../types/api'
import type { DataSource } from './source'
import { t } from '../lib/i18n'

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
              const before = store.getState().system?.low_space.join(', ') ?? ''
              store.set({ system: msg.system })
              const low = msg.system.low_space.join(', ')
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
        const all = await getEvents({ camera: q.camera, fromMs: q.fromMs, toMs: q.toMs })
        const kinds = q.kinds
        if (!kinds || kinds.includes('motion')) return all
        return all.filter((e) => e.objects.some((o) => kinds.some((k) => OBJECT_KINDS[k]?.includes(o.label))))
      } catch (e) {
        if (e instanceof ApiError && e.status === 503) {
          throw new UnavailableError('GET /api/v1/events', t("Ce serveur n'enregistre pas d'événements (base de données indisponible).", "This server does not record events (database unavailable)."))
        }
        throw e
      }
    },
  }
}
