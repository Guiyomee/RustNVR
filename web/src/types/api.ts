// Mirror of crates/rustnvr-api/src/dto.rs (API v1). Change both together and
// update API_CONTRACT.md. Times are Unix milliseconds (UTC).

export interface Capabilities {
  api_version: number
  live_video: boolean
  live_transcode: boolean
  /** Finding cameras on the network (`POST /discovery/scan`, `/discovery/streams`). */
  discovery?: boolean
  live_webrtc: boolean
  snapshots: boolean
  events: boolean
  motion_detection: boolean
  camera_management: boolean
  camera_probe: boolean
  recordings: boolean
  export: boolean
  protect: boolean
  retention: boolean
  settings_write: boolean
  mqtt: boolean
  ai: boolean
  users: boolean
  /** Backup and restore of the configuration (`/backup`). */
  backup: boolean
  /** Pan, tilt, zoom and presets of motorised cameras (`/cameras/{id}/ptz`), for cameras with `ptz`. */
  ptz?: boolean
  /** USB webcams (V4L2) can be added as cameras (`usb` in the camera input, `/devices/video` lists them). */
  usb_cameras?: boolean
}

export type CapabilityKey = Exclude<keyof Capabilities, 'api_version'>

export interface SystemInfo {
  version: string
  started_at_ms: number
  uptime_seconds: number
  auth_enabled: boolean
  user: string | null
  cpu_percent: number | null
  cpu_history: number[]
  memory_total_bytes: number | null
  memory_used_bytes: number | null
  process_cpu_percent: number | null
  process_rss_bytes: number | null
  /** RustNVR with the FFmpeg processes it starts, in % of one core (basis of `docker stats`). */
  service_cpu_percent?: number | null
  /** RustNVR with FFmpeg: memory in use (in a container, the `docker stats` figure). */
  service_memory_bytes?: number | null
  service_memory_limit_bytes?: number | null
  /** What `service_*` measured. */
  service_scope?: 'container' | 'service' | 'processes' | null
  cameras_total: number
  cameras_enabled: number
  cameras_online: number
  /** Storage destinations short of space (names). */
  low_space: string[]
  /** Storage destinations that cannot be used (share not mounted, not writable, not answering). */
  storage_problems?: StorageProblem[]
  /** The executable was replaced on disk (update installed): restart the server. */
  restart_needed: boolean
  /** A configuration backup was restored: restart the server to use it. */
  config_restored?: boolean
  /** A newer version was published on the watched repository. */
  update_available: string | null
  /** Graphics cards of the machine (Linux). */
  gpus?: Gpu[]
  /** H.264 conversions on the graphics card. */
  hwaccel?: HwAccel
}

/** State of the H.264 conversions on the graphics card (`live.hwaccel`). */
export interface HwAccel {
  mode: 'auto' | 'off'
  /** `testing`: test conversion running; `no_card`: no NVIDIA/Intel/AMD card usable; `failed`: see `detail`. */
  state: 'off' | 'testing' | 'active' | 'no_card' | 'failed'
  /** `nvenc` (NVIDIA) or `vaapi` (Intel, AMD), when active. */
  api?: 'nvenc' | 'vaapi' | null
  card: string | null
  /** VAAPI render node (null for NVENC). */
  device: string | null
  /** VAAPI driver chosen by name (`iHD`, `i965`), null for the default one. */
  driver: string | null
  /** The VAAPI driver offers constant QP only: live at a constant quality, no bit rate cap. */
  constant_qp?: boolean
  /** Why the test failed, or the last conversion redone on the processor. */
  detail: string | null
}

/** A graphics card of the machine (`GET /api/v1/system`). */
export interface Gpu {
  id: string
  name: string
  vendor: 'intel' | 'nvidia' | 'amd' | 'other'
  driver: string | null
  /** Device used to compute on it (`/dev/dri/renderD128`, `/dev/nvidia0`). */
  device: string | null
  /** `not_passed`: its device is not given to the container; `display_only`: cannot compute (VM adapter…). */
  status: 'usable' | 'not_passed' | 'no_permission' | 'display_only'
  /** Busy share over the last 2 s, all its users (Intel, AMD). */
  busy_percent: number | null
}

export type CameraStatus = 'online' | 'connecting' | 'retrying' | 'idle' | 'stopped' | 'disabled'

export interface StreamStats {
  frames: number
  key_frames: number
  bytes: number
  lost_packets: number
  sessions: number
  failures: number
  dropped_frames: number
}

export interface RecorderStats {
  segments: number
  frames_written: number
  bytes_written: number
  frames_skipped: number
  write_errors: number
  discontinuities: number
}

export interface Camera {
  id: string
  name: string
  enabled: boolean
  recording_enabled: boolean
  has_sub_stream: boolean
  endpoint: string
  status: CameraStatus
  attempt: number | null
  retry_in_ms: number | null
  last_error: string | null
  online_since_ms: number | null
  codec: string | null
  width: number | null
  height: number | null
  fps: number | null
  bitrate_kbps: number | null
  /** Sub stream status (live view source), when configured. */
  sub_status: CameraStatus | null
  /** Added from the interface (editable); false for cameras of config.toml. */
  managed: boolean
  /** Current settings of a managed camera (never the password). */
  edit: CameraEdit | null
  stream: StreamStats
  /** Destinations this camera records to (empty when not recording). */
  record_destinations: string[]
  record_mode: RecordMode
  /** Per-camera retention (in addition to the destination's). */
  retention_days: number | null
  max_size_gb: number | null
  recorder: RecorderStats | null
  motion: MotionState
  /** The sound is recorded with the video (when the camera sends AAC). */
  record_audio: boolean
  /** Sound received from the camera (`mp4a.40.2 16000 Hz`), while online. */
  audio: string | null
  /** Privacy masks `[x1, y1, x2, y2]` (picture fractions), blacked out on the live view and playback. */
  privacy_masks?: number[][]
  /** An ONVIF service is configured: pan, tilt, zoom and presets offered (GET /cameras/{id}/ptz tells whether the camera has them). */
  ptz?: boolean
}

/** Motion detection state of a camera. */
export interface MotionState {
  enabled: boolean
  /** Pictures are being analysed. */
  running: boolean
  /** An event is ongoing. */
  active: boolean
  /** Last decoder failure. */
  error: string | null
}

/** Polygon in picture fractions (0..1). */
export interface MotionZone {
  name: string
  points: [number, number][]
  /** 1 (only large changes) to 100 (the slightest change). */
  sensitivity: number
  /** Objects (optional AI) that count only in this zone or the other zones listing them. */
  objects?: string[]
}

/** GET/PUT /api/v1/cameras/{id}/motion. */
export interface MotionSettings {
  enabled: boolean
  sensitivity: number
  /** Empty: the whole picture. */
  zones: MotionZone[]
  /** Objects recognized for this camera (optional AI); empty: all those of the settings. */
  objects?: string[]
  /** Privacy masks: hidden from view, ignored by detection. */
  masks?: PrivacyMask[]
  /** Lines whose crossings are counted. */
  lines?: CrossingLine[]
}

/** A line whose crossings are counted; `forward`: from the left of `a → b` to its right (the arrow). */
export interface CrossingLine {
  name: string
  a: [number, number]
  b: [number, number]
  direction: 'both' | 'forward' | 'backward'
  /** Objects (optional AI) one of which must be seen at the line for a crossing to count; absent or empty: any motion. */
  objects?: string[]
  /** Count who is inside: the arrow direction enters, the other leaves (needs both directions). */
  presence?: boolean
}

/** A storage destination that cannot be used. */
export interface StorageProblem {
  name: string
  /** What is wrong (French). */
  problem: string
}

/** A crossing of a line (WebSocket `crossing`). */
export interface Crossing {
  camera: string
  line: string
  direction: 'forward' | 'backward'
  at_ms: number
  /** Crossings of the line since local midnight. */
  today: number
  /** Object recognized at the line (lines limited to objects). */
  object: string | null
}

/** A rectangle hidden from view, `[x1, y1, x2, y2]` in picture fractions. */
export interface PrivacyMask {
  name: string
  rect: [number, number, number, number]
}

/** Object recognition provider (`[ai].provider`). */
export type AiProvider = { type: 'onnx'; model: string } | { type: 'deepstack_http'; url: string; api_key_env?: string }

/** `[ai]` settings (GET/PUT /api/v1/ai). */
export interface AiSettings {
  /** Absent: recognition off. */
  provider?: AiProvider
  min_score: number
  /** Labels kept; empty: all. */
  labels: string[]
  timeout_seconds: number
  max_pictures: number
  interval_seconds: number
  /** Keep every analysed picture in the default destination, to replay the AI on them. */
  keep_pictures?: boolean
}

/** GET /api/v1/ai. */
export interface AiView {
  settings: AiSettings
  /** Provider running (`onnx`, `deepstack_http`), if any. */
  active: string | null
  /** Where its analyses run (`CPU`, `GPU`…): `CPU` for the built-in one, as last reported by CodeProject.AI. */
  device?: string | null
  /** Why the configured provider is not running. */
  error: string | null
  onnx_available: boolean
  detection_ready: boolean
  managed: boolean
  /** Models of the built-in recognition that can be downloaded. */
  models: AiModel[]
}

/** A downloadable model (`POST /api/v1/ai/models/{id}`). */
export interface AiModel {
  id: string
  name: string
  bytes: number
  license: string
  /** What to expect (speed, accuracy). */
  hint: string
  /** Where it is (or will be) on the server. */
  path: string
  downloaded: boolean
}

/** POST /api/v1/ai/test?camera=… */
/** Answer of `POST /api/v1/ai/probe` (server checked with a test pattern, not stored). */
export interface AiProbeResult {
  ok: boolean
  ms: number
  device: string | null
  error: string | null
}

export interface AiTestResult {
  ms: number
  /** Where it ran (`CPU`, `GPU`…), when the provider tells. */
  device?: string | null
  objects: (DetectedObject & { kept: boolean })[]
  /** The analysed picture (data URL). */
  image: string
}

/** GET /api/v1/mqtt (never contains the password). */
export interface MqttView {
  enabled: boolean
  host: string
  port: number
  tls: boolean
  /** PEM file of the broker's certificate authority, on the server. */
  ca_file: string | null
  username: string | null
  has_password: boolean
  client_id: string
  topic_prefix: string
  discovery: boolean
  discovery_prefix: string
  /** Settings come from the interface (mqtt.ui.toml) rather than config.toml. */
  managed: boolean
  connected: boolean
  error: string | null
}

/** PUT /api/v1/mqtt. */
export interface MqttInput {
  enabled: boolean
  host: string
  port: number
  tls: boolean
  ca_file: string | null
  username: string | null
  /** Omit (or null) to keep the stored password; "" removes it. */
  password?: string | null
  client_id: string
  topic_prefix: string
  discovery: boolean
  discovery_prefix: string
}

export type Transport = 'tcp' | 'udp'

/** `continuous`: all the time; `motion`: around motion events only (needs detection). */
export type RecordMode = 'continuous' | 'motion'

export interface CameraEdit {
  main_url: string
  sub_url: string | null
  username: string | null
  has_password: boolean
  transport: Transport
  /** Port of the ONVIF service (PTZ), null when not set. */
  onvif_port?: number | null
  /** USB webcam settings (`main_url` empty), null for an RTSP camera. */
  usb?: UsbSettings | null
}

export interface StreamInput {
  url: string
  username?: string | null
  /** Omit (or null) to keep the stored password when editing. */
  password?: string | null
  transport?: Transport
}

/** Body of POST /cameras and PUT /cameras/{id}. */
export interface CameraInput {
  id?: string
  name: string
  enabled: boolean
  record: boolean
  /** RTSP main stream; null for a USB webcam. */
  main: StreamInput | null
  sub: StreamInput | null
  /** A USB webcam instead of RTSP streams. */
  usb?: UsbSettings | null
  /** `local` and/or destination ids. */
  record_destinations: string[]
  record_mode: RecordMode
  retention_days: number | null
  max_size_gb: number | null
  /** Absent keeps the current choice (on for a new camera). */
  record_audio?: boolean
  /** Port of the ONVIF service, for PTZ (null: none). */
  onvif_port?: number | null
}

/** A USB webcam (V4L2) read by FFmpeg (`usb` of a camera). */
export interface UsbSettings {
  /** `/dev/video0` or `/dev/v4l/by-id/…`. */
  device: string
  /** `mjpeg`, `yuyv422`… or `h264` (copied, nothing encoded); null: the webcam's default. */
  input_format?: string | null
  width?: number | null
  height?: number | null
  framerate?: number | null
  /** Bit rate of the H.264 encoded by RustNVR (kbit/s, 2000 by default). */
  bitrate_kbps?: number
}

/** `GET /api/v1/devices/video`: a video device of the machine. */
export interface VideoDevice {
  path: string
  /** Same device under `/dev/v4l/by-id` (does not change when plugged elsewhere). */
  by_id: string | null
  name: string | null
  /** Empty for the extra nodes of a webcam (no pictures). */
  formats: { format: string; compressed: boolean; description: string; sizes: string[] }[]
  error: string | null
}

/** A position stored on a motorised camera. */
export interface PtzPreset {
  token: string
  name: string
}

/** Speeds of a continuous move, each from -1 to 1. */
export interface PtzMove {
  pan?: number
  tilt?: number
  zoom?: number
}

export interface Destination {
  id: string
  name: string
  kind: 'local' | 'nas' | 's3' | 'sftp'
  path: string
  /** Editable from the interface (`local`: retention only). */
  managed: boolean
  require_mount: boolean
  available: boolean
  unavailable_reason: string | null
  disk: Disk | null
  retention_days: number | null
  max_size_gb: number | null
  /** Free space kept (GB), deleting the oldest recordings if needed. */
  min_free_gb: number | null
  /** Usage above the threshold, or free space below `min_free_gb`. */
  low_space: boolean
  /** S3 or SFTP settings (never secrets); the path is then the local outbox. */
  remote: RemoteView | null
  /** Upload state of a remote destination. */
  upload: UploadView | null
  used_by: string[]
  /** Only filled by GET /storage. */
  recordings_bytes: number | null
  /** Days until its disk is full at its own write rate (GET /storage, ≥ 1 h of recordings, not S3/SFTP). */
  estimated_days_until_full?: number | null
}

export interface DestinationInput {
  id?: string
  name?: string | null
  path?: string | null
  require_mount?: boolean
  retention_days: number | null
  max_size_gb: number | null
  min_free_gb?: number | null
  /** Secrets left out keep the stored ones. */
  remote?: RemoteInput | null
}

export type RemoteView =
  | { type: 's3'; endpoint: string | null; region: string; bucket: string; prefix: string; access_key_id: string; has_secret: boolean }
  | { type: 'sftp'; host: string; port: number; username: string; remote_dir: string; host_key: string | null; private_key: string | null; has_password: boolean }

export type RemoteInput =
  | { type: 's3'; endpoint: string | null; region: string; bucket: string; prefix: string; access_key_id: string; secret_access_key?: string | null }
  | { type: 'sftp'; host: string; port: number; username: string; password?: string | null; private_key?: string | null; remote_dir: string; host_key: string | null }

export interface UploadView {
  /** Finished segments waiting in the outbox. */
  pending: number
  uploaded: number
  uploaded_bytes: number
  last_upload_ms: number | null
  error: string | null
  /** SFTP: fingerprint of the server key. */
  host_key: string | null
}

export interface ProbeInput {
  url: string
  username?: string | null
  password?: string | null
  transport?: Transport
}

export interface ProbeResult {
  streams: { media: string; encoding: string; clock_rate: number }[]
  selected: number | null
  video: { codec: string; rfc6381_codec: string; width: number; height: number; time_to_key_frame_ms: number } | null
  /** Live preview of the camera (motion JPEG for an <img>, single use, 2 minutes), when video came. */
  preview?: string | null
}

export interface Disk {
  total_bytes: number
  used_bytes: number
  free_bytes: number
}

export interface CameraStorage {
  camera: string
  bytes: number
  segments: number
  oldest_ms: number | null
  newest_ms: number | null
}

export interface DayStorage {
  day: string
  bytes: number
}

export interface Storage {
  recordings_dir: string
  destinations: Destination[]
  /** Destination used by default (the local disk if it no longer exists). */
  default_destination: string
  disk: Disk | null
  recordings_bytes: number
  cameras: CameraStorage[]
  days: DayStorage[]
  fill_rate_bytes_per_day: number | null
  estimated_days_until_full: number | null
  retention_configured: boolean
  computed_at_ms: number
}

export interface Recording {
  camera: string
  start_ms: number
  end_ms: number
  duration_ms: number
  bytes: number
  /** Empty in demo mode: there is no file behind a fictitious recording. */
  url: string
}

export type WsMessage =
  | { type: 'system'; system: SystemInfo }
  | { type: 'camera'; camera: Camera }
  | { type: 'cameras'; cameras: Camera[] }
  | { type: 'segment'; recording: Recording }
  | { type: 'event'; event: NvrEvent }
  | { type: 'recorder_error'; camera: string; message: string }
  | { type: 'crossing'; crossing: Crossing }

export interface ApiErrorBody {
  error: { code: string; message: string }
}

// ---- Not implemented by the backend yet (contracts in API_CONTRACT.md) ----

export type EventKind = 'motion' | 'person' | 'vehicle' | 'animal'

/** `GET /api/v1/events` item. Only `motion` exists today (no object recognition). */
export interface NvrEvent {
  id: string
  camera: string
  kind: EventKind
  start_ms: number
  /** null while ongoing. */
  end_ms: number | null
  /** Picture taken shortly after the start; null when none. */
  thumbnail_url: string | null
  /** Largest changed fraction of a zone (or of the picture), 0..1. */
  score: number | null
  /** Zones with motion (empty without zones). */
  zones: string[]
  /** Objects recognized in the thumbnail by the optional AI provider (empty without it). */
  objects: DetectedObject[]
}

export interface DetectedObject {
  /** Provider label (`person`, `car`…). */
  label: string
  /** Confidence, 0..1. */
  score: number
  /** Box as fractions of the picture: [x_min, y_min, x_max, y_max]. */
  bbox?: [number, number, number, number]
  /** Detection zone the object is in. */
  zone?: string
}

/** `[recording]` of the configuration. */
export interface RecordingSettings {
  segment_seconds: number
  fragment_millis: number
  queue_frames: number
  max_timestamp_jump_seconds: number
  retry_after_error_seconds: number
  pre_roll_seconds: number
  post_roll_seconds: number
}

/** `[motion]` of the configuration. */
export interface MotionGlobalSettings {
  fps: number
  width: number
  post_seconds: number
  max_event_seconds: number
  events_retention_days: number
}

/** GET /api/v1/settings. */
export interface GeneralSettings {
  recording: RecordingSettings
  motion: MotionGlobalSettings
  live_transcode: boolean
  live_max_bitrate_kbps: number
  /** H.264 conversions on the graphics card (`live.hwaccel`); state in `System.hwaccel`. */
  live_hwaccel?: 'auto' | 'off'
  low_space_percent: number
  /** Destination of cameras whose recording is enabled without choosing one. */
  default_destination: string
  /** Read-only here. */
  ffmpeg: string
  /** Changed from the interface (settings.ui.toml). */
  managed: boolean
}

/** PUT /api/v1/settings; `default_destination` absent keeps the current one. */
export type GeneralSettingsInput = Omit<GeneralSettings, 'ffmpeg' | 'managed' | 'default_destination'> & { default_destination?: string }

/** Severity of a log message. */
export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace'

/** One message of GET /api/v1/logs. */
export interface LogEntry {
  seq: number
  time_ms: number
  level: LogLevel
  /** Module that wrote it. */
  target: string
  camera: string | null
  message: string
}

/** `POST /cameras/{id}/export`: a prepared extract, downloaded from `url`. */
export interface ExportResult {
  url: string
  name: string
  bytes: number
  duration_ms: number
  start_ms: number
  h264: boolean
}

/** GET /api/v1/updates: check for new versions. */
export interface UpdateStatus {
  current: string
  /** Watched repository (web address); null: no check. */
  repository: string | null
  latest: string | null
  available: boolean
  name: string | null
  /** Release notes (Markdown, shown as plain text). */
  notes: string | null
  url: string | null
  published_at: string | null
  checked_at_ms: number | null
  error: string | null
  /** How this RustNVR runs: installation instructions to show. */
  install: 'docker' | 'systemd' | 'other'
}

/** A protected period (`GET /api/v1/protected`): never deleted by retention. */
export interface ProtectedRange {
  id: number
  camera: string
  start_ms: number
  end_ms: number
  note: string
  created_ms: number
}

/** A webhook (`GET /api/v1/webhooks`): HTTP POST with the event as JSON. */
export interface Webhook {
  name: string
  url: string
  enabled: boolean
  /** Camera identifiers; empty: all. */
  cameras: string[]
  /** Object labels; empty: every event. */
  objects: string[]
  events: ('motion' | 'objects' | 'end' | 'crossing' | 'storage' | 'camera')[]
  /** The picture is sent (base64 in the JSON). */
  image: boolean
}

/** POST /api/v1/webhooks/test */
export interface WebhookTestResult {
  status: number | null
  ms: number
  error: string | null
}

/** GET /api/v1/events/stats?days=7|30 */
export interface EventStats {
  from_ms: number
  to_ms: number
  days: number
  total: number
  /** Every day of the period (`YYYY-MM-DD`, server time), oldest first. */
  by_day: { key: string; count: number }[]
  /** Hours 0 to 23. */
  by_hour: number[]
  by_camera: { key: string; count: number }[]
  by_object: { key: string; count: number }[]
  without_object: number
}

/** GET /api/v1/crossings?days=7|30: crossings of the lines drawn on the cameras. */
export interface CrossingStats {
  from_ms: number
  to_ms: number
  days: number
  lines: {
    camera: string
    line: string
    total: number
    forward: number
    backward: number
    today: number
    by_day: { key: string; count: number }[]
    /** Who is inside, for a line counting the presence (`presence`). */
    present?: number | null
  }[]
}

/** A device found by `POST /api/v1/discovery/scan`. */
export interface FoundDevice {
  address: string
  /** Answered ONVIF WS-Discovery. */
  onvif: boolean
  xaddr: string | null
  name: string | null
  hardware: string | null
  /** RTSP port that answered the scan. */
  rtsp_port: number | null
  /** RTSP `Server` header. */
  server: string | null
  /** Camera already configured at this address. */
  camera: string | null
}

/** `POST /api/v1/discovery/scan`. */
export interface DiscoveryScan {
  /** Network scanned for RTSP; null: only ONVIF WS-Discovery. */
  network: string | null
  devices: FoundDevice[]
  took_ms: number
}

/** A stream offered by `POST /api/v1/discovery/streams`. */
export interface FoundStream {
  url: string
  label: string
  codec: string | null
  width: number | null
  height: number | null
  /** H.264 or H.265: RustNVR can record it. */
  recordable: boolean
}

/** `POST /api/v1/discovery/streams`. */
export interface DiscoveryStreams {
  onvif: boolean
  manufacturer: string | null
  model: string | null
  streams: FoundStream[]
  error: string | null
  /** Port of the ONVIF device service that answered (PTZ). */
  onvif_port?: number | null
}

export interface DiscoveryStreamsInput {
  address: string
  xaddr?: string | null
  rtsp_port?: number | null
  server?: string | null
  username?: string | null
  password?: string | null
}

/** What restoring a backup would do (`POST /backup/inspect`). */
export interface BackupPreview {
  created_ms: number
  /** Version of RustNVR that made it. */
  rustnvr: string
  files: { name: string; state: 'same' | 'changed' | 'new' | 'not_in_backup'; problem?: string | null }[]
  /** Cameras once every file of the backup is restored. */
  cameras: { id: string; name: string }[]
  /** Cameras whose detection settings (zones, lines, masks) are in it. */
  motion: string[]
  /** Why the restored configuration would not load (empty when it would). */
  problems: string[]
}

/** What a restore did (`POST /backup/restore`). */
export interface BackupRestored {
  files: string[]
  motion: number
  restart_needed: boolean
}

/** Kept pictures of one camera on one day (`GET /ai/captures`). */
export interface CaptureDay {
  camera: string
  /** `YYYY-MM-DD` (UTC). */
  day: string
  count: number
  bytes: number
}

/** A kept picture and what the recognition found in it then. */
export interface CapturePicture {
  file: string
  at_ms: number
  event: number | null
  picture: number | null
  line: string | null
  provider: string
  ms: number
  found: DetectedObject[]
  /** Labels that counted for the camera (objects, zones, line). */
  kept: string[]
}

/** The recognition replayed on the kept pictures of a day (`POST /ai/replay`). */
export interface ReplayResult {
  provider: string
  pictures: { before: CapturePicture; found: DetectedObject[]; kept: string[]; ms: number; changed: boolean }[]
  changed: number
  /** Pictures of the day left out (beyond 200). */
  skipped: number
}
