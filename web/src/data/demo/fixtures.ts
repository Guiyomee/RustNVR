// FICTITIOUS data for demo mode only. Nothing here comes from a camera.
// Every identifier starts with "demo-" and every name is visibly fictitious.

import type { Camera, StreamStats } from '../../types/api'

export const DEMO_PREFIX = 'demo-'

const stats = (frames: number): StreamStats => ({
  frames,
  key_frames: Math.round(frames / 50),
  bytes: frames * 20_000,
  lost_packets: 0,
  sessions: 1,
  failures: 0,
  dropped_frames: 0,
})

interface DemoCameraSpec {
  id: string
  name: string
  codec: 'h264' | 'h265'
  width: number
  height: number
  fps: number
  kbps: number
  /** Hue of the placeholder picture. */
  hue: number
  /** Offline for the last N minutes (shows the "retrying" state). */
  offlineMinutes?: number
  /** [startHour, endHour) of a recording gap today, local time. */
  gap?: [number, number]
}

export const DEMO_CAMERAS: DemoCameraSpec[] = [
  { id: 'demo-entree', name: 'Entrée (démo)', codec: 'h265', width: 2560, height: 1440, fps: 20, kbps: 3800, hue: 210 },
  { id: 'demo-garage', name: 'Garage (démo)', codec: 'h264', width: 1920, height: 1080, fps: 25, kbps: 4100, hue: 30 },
  { id: 'demo-jardin', name: 'Jardin (démo)', codec: 'h265', width: 2688, height: 1520, fps: 15, kbps: 2900, hue: 130, gap: [2.2, 3.1] },
  { id: 'demo-parking', name: 'Parking (démo)', codec: 'h264', width: 1920, height: 1080, fps: 25, kbps: 4000, hue: 260 },
  { id: 'demo-allee', name: 'Allée (démo)', codec: 'h264', width: 1280, height: 720, fps: 15, kbps: 1500, hue: 340, offlineMinutes: 42 },
  { id: 'demo-atelier', name: 'Atelier (démo)', codec: 'h265', width: 1920, height: 1080, fps: 20, kbps: 2200, hue: 180 },
]

export function demoSpec(id: string): DemoCameraSpec | undefined {
  return DEMO_CAMERAS.find((c) => c.id === id)
}

/** Still photo standing for a demo camera (CC0, see public/demo/CREDITS.md); null for other cameras. */
export function demoPicture(id: string): string | null {
  if (!demoSpec(id)) return null
  return `${import.meta.env.BASE_URL}demo/${id.slice(DEMO_PREFIX.length)}.jpg`
}

export function demoCamera(spec: DemoCameraSpec, now: number, jitter: number): Camera {
  const offline = spec.offlineMinutes !== undefined
  return {
    id: spec.id,
    name: spec.name,
    enabled: true,
    recording_enabled: true,
    has_sub_stream: true,
    endpoint: 'rtsp://demo.invalid:554',
    status: offline ? 'retrying' : 'online',
    attempt: offline ? 7 : null,
    retry_in_ms: offline ? 30_000 : null,
    last_error: offline ? 'DESCRIBE failed: Unable to connect to RTSP server (démo)' : null,
    online_since_ms: offline ? null : now - 9 * 3600_000,
    codec: offline ? null : spec.codec,
    width: offline ? null : spec.width,
    height: offline ? null : spec.height,
    fps: offline ? null : Math.max(1, spec.fps + (jitter - 0.5) * 0.4),
    bitrate_kbps: offline ? null : Math.round(spec.kbps * (0.9 + jitter * 0.2)),
    sub_status: offline ? 'retrying' : 'online',
    managed: false,
    edit: null,
    record_destinations: ['local'],
    record_mode: 'continuous',
    retention_days: null,
    max_size_gb: null,
    motion: { enabled: !offline, running: !offline, active: false, error: null },
    record_audio: true,
    audio: null,
    // The garage camera shows the PTZ controls (nothing is sent in demo mode).
    ptz: spec.id === 'demo-garage',
    stream: stats(offline ? 0 : 9 * 3600 * spec.fps),
    recorder: {
      segments: 540,
      frames_written: 9 * 3600 * spec.fps,
      bytes_written: 9 * 3600 * spec.kbps * 125,
      frames_skipped: 0,
      write_errors: 0,
      discontinuities: offline ? 1 : 0,
    },
  }
}
