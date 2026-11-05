//! JSON documents of API v1. Field names are `snake_case`; times are Unix
//! milliseconds (UTC). `web/src/types/api.ts` mirrors these types: change both
//! together and update `API_CONTRACT.md`.

use serde::Serialize;

/// Which features the backend actually implements. The interface uses this to
/// show "not available" instead of guessing.
#[derive(Clone, Debug, Serialize)]
pub struct CapabilitiesDto {
    /// API version.
    pub api_version: u32,
    /// Live video to the browser (`/cameras/{id}/live`, fMP4 over WebSocket for MSE).
    pub live_video: bool,
    /// On-demand H.264 conversion of the live view (`?transcode=h264`).
    pub live_transcode: bool,
    /// Finding cameras on the network (`POST /discovery/scan`, `/discovery/streams`).
    pub discovery: bool,
    /// Low-latency live view over WebRTC (WHEP, `POST /cameras/{id}/live/webrtc`), prototype.
    pub live_webrtc: bool,
    /// Still images from cameras.
    pub snapshots: bool,
    /// Motion or other events.
    pub events: bool,
    /// Motion detection and zones.
    pub motion_detection: bool,
    /// Adding/editing/deleting cameras through the API.
    pub camera_management: bool,
    /// Testing a camera connection through the API.
    pub camera_probe: bool,
    /// Listing and downloading recorded segments.
    pub recordings: bool,
    /// Exporting an extract as one MP4 file (needs FFmpeg).
    pub export: bool,
    /// Protecting recordings from deletion (needs the database).
    pub protect: bool,
    /// Retention / quotas.
    pub retention: bool,
    /// Changing settings through the API.
    pub settings_write: bool,
    /// MQTT / Home Assistant.
    pub mqtt: bool,
    /// Optional AI providers.
    pub ai: bool,
    /// User management.
    pub users: bool,
}

/// Process and host status.
#[derive(Clone, Debug, Serialize)]
pub struct SystemDto {
    /// RustNVR version.
    pub version: String,
    /// Process start time.
    pub started_at_ms: i64,
    /// Seconds since start.
    pub uptime_seconds: u64,
    /// Whether HTTP authentication is enabled.
    pub auth_enabled: bool,
    /// Authenticated user name, if authentication is enabled.
    pub user: Option<String>,
    /// Host CPU usage in percent of all cores (Linux only).
    pub cpu_percent: Option<f32>,
    /// Last host CPU samples, oldest first, one every 2 seconds (at most 30).
    pub cpu_history: Vec<f32>,
    /// Host RAM.
    pub memory_total_bytes: Option<u64>,
    /// Host RAM in use.
    pub memory_used_bytes: Option<u64>,
    /// RustNVR CPU usage in percent of one core.
    pub process_cpu_percent: Option<f32>,
    /// RustNVR resident memory.
    pub process_rss_bytes: Option<u64>,
    /// RustNVR and the programs it starts (FFmpeg): CPU in percent of one
    /// core, the basis of `docker stats`.
    pub service_cpu_percent: Option<f32>,
    /// RustNVR and the programs it starts: memory in use (in a container, the
    /// figure of `docker stats`, file cache in active use included).
    pub service_memory_bytes: Option<u64>,
    /// Memory limit of the container or service, when one is set.
    pub service_memory_limit_bytes: Option<u64>,
    /// What `service_*` measured: `container`, `service` (systemd) or
    /// `processes` (RustNVR and its child processes).
    pub service_scope: Option<&'static str>,
    /// Configured cameras.
    pub cameras_total: usize,
    /// Enabled cameras.
    pub cameras_enabled: usize,
    /// Cameras currently streaming.
    pub cameras_online: usize,
    /// Storage destinations short of space (names); empty when all is well.
    pub low_space: Vec<String>,
    /// Storage destinations that cannot be used (share not mounted, not
    /// writable, not answering); empty when all is well.
    pub storage_problems: Vec<StorageProblemDto>,
    /// The executable was replaced on disk since startup (an update was
    /// installed): restart the server to use it.
    pub restart_needed: bool,
    /// A newer version was published on the watched repository.
    pub update_available: Option<String>,
    /// Graphics cards of the machine (Linux), and whether RustNVR may use them.
    pub gpus: Vec<GpuDto>,
    /// H.264 conversions on the graphics card (`live.hwaccel`).
    pub hwaccel: HwAccelDto,
}

/// State of the H.264 conversions on the graphics card.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct HwAccelDto {
    /// Setting: `auto` or `off`.
    pub mode: &'static str,
    /// `off` (setting), `testing` (test conversion running), `active`
    /// (conversions on the card), `no_card` (no Intel/AMD card usable by
    /// RustNVR) or `failed` (the test conversion failed: see `detail`).
    pub state: &'static str,
    /// How the card is used: `nvenc` (NVIDIA) or `vaapi` (Intel, AMD).
    pub api: Option<&'static str>,
    /// Card in use.
    pub card: Option<String>,
    /// Its render node.
    pub device: Option<String>,
    /// VAAPI driver chosen by name (`iHD`, `i965`), `None` for the default.
    pub driver: Option<String>,
    /// Why the test failed, or the last conversion redone on the processor.
    pub detail: Option<String>,
}

impl Default for HwAccelDto {
    fn default() -> Self {
        Self {
            mode: "off",
            state: "off",
            api: None,
            card: None,
            device: None,
            driver: None,
            detail: None,
        }
    }
}

/// A graphics card of the machine.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct GpuDto {
    /// `card0`, `nvidia0`…
    pub id: String,
    /// Model, from the driver or `pci.ids` (vendor and PCI ids otherwise).
    pub name: String,
    /// `intel`, `nvidia`, `amd`, or `other` (display only: VM adapter…).
    pub vendor: &'static str,
    /// Kernel driver (`i915`, `xe`, `amdgpu`, `nvidia`…).
    pub driver: Option<String>,
    /// Device used to compute on it (`/dev/dri/renderD128`, `/dev/nvidia0`).
    pub device: Option<String>,
    /// `usable` (its device is here and may be opened), `not_passed` (its
    /// device is missing: not given to the container), `no_permission`, or
    /// `display_only` (cannot compute).
    pub status: &'static str,
    /// Share of time the card was busy over the last 2 s (all its users),
    /// when the driver tells (Intel, AMD).
    pub busy_percent: Option<f32>,
}

/// Check for new versions (`GET /api/v1/updates`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateStatus {
    /// Running version.
    pub current: String,
    /// Watched repository (web address); `None`: no check.
    pub repository: Option<String>,
    /// Latest version published there (its tag), once checked.
    pub latest: Option<String>,
    /// `latest` is newer than `current`.
    pub available: bool,
    /// Title of the latest release.
    pub name: Option<String>,
    /// Release notes (Markdown text, shown as plain text).
    pub notes: Option<String>,
    /// Web page of the latest release.
    pub url: Option<String>,
    /// Publication date (as given by the forge).
    pub published_at: Option<String>,
    /// Last check (milliseconds since the Unix epoch).
    pub checked_at_ms: Option<i64>,
    /// Why the last check failed.
    pub error: Option<String>,
    /// How this RustNVR runs, for the installation instructions:
    /// `docker`, `systemd` or `other`.
    pub install: String,
}

/// Connection status of a camera, summarized for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraStatus {
    /// Frames are flowing.
    Online,
    /// Connection attempt in progress.
    Connecting,
    /// Last attempt failed; waiting to retry.
    Retrying,
    /// Supervisor not started yet.
    Idle,
    /// Supervisor stopped (shutdown).
    Stopped,
    /// Camera disabled in the configuration.
    Disabled,
}

/// Ingestion counters since start.
#[derive(Clone, Debug, Default, Serialize)]
pub struct StreamStatsDto {
    /// Frames received.
    pub frames: u64,
    /// Key frames received.
    pub key_frames: u64,
    /// Bytes received.
    pub bytes: u64,
    /// RTP packets lost.
    pub lost_packets: u64,
    /// Sessions established.
    pub sessions: u64,
    /// Failed attempts or lost sessions.
    pub failures: u64,
    /// Frames dropped because a consumer was too slow.
    pub dropped_frames: u64,
}

/// Recorder counters since start.
#[derive(Clone, Debug, Serialize)]
pub struct RecorderStatsDto {
    /// Segments completed.
    pub segments: u64,
    /// Frames written.
    pub frames_written: u64,
    /// Bytes written.
    pub bytes_written: u64,
    /// Frames skipped (waiting for a key frame, or after an error).
    pub frames_skipped: u64,
    /// Write errors.
    pub write_errors: u64,
    /// Segments closed early by a discontinuity.
    pub discontinuities: u64,
}

/// A camera and its live state.
#[derive(Clone, Debug, Serialize)]
pub struct CameraDto {
    /// Identifier.
    pub id: String,
    /// Display name (the identifier if none is configured).
    pub name: String,
    /// Enabled in the configuration.
    pub enabled: bool,
    /// Continuous recording enabled.
    pub recording_enabled: bool,
    /// A sub stream is configured (not connected yet).
    pub has_sub_stream: bool,
    /// `rtsp://host:port`, never with path or credentials.
    pub endpoint: String,
    /// Summary status.
    pub status: CameraStatus,
    /// Consecutive connection attempt (connecting/retrying).
    pub attempt: Option<u32>,
    /// Delay before the next attempt (retrying).
    pub retry_in_ms: Option<u64>,
    /// Last error (retrying).
    pub last_error: Option<String>,
    /// Start of the current session (online).
    pub online_since_ms: Option<i64>,
    /// `h264` or `h265` (online).
    pub codec: Option<String>,
    /// Width in pixels (online).
    pub width: Option<u32>,
    /// Height in pixels (online).
    pub height: Option<u32>,
    /// Measured frame rate over the last sampling period.
    pub fps: Option<f32>,
    /// Measured bit rate over the last sampling period.
    pub bitrate_kbps: Option<u32>,
    /// Status of the sub stream (used for the live view), if configured.
    pub sub_status: Option<CameraStatus>,
    /// Added from the interface (editable); false for cameras of the configuration file.
    pub managed: bool,
    /// Current settings of a managed camera (never the password).
    pub edit: Option<CameraEditDto>,
    /// Ingestion counters.
    pub stream: StreamStatsDto,
    /// Destinations this camera records to (empty when recording is disabled).
    pub record_destinations: Vec<String>,
    /// `continuous` or `motion`.
    pub record_mode: &'static str,
    /// Per-camera retention in days, if any.
    pub retention_days: Option<u32>,
    /// Per-camera size limit (GB per destination), if any.
    pub max_size_gb: Option<u32>,
    /// Recorder counters summed over destinations, if recording is enabled.
    pub recorder: Option<RecorderStatsDto>,
    /// Motion detection.
    pub motion: MotionDto,
    /// The sound is recorded with the video (when the camera sends AAC).
    pub record_audio: bool,
    /// Sound received from the camera (`mp4a.40.2 16000 Hz`), while online.
    pub audio: Option<String>,
    /// Privacy masks (`[x1, y1, x2, y2]`, picture fractions), blacked out by
    /// the interface on the live view and playback.
    pub privacy_masks: Vec<[f32; 4]>,
}

/// Motion detection state of a camera.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MotionDto {
    /// Detection is enabled for this camera.
    pub enabled: bool,
    /// Pictures are being analysed.
    pub running: bool,
    /// An event is ongoing.
    pub active: bool,
    /// Last decoder failure.
    pub error: Option<String>,
}

/// A detection event (`GET /api/v1/events`).
#[derive(Clone, Debug, Serialize)]
pub struct EventDto {
    /// Identifier (a string, for forward compatibility).
    pub id: String,
    /// Camera identifier.
    pub camera: String,
    /// `motion`.
    pub kind: &'static str,
    /// Start (ms since the Unix epoch).
    pub start_ms: i64,
    /// End; `null` while ongoing.
    pub end_ms: Option<i64>,
    /// Largest changed fraction of a zone or of the picture, 0..1.
    pub score: f32,
    /// Zones with motion.
    pub zones: Vec<String>,
    /// Thumbnail URL, when one was taken.
    pub thumbnail_url: Option<String>,
    /// Objects recognized in the thumbnail by the optional AI provider.
    pub objects: Vec<rustnvr_core::DetectedObject>,
}

impl From<&rustnvr_core::DetectionEvent> for EventDto {
    fn from(e: &rustnvr_core::DetectionEvent) -> Self {
        Self {
            id: e.id.to_string(),
            camera: e.camera.to_string(),
            kind: e.kind.as_str(),
            start_ms: e.start_ms,
            end_ms: e.end_ms,
            score: e.score,
            zones: e.zones.clone(),
            // The thumbnail may be replaced by the picture where objects were
            // recognized: a new URL then, since thumbnails are cached for good.
            thumbnail_url: e.has_thumbnail.then(|| {
                let suffix = if e.objects.is_empty() {
                    ""
                } else {
                    "?objects=1"
                };
                format!("/api/v1/events/{}/thumbnail{suffix}", e.id)
            }),
            objects: e.objects.clone(),
        }
    }
}
/// Editable settings of a managed camera.
#[derive(Clone, Debug, Serialize)]
pub struct CameraEditDto {
    /// Main stream URL, without credentials.
    pub main_url: String,
    /// Sub stream URL, without credentials.
    pub sub_url: Option<String>,
    /// User name.
    pub username: Option<String>,
    /// Whether a password is stored (the password itself is never returned).
    pub has_password: bool,
    /// `tcp` or `udp`.
    pub transport: &'static str,
}

/// Result of a connection test.
#[derive(Clone, Debug, Serialize)]
pub struct ProbeDto {
    /// Streams announced by the camera.
    pub streams: Vec<ProbedStreamDto>,
    /// Index of the stream that would be recorded.
    pub selected: Option<usize>,
    /// First key frame, when video was received.
    pub video: Option<ProbedVideoDto>,
}

/// One announced stream.
#[derive(Clone, Debug, Serialize)]
pub struct ProbedStreamDto {
    /// `video`, `audio`…
    pub media: String,
    /// Encoding (`h264`, `h265`, `mpeg4-generic`…).
    pub encoding: String,
    /// RTP clock rate.
    pub clock_rate: u32,
}

/// Video received during a connection test.
#[derive(Clone, Debug, Serialize)]
pub struct ProbedVideoDto {
    /// `h264` or `h265`.
    pub codec: String,
    /// RFC 6381 codec string.
    pub rfc6381_codec: String,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Time to the first key frame.
    pub time_to_key_frame_ms: u64,
}

/// Disk holding the recordings.
#[derive(Clone, Debug, Serialize)]
pub struct DiskDto {
    /// Capacity.
    pub total_bytes: u64,
    /// Used (capacity minus space available to RustNVR).
    pub used_bytes: u64,
    /// Available to RustNVR.
    pub free_bytes: u64,
}

/// Recordings of one camera on disk.
#[derive(Clone, Debug, Serialize)]
pub struct CameraStorageDto {
    /// Camera identifier.
    pub camera: String,
    /// Bytes of finished segments.
    pub bytes: u64,
    /// Number of finished segments.
    pub segments: u64,
    /// Start of the oldest segment.
    pub oldest_ms: Option<i64>,
    /// Start of the newest segment.
    pub newest_ms: Option<i64>,
}

/// A recording destination with its state.
#[derive(Clone, Debug, Serialize)]
pub struct DestinationDto {
    /// Identifier (`local` for the local disk).
    pub id: String,
    /// Display name.
    pub name: String,
    /// `local`, `nas`, `s3` or `sftp`.
    pub kind: &'static str,
    /// Root directory.
    pub path: String,
    /// Added from the interface (editable). `local` is editable for retention only.
    pub managed: bool,
    /// Only used while on a mounted filesystem other than the system disk.
    pub require_mount: bool,
    /// Usable right now.
    pub available: bool,
    /// Why it is not usable, if it is not.
    pub unavailable_reason: Option<String>,
    /// Disk usage of the filesystem holding it.
    pub disk: Option<DiskDto>,
    /// Retention in days.
    pub retention_days: Option<u32>,
    /// Maximum size in GB.
    pub max_size_gb: Option<u32>,
    /// Free space kept (GB), if any.
    pub min_free_gb: Option<u32>,
    /// Usage above `storage.low_space_percent`, or free space below `min_free_gb`.
    pub low_space: bool,
    /// S3 or SFTP settings, without secrets.
    pub remote: Option<serde_json::Value>,
    /// Upload state of a remote destination.
    pub upload: Option<serde_json::Value>,
    /// Cameras recording to it.
    pub used_by: Vec<String>,
    /// Size of its finished segments (only in `GET /storage`, otherwise null).
    pub recordings_bytes: Option<u64>,
    /// Days until its disk is full at its own average write rate (only in
    /// `GET /storage`, with at least one hour of recordings; otherwise null).
    pub estimated_days_until_full: Option<f64>,
}

/// Recordings of one UTC day, all cameras.
#[derive(Clone, Debug, Serialize)]
pub struct DayStorageDto {
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    /// Bytes.
    pub bytes: u64,
}

/// Storage usage.
#[derive(Clone, Debug, Serialize)]
pub struct StorageDto {
    /// Recordings directory as configured.
    pub recordings_dir: String,
    /// Every destination (local disk first), with its usage.
    pub destinations: Vec<DestinationDto>,
    /// Destination used by default (`storage.default_destination`, the local
    /// disk if that destination no longer exists).
    pub default_destination: String,
    /// Disk usage of the recordings directory, if available on this platform.
    pub disk: Option<DiskDto>,
    /// Total size of finished segments.
    pub recordings_bytes: u64,
    /// Per camera.
    pub cameras: Vec<CameraStorageDto>,
    /// Per day, oldest first.
    pub days: Vec<DayStorageDto>,
    /// Average write rate over the recorded span (needs at least one hour of recordings).
    pub fill_rate_bytes_per_day: Option<f64>,
    /// Days until the disk is full at that rate. Not a retention policy: none exists yet.
    pub estimated_days_until_full: Option<f64>,
    /// Whether a retention policy exists (always false for now).
    pub retention_configured: bool,
    /// When these figures were computed (they are cached for 30 s).
    pub computed_at_ms: i64,
}

/// One finished recording segment.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RecordingDto {
    /// Camera identifier.
    pub camera: String,
    /// Wall-clock start.
    pub start_ms: i64,
    /// `start_ms + duration_ms`.
    pub end_ms: i64,
    /// Media duration.
    pub duration_ms: u64,
    /// File size.
    pub bytes: u64,
    /// URL of the MP4 file (supports HTTP Range requests).
    pub url: String,
}

/// Messages pushed on `/api/v1/ws`.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Sent on connection and every 5 seconds.
    System {
        /// Status.
        system: Box<SystemDto>,
    },
    /// The camera list changed (camera added, edited or removed): full list.
    Cameras {
        /// All cameras.
        cameras: Vec<CameraDto>,
    },
    /// A camera changed state (also sent for every camera on connection).
    Camera {
        /// Camera.
        camera: Box<CameraDto>,
    },
    /// A segment was finished.
    Segment {
        /// Segment.
        recording: RecordingDto,
    },
    /// A detection event started, changed or ended.
    Event {
        /// Event.
        event: EventDto,
    },
    /// The recorder of a camera failed to write.
    RecorderError {
        /// Camera identifier.
        camera: String,
        /// Error.
        message: String,
    },
    /// A line was crossed.
    Crossing {
        /// The crossing.
        crossing: CrossingDto,
    },
}

/// A storage destination that cannot be used.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct StorageProblemDto {
    /// Display name of the destination.
    pub name: String,
    /// What is wrong (French).
    pub problem: String,
}

/// A crossing of a line (WebSocket `crossing`).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CrossingDto {
    /// Camera identifier.
    pub camera: String,
    /// Name of the line.
    pub line: String,
    /// `forward` (the arrow of the interface) or `backward`.
    pub direction: &'static str,
    /// When, in milliseconds since the Unix epoch.
    pub at_ms: i64,
    /// Crossings of the line since local midnight.
    pub today: u64,
    /// Object recognized at the line (lines limited to objects), if any.
    pub object: Option<String>,
}

/// Error body: `{"error": {"code": "...", "message": "..."}}`.
#[derive(Clone, Debug, Serialize)]
pub struct ErrorBody {
    /// Error.
    pub error: ErrorDetail,
}

/// Error details.
#[derive(Clone, Debug, Serialize)]
pub struct ErrorDetail {
    /// Stable machine-readable code (`not_found`, `bad_request`, …).
    pub code: &'static str,
    /// Human-readable message.
    pub message: String,
}
