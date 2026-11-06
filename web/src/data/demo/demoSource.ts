// Demo data source: deterministic FICTITIOUS data to preview the interface
// without a backend. It never calls the API, and the real source never reads
// from here.

import { LiveStore } from '../../state/liveStore'
import type { Capabilities, NvrEvent, Recording, Storage, SystemInfo } from '../../types/api'
import type { DataSource, EventQuery } from '../source'
import { DEMO_CAMERAS, demoCamera, demoSpec } from './fixtures'

const SEGMENT_MS = 60_000
const DAY_MS = 86_400_000

/** Deterministic PRNG (mulberry32) so that the demo looks the same on every load. */
function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function hash(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return h >>> 0
}

const DEMO_CAPABILITIES: Capabilities = {
  api_version: 1,
  live_video: false,
  live_transcode: false,
  live_webrtc: false,
  snapshots: false,
  // The demo shows what the events pages will look like.
  events: true,
  motion_detection: false,
  camera_management: false,
  camera_probe: false,
  recordings: true,
  export: false,
  protect: false,
  retention: false,
  settings_write: false,
  mqtt: false,
  ai: false,
  users: false,
  backup: false,
}

function isRecording(id: string, t: number, now: number): boolean {
  const spec = demoSpec(id)
  if (!spec || t > now) return false
  if (spec.offlineMinutes !== undefined && t > now - spec.offlineMinutes * 60_000) return false
  if (spec.gap) {
    const d = new Date(t)
    const hour = d.getHours() + d.getMinutes() / 60
    if (hour >= spec.gap[0] && hour < spec.gap[1]) return false
  }
  return true
}

function demoRecordings(camera: string, fromMs: number, toMs: number): Recording[] {
  const now = Date.now()
  const out: Recording[] = []
  const first = Math.floor(fromMs / SEGMENT_MS) * SEGMENT_MS
  for (let t = first; t < toMs && t < now; t += SEGMENT_MS) {
    if (!isRecording(camera, t, now)) continue
    const end = Math.min(t + SEGMENT_MS, now)
    out.push({ camera, start_ms: t, end_ms: end, duration_ms: end - t, bytes: 30_000_000, url: '' })
  }
  return out
}

/** Motion events every 15-90 minutes per camera, over the last 7 days. */
function demoEvents(q: EventQuery): NvrEvent[] {
  const now = Date.now()
  const out: NvrEvent[] = []
  for (const spec of DEMO_CAMERAS) {
    if (q.camera && q.camera !== spec.id) continue
    // Anchor the sequence on the day so that it is stable across reloads.
    const dayStart = Math.floor(now / DAY_MS) * DAY_MS - 7 * DAY_MS
    const r = rng(hash(spec.id))
    for (let t = dayStart + r() * 3600_000; t < now; t += (15 + r() * 75) * 60_000) {
      const duration = (5 + r() * 85) * 1000
      if (t < q.fromMs || t > q.toMs || !isRecording(spec.id, t, now)) continue
      out.push({
        id: `${spec.id}-${Math.round(t)}`,
        camera: spec.id,
        kind: 'motion',
        start_ms: Math.round(t),
        end_ms: Math.round(Math.min(t + duration, now)),
        thumbnail_url: null,
        score: null,
        zones: [],
        objects: [],
      })
    }
  }
  if (q.kinds) return out.filter((e) => q.kinds?.includes(e.kind)).sort((a, b) => b.start_ms - a.start_ms)
  return out.sort((a, b) => b.start_ms - a.start_ms)
}

function demoStorage(): Storage {
  const now = Date.now()
  const total = 4_000_787_030_016
  // A full day at the cameras' nominal bit rates (kbit/s × 125 = bytes/s).
  const fullDay = DEMO_CAMERAS.reduce((a, c) => a + c.kbps, 0) * 125 * 86_400
  const days = Array.from({ length: 14 }, (_, i) => {
    const d = new Date(now - (13 - i) * DAY_MS)
    const day = d.toISOString().slice(0, 10)
    const share = i === 13 ? (d.getUTCHours() + 1) / 24 : 0.95 + rng(i)() * 0.1
    return { day, bytes: Math.round(fullDay * share) }
  })
  const recordings = days.reduce((a, d) => a + d.bytes, 0)
  const used = recordings + 48_000_000_000
  const perDay = recordings / 13.5
  return {
    recordings_dir: '/var/lib/rustnvr/recordings (démo)',
    destinations: [
      {
        id: 'local',
        name: 'Disque local (démo)',
        kind: 'local',
        path: '/var/lib/rustnvr/recordings',
        managed: true,
        require_mount: false,
        available: true,
        unavailable_reason: null,
        disk: { total_bytes: total, used_bytes: used, free_bytes: total - used },
        retention_days: 14,
        max_size_gb: null,
        min_free_gb: null,
        low_space: false,
        remote: null,
        upload: null,
        used_by: DEMO_CAMERAS.map((c) => c.id),
        recordings_bytes: recordings,
        estimated_days_until_full: (total - used) / perDay,
      },
    ],
    default_destination: 'local',
    disk: { total_bytes: total, used_bytes: used, free_bytes: total - used },
    recordings_bytes: recordings,
    cameras: DEMO_CAMERAS.map((c) => ({
      camera: c.id,
      bytes: Math.round((recordings * c.kbps) / DEMO_CAMERAS.reduce((a, x) => a + x.kbps, 0)),
      segments: 14 * 1440,
      oldest_ms: now - 14 * DAY_MS,
      newest_ms: now - 60_000,
    })),
    days,
    fill_rate_bytes_per_day: perDay,
    estimated_days_until_full: (total - used) / perDay,
    retention_configured: false,
    computed_at_ms: now,
  }
}

export function createDemoSource(): DataSource {
  const store = new LiveStore()
  const started = Date.now() - 9 * 3600_000
  const cpuRand = rng(42)
  const history: number[] = []

  function tick(): void {
    const now = Date.now()
    const cpu = 14 + cpuRand() * 9
    history.push(cpu)
    if (history.length > 30) history.shift()
    const cameras = DEMO_CAMERAS.map((s) => demoCamera(s, now, cpuRand()))
    const system: SystemInfo = {
      version: 'démo',
      started_at_ms: started,
      uptime_seconds: Math.round((now - started) / 1000),
      auth_enabled: true,
      user: 'démo',
      cpu_percent: cpu,
      cpu_history: [...history],
      memory_total_bytes: 8 * 1024 ** 3,
      memory_used_bytes: 2.9 * 1024 ** 3,
      process_cpu_percent: 6.4,
      process_rss_bytes: 21 * 1024 ** 2,
      service_cpu_percent: 14.2,
      service_memory_bytes: 158 * 1024 ** 2,
      service_memory_limit_bytes: null,
      service_scope: 'processes',
      cameras_total: cameras.length,
      cameras_enabled: cameras.length,
      cameras_online: cameras.filter((c) => c.status === 'online').length,
      low_space: [],
      restart_needed: false,
      update_available: null,
      gpus: [{ id: 'card0', name: 'Intel UHD Graphics (démo)', vendor: 'intel', driver: 'i915', device: '/dev/dri/renderD128', status: 'usable', busy_percent: 12 }],
    }
    store.set({ system, connection: 'open', error: null })
    store.setCameras(cameras)
  }

  return {
    mode: 'demo',
    store,
    start() {
      for (let i = 0; i < 30; i++) tick()
      store.notify('error', 'Allée (démo) : flux perdu (notification fictive)', 'demo-allee')
      const timer = window.setInterval(tick, 2000)
      return () => window.clearInterval(timer)
    },
    capabilities: () => Promise.resolve(DEMO_CAPABILITIES),
    recordings: (camera, from, to) => Promise.resolve(demoRecordings(camera, from, to)),
    storage: () => Promise.resolve(demoStorage()),
    events: (q) => Promise.resolve(demoEvents(q)),
  }
}
