// Typed wrappers of the routes that exist in crates/rustnvr-api/src/routes.rs.
// Routes that do not exist yet are documented in API_CONTRACT.md and are not
// called from here: the data layer reports them as unavailable instead.

import { apiGet, apiSend, apiSendForFile } from './client'
import type {
  BackupPreview,
  BackupRestored,
  CaptureDay,
  CapturePicture,
  ReplayResult,
  AiModel,
  AiProbeResult,
  AiSettings,
  AiTestResult,
  AiView,
  Camera,
  LogEntry,
  LogLevel,
  CameraInput,
  PtzMove,
  PtzPreset,
  VideoDevice,
  Capabilities,
  CrossingStats,
  Destination,
  DestinationInput,
  DiscoveryScan,
  DiscoveryStreams,
  DiscoveryStreamsInput,
  EventStats,
  ExportResult,
  GeneralSettings,
  GeneralSettingsInput,
  MotionSettings,
  MqttInput,
  MqttView,
  NvrEvent,
  ProbeInput,
  ProbeResult,
  ProtectedRange,
  Recording,
  Storage,
  SystemInfo,
  UpdateStatus,
  Webhook,
  WebhookTestResult,
} from '../types/api'

export const getCapabilities = () => apiGet<Capabilities>('/capabilities')
export const getSystem = () => apiGet<SystemInfo>('/system')
export const getCameras = () => apiGet<Camera[]>('/cameras')
// Camera management (only when capabilities.camera_management is true).
export const createCamera = (input: CameraInput) => apiSend<Camera>('POST', '/cameras', input, 20_000)
export const updateCamera = (id: string, input: CameraInput) =>
  apiSend<Camera>('PUT', `/cameras/${encodeURIComponent(id)}`, input, 30_000)
export const deleteCamera = (id: string) => apiSend<null>('DELETE', `/cameras/${encodeURIComponent(id)}`, undefined, 30_000)
// Connection tests wait up to 10 s per RTSP step on the server.
export const probeStream = (input: ProbeInput) => apiSend<ProbeResult>('POST', '/cameras/probe', input, 45_000)
// Cameras on the network: scan (≈ 3 s), then the streams of one camera (ONVIF, else usual paths: up to ≈ 1 min).
export const scanNetwork = (network: string | null) => apiSend<DiscoveryScan>('POST', '/discovery/scan', network ? { network } : {}, 60_000)
export const discoverStreams = (input: DiscoveryStreamsInput) => apiSend<DiscoveryStreams>('POST', '/discovery/streams', input, 120_000)
export const probeCamera = (id: string) =>
  apiSend<ProbeResult>('POST', `/cameras/${encodeURIComponent(id)}/probe`, undefined, 45_000)

// Storage destinations and retention.
export const getDestinations = () => apiGet<Destination[]>('/storage/destinations', 20_000)
export const createDestination = (input: DestinationInput) => apiSend<Destination[]>('POST', '/storage/destinations', input, 20_000)
export const updateDestination = (id: string, input: DestinationInput) =>
  apiSend<Destination[]>('PUT', `/storage/destinations/${encodeURIComponent(id)}`, input, 60_000)
export const deleteDestination = (id: string) =>
  apiSend<Destination[]>('DELETE', `/storage/destinations/${encodeURIComponent(id)}`, undefined, 20_000)

export const getStorage = () => apiGet<Storage>('/storage', 20_000)
export const getRecordings = (id: string, fromMs: number, toMs: number, signal?: AbortSignal) =>
  apiGet<Recording[]>(
    `/cameras/${encodeURIComponent(id)}/recordings?from=${Math.floor(fromMs)}&to=${Math.ceil(toMs)}`,
    20_000,
    signal,
  )

function wsBase(): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}/api/v1`
}

/** Event WebSocket on the same origin (ws:// or wss://). */
export const wsUrl = () => `${wsBase()}/ws`

export type LiveStream = 'main' | 'sub'

/** Live video WebSocket of a camera (fMP4 fragments for MSE). */
export function liveUrl(id: string, opts: { stream?: LiveStream; transcode?: boolean; audio?: boolean } = {}): string {
  const q = new URLSearchParams()
  if (opts.stream) q.set('stream', opts.stream)
  if (opts.transcode) q.set('transcode', 'h264')
  // The sound, when the camera sends it and records it (not with a conversion).
  if (opts.audio) q.set('audio', '1')
  const qs = q.toString()
  return `${wsBase()}/cameras/${encodeURIComponent(id)}/live${qs ? `?${qs}` : ''}`
}

// Detection events (capabilities.events).
export const getEvents = (q: { camera: string | null; fromMs: number; toMs: number; limit?: number }, signal?: AbortSignal) => {
  const p = new URLSearchParams({ from: String(Math.floor(q.fromMs)), to: String(Math.ceil(q.toMs)), limit: String(q.limit ?? 500) })
  if (q.camera) p.set('camera', q.camera)
  return apiGet<NvrEvent[]>(`/events?${p.toString()}`, 20_000, signal)
}

// Motion detection settings (capabilities.motion_detection).
export const getMotion = (id: string) => apiGet<MotionSettings>(`/cameras/${encodeURIComponent(id)}/motion`)
export const setMotion = (id: string, settings: MotionSettings) =>
  apiSend<MotionSettings>('PUT', `/cameras/${encodeURIComponent(id)}/motion`, settings, 30_000)

/** Current picture of a camera as a JPEG (capabilities.snapshots); `t` defeats caching. */
export const snapshotUrl = (id: string, t = Date.now()) => `/api/v1/cameras/${encodeURIComponent(id)}/snapshot?t=${t}`

// MQTT and Home Assistant (capabilities.mqtt).
export const getMqtt = () => apiGet<MqttView>('/mqtt')
export const setMqtt = (input: MqttInput) => apiSend<MqttView>('PUT', '/mqtt', input, 20_000)

// General settings (capabilities.settings_write).
export const getSettings = () => apiGet<GeneralSettings>('/settings')
export const setSettings = (input: GeneralSettingsInput) => apiSend<GeneralSettings>('PUT', '/settings', input, 120_000)

// Accounts and sessions.
export interface AuthUser {
  username: string | null
  role: 'admin' | 'viewer'
}
export interface AuthStatus {
  /** Authentication is required. */
  required: boolean
  /** Accounts can be managed (database available). */
  accounts: boolean
  /** No account yet: the first administrator can be created. */
  setup: boolean
  user: AuthUser | null
}
export interface UserAccount {
  username: string
  role: 'admin' | 'viewer'
  created_ms: number
}
export const getAuthStatus = () => apiGet<AuthStatus>('/auth/status')
export const login = (username: string, password: string) => apiSend<AuthUser>('POST', '/auth/login', { username, password }, 30_000)
export const logout = () => apiSend<null>('POST', '/auth/logout')
export const setupAdmin = (username: string, password: string) => apiSend<AuthUser>('POST', '/auth/setup', { username, password }, 30_000)
export const changePassword = (current: string, next: string) => apiSend<null>('PUT', '/auth/password', { current, new: next }, 30_000)
export const getUsers = () => apiGet<UserAccount[]>('/users')
export const createUser = (username: string, password: string, role: 'admin' | 'viewer') => apiSend<null>('POST', '/users', { username, password, role }, 30_000)
export const updateUser = (name: string, change: { password?: string; role?: 'admin' | 'viewer' }) =>
  apiSend<null>('PUT', `/users/${encodeURIComponent(name)}`, change, 30_000)
export const deleteUser = (name: string) => apiSend<null>('DELETE', `/users/${encodeURIComponent(name)}`)

// Sound recording of a camera (camera_management).
export const setDetectionPaused = (paused: boolean) => apiSend<{ paused: boolean }>('PUT', '/detection/pause', { paused }, 30_000)
export const setAudio = (id: string, enabled: boolean) => apiSend<Camera>('PUT', `/cameras/${encodeURIComponent(id)}/audio`, { enabled }, 30_000)

// Object recognition settings (capabilities.settings_write).
export const getAi = () => apiGet<AiView>('/ai')
export const setAi = (settings: AiSettings) => apiSend<AiView>('PUT', '/ai', settings, 60_000)
export const testAi = (camera: string) => apiSend<AiTestResult>('POST', `/ai/test?camera=${encodeURIComponent(camera)}`, {}, 60_000)
export const probeAi = (url: string, apiKeyEnv: string | null) => apiSend<AiProbeResult>('POST', '/ai/probe', { url, api_key_env: apiKeyEnv }, 45_000)
// Downloads a model of the built-in recognition (checked by its SHA-256) on the server.
export const downloadAiModel = (id: string) => apiSend<AiModel>('POST', `/ai/models/${encodeURIComponent(id)}`, {}, 31 * 60_000)

// Recent log messages (administrators).
export const getLogs = (q: { since: number; level: LogLevel; camera?: string; limit?: number }) => {
  const p = new URLSearchParams({ since: String(q.since), level: q.level, limit: String(q.limit ?? 500) })
  if (q.camera) p.set('camera', q.camera)
  return apiGet<{ entries: LogEntry[]; last_seq: number }>(`/logs?${p}`)
}

// Extract of the recordings as one MP4 file (capabilities.export). Preparing
// it can take a while (a conversion to H.264 above all).
export const exportClip = (id: string, q: { fromMs: number; toMs: number; h264: boolean }) =>
  apiSend<ExportResult>('POST', `/cameras/${encodeURIComponent(id)}/export`, { from_ms: Math.floor(q.fromMs), to_ms: Math.ceil(q.toMs), h264: q.h264 }, 30 * 60_000)

// New versions published on the watched repository (Gitea, Forgejo, GitHub).
export const getUpdates = () => apiGet<UpdateStatus>('/updates')
export const checkUpdates = () => apiSend<UpdateStatus>('POST', '/updates/check', {}, 30_000)
export const setUpdates = (check: boolean) => apiSend<UpdateStatus>('PUT', '/updates', { check }, 30_000)

// Protected recordings (capabilities.protect): kept whatever the retention.
export const getProtected = () => apiGet<ProtectedRange[]>('/protected')
export const protectRange = (q: { camera: string; startMs: number; endMs: number; note: string }) =>
  apiSend<ProtectedRange>('POST', '/protected', { camera: q.camera, start_ms: Math.floor(q.startMs), end_ms: Math.ceil(q.endMs), note: q.note })
export const unprotectRange = (id: number) => apiSend<null>('DELETE', `/protected/${id}`)

// Webhooks (administrators only: addresses may carry tokens).
export const getWebhooks = () => apiGet<Webhook[]>('/webhooks')
export const setWebhooks = (hooks: Webhook[]) => apiSend<Webhook[]>('PUT', '/webhooks', hooks)
export const testWebhook = (hook: Webhook) => apiSend<WebhookTestResult>('POST', '/webhooks/test', hook, 30_000)

// Activity statistics (capabilities.events).
export const getEventStats = (days: 7 | 30) => apiGet<EventStats>(`/events/stats?days=${days}`)
export const getCrossings = (days: 7 | 30) => apiGet<CrossingStats>(`/crossings?days=${days}`)

// Backup of the configuration (administrators). Argon2 makes them take a moment.
export const createBackup = (passphrase: string) => apiSendForFile('/backup', { passphrase }, 60_000)
export const inspectBackup = (data: string, passphrase: string) => apiSend<BackupPreview>('POST', '/backup/inspect', { data, passphrase }, 60_000)
export const restoreBackup = (data: string, passphrase: string, files: string[], motion: boolean) =>
  apiSend<BackupRestored>('POST', '/backup/restore', { data, passphrase, files, motion }, 60_000)
export const resetPresence = (camera: string, line: string) =>
  apiSend<null>('POST', `/cameras/${encodeURIComponent(camera)}/presence/reset`, { line })

// Kept pictures of the object recognition and its replay on them.
export const getCaptureDays = () => apiGet<CaptureDay[]>('/ai/captures')
export const captureUrl = (camera: string, day: string, file: string) =>
  `/api/v1/ai/captures/${encodeURIComponent(camera)}/${encodeURIComponent(day)}/${encodeURIComponent(file)}`
export const deleteCaptureDay = (camera: string, day: string) =>
  apiSend<{ deleted: number }>('DELETE', `/ai/captures/${encodeURIComponent(camera)}/${encodeURIComponent(day)}`)
// Up to 200 pictures, about 0.5 s each with CodeProject.AI on a processor;
// one picture (`file`) with YOLOX-x takes a few seconds.
export const replayCaptures = (camera: string, day: string, file?: string) =>
  apiSend<ReplayResult>('POST', '/ai/replay', file ? { camera, day, file } : { camera, day }, file ? 60_000 : 300_000)
export const getCapturePictures = (camera: string, day: string) =>
  apiGet<CapturePicture[]>(`/ai/captures/${encodeURIComponent(camera)}/${encodeURIComponent(day)}`)

// Motorised cameras (ONVIF PTZ, capabilities.ptz and camera.ptz). Moves and
// presets are administrator writes; a move lasts until stop (10 s at most).
const ptzPath = (camera: string) => `/cameras/${encodeURIComponent(camera)}/ptz`
export const getPtz = (camera: string) => apiGet<{ presets: PtzPreset[] }>(ptzPath(camera))
export const ptzMove = (camera: string, move: PtzMove) => apiSend<null>('POST', `${ptzPath(camera)}/move`, move)
export const ptzStop = (camera: string) => apiSend<null>('POST', `${ptzPath(camera)}/stop`, {})
export const ptzGoto = (camera: string, token: string) =>
  apiSend<null>('POST', `${ptzPath(camera)}/presets/${encodeURIComponent(token)}/goto`, {})
export const ptzSavePreset = (camera: string, name: string) => apiSend<null>('POST', `${ptzPath(camera)}/presets`, { name })
export const ptzDeletePreset = (camera: string, token: string) =>
  apiSend<null>('DELETE', `${ptzPath(camera)}/presets/${encodeURIComponent(token)}`)

// Video devices of the machine (USB webcams), for the camera form
// (capabilities.usb_cameras). FFmpeg reads their formats: a few seconds.
export const getVideoDevices = () => apiGet<{ devices: VideoDevice[] }>('/devices/video', 30_000)
// Live preview of a webcam in the camera form (a minute, motion JPEG).
export const usbPreview = (usb: NonNullable<CameraInput['usb']>) =>
  apiSend<{ preview: string }>('POST', '/cameras/usb/preview', usb, 30_000)
