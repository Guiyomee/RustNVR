//! Shared state of the API and the background sampler feeding it.

use crate::control::NvrControl;
use crate::dto::{
    CameraDto, CameraEditDto, CameraStatus, MotionDto, RecorderStatsDto, StorageDto,
    StreamStatsDto, SystemDto,
};
use crate::sysinfo::{CpuTimes, GroupSample, MemInfo, ProcSample};
use crate::transcode::{LiveSettings, Transcoders};
use rustnvr_core::config::{
    LOCAL_DESTINATION, RecordMode, RemoteConfig, RetentionConfig, RtspTransport,
};
use rustnvr_core::{CameraId, EventBus, Secret, StreamState};
use rustnvr_motion::MotionStatus;
use rustnvr_recorder::{Destination, RecorderMetrics};
use rustnvr_remote::UploadStatus;
use rustnvr_storage::Database;
use rustnvr_stream::{LiveTap, StreamMetrics};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Sampling period of CPU, memory and stream rates.
pub(crate) const SAMPLE_PERIOD: Duration = Duration::from_secs(2);
const CPU_HISTORY: usize = 30;

/// Live handles of a connected stream.
#[derive(Clone)]
pub struct StreamSource {
    /// Latest state.
    pub state: watch::Receiver<StreamState>,
    /// Counters.
    pub metrics: Arc<StreamMetrics>,
}

/// What the API knows about one configured camera.
#[derive(Clone)]
pub struct CameraSource {
    /// Identifier.
    pub id: CameraId,
    /// Display name.
    pub name: Option<String>,
    /// Enabled in the configuration.
    pub enabled: bool,
    /// Recording enabled.
    pub recording_enabled: bool,
    /// A sub stream is configured.
    pub has_sub_stream: bool,
    /// Redacted endpoint (`rtsp://host:port`).
    pub endpoint: String,
    /// Present for enabled cameras.
    pub stream: Option<StreamSource>,
    /// One per recording destination (empty when recording is disabled).
    pub recorders: Vec<Arc<RecorderMetrics>>,
    /// Destinations this camera records to (empty when recording is disabled).
    pub record_destinations: Vec<String>,
    /// Continuous or motion-triggered recording.
    pub record_mode: RecordMode,
    /// Per-camera retention.
    pub retention: RetentionConfig,
    /// Live frames of the main stream, present for enabled cameras.
    pub live: Option<LiveTap>,
    /// Sub stream (live view only), present when configured and the camera is enabled.
    pub sub_stream: Option<StreamSource>,
    /// Live frames of the sub stream.
    pub live_sub: Option<LiveTap>,
    /// Editable settings, for cameras managed from the interface only
    /// (cameras of the configuration file are read-only and expose no URL path).
    pub edit: Option<CameraEditInfo>,
    /// Motion detection enabled for this camera.
    pub motion_enabled: bool,
    /// Detection state, while it runs.
    pub motion: Option<watch::Receiver<MotionStatus>>,
    /// The sound is recorded with the video.
    pub record_audio: bool,
    /// Privacy masks (`[x1, y1, x2, y2]`, picture fractions).
    pub privacy_masks: Vec<[f32; 4]>,
}

/// A recording destination known to the API (the local disk or a mounted share).
#[derive(Clone, Debug)]
pub struct DestinationInfo {
    /// Identifier (`local` for the local disk).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Root directory.
    pub root: PathBuf,
    /// Only used while on a mounted filesystem other than the system disk.
    pub require_mount: bool,
    /// Added from the interface (editable).
    pub managed: bool,
    /// Retention limits.
    pub retention: RetentionConfig,
    /// S3 or SFTP upload settings (the root is then the local outbox).
    pub remote: Option<RemoteConfig>,
    /// Upload state, for remote destinations.
    pub upload: Option<watch::Receiver<UploadStatus>>,
}

impl DestinationInfo {
    /// The recorder-level view, for availability checks.
    pub fn destination(&self) -> Destination {
        Destination {
            id: self.id.clone(),
            name: self.name.clone(),
            root: self.root.clone(),
            require_mount: self.require_mount,
        }
    }

    /// Whether this is the local recordings directory.
    pub fn is_local(&self) -> bool {
        self.id == LOCAL_DESTINATION
    }
}

/// Current settings of a managed camera, to prefill the edit form. Never
/// contains a password.
#[derive(Clone, Debug)]
pub struct CameraEditInfo {
    /// Main stream URL, without credentials.
    pub main_url: String,
    /// Sub stream URL, without credentials.
    pub sub_url: Option<String>,
    /// User name.
    pub username: Option<String>,
    /// Whether a password is stored.
    pub has_password: bool,
    /// Transport.
    pub transport: RtspTransport,
}

#[derive(Default)]
pub(crate) struct Live {
    pub cpu_percent: Option<f32>,
    pub cpu_history: VecDeque<f32>,
    pub memory: Option<MemInfo>,
    pub process_cpu_percent: Option<f32>,
    pub process_rss_bytes: Option<u64>,
    pub service_cpu_percent: Option<f32>,
    pub service: Option<GroupSample>,
    /// fps, kbps per camera.
    pub rates: HashMap<CameraId, (f32, u32)>,
    /// Graphics cards, with how busy they were.
    pub gpus: Vec<crate::dto::GpuDto>,
}

/// State shared by all request handlers.
pub struct ApiState {
    cameras: RwLock<Vec<CameraSource>>,
    destinations: RwLock<Vec<DestinationInfo>>,
    pub(crate) control: OnceLock<Arc<dyn NvrControl>>,
    pub(crate) recordings_dir: PathBuf,
    pub(crate) events: EventBus,
    pub(crate) auth: Option<(String, Secret)>,
    pub(crate) started_at: SystemTime,
    /// The executable file at startup (see [`ExeIdentity`]).
    pub(crate) exe: Option<ExeIdentity>,
    pub(crate) started: Instant,
    pub(crate) live: Mutex<Live>,
    pub(crate) storage_cache: tokio::sync::Mutex<Option<(Instant, StorageDto)>>,
    /// Segment durations by path, invalidated when the file size changes.
    pub(crate) durations: Mutex<HashMap<PathBuf, (u64, u64)>>,
    pub(crate) shutdown: CancellationToken,
    live_settings: RwLock<LiveSettings>,
    pub(crate) transcoders: Arc<Transcoders>,
    /// H.264 copies of recordings for browsers without H.265.
    pub(crate) playback: crate::playback::PlaybackConverter,
    /// Recent log messages, when the application captures them.
    pub(crate) logs: OnceLock<Arc<crate::logs::LogBuffer>>,
    /// Events and detection settings, once the database is open.
    pub(crate) database: OnceLock<Database>,
    /// Whether FFmpeg could be started (snapshots and motion detection need it).
    pub(crate) ffmpeg_available: OnceLock<bool>,
    /// Object recognition provider, when one is configured.
    pub(crate) ai_provider: RwLock<Option<String>>,
    /// Last check for new versions.
    pub(crate) updates: RwLock<crate::dto::UpdateStatus>,
    /// `storage.low_space_percent`.
    low_space_percent: std::sync::atomic::AtomicU8,
    /// Served over HTTPS (session cookies get `Secure`).
    pub(crate) https: std::sync::atomic::AtomicBool,
    /// Accounts exist in the database (authentication required).
    pub(crate) has_users: std::sync::atomic::AtomicBool,
    /// Failed login attempts.
    pub(crate) login_guard: crate::auth::LoginGuard,
    /// Names of the destinations short of space (updated every minute).
    low_space: RwLock<Vec<String>>,
    /// Destinations that cannot be used: name, problem (every minute).
    storage_problems: RwLock<Vec<(String, String)>>,
    /// `storage.default_destination`.
    default_destination: RwLock<String>,
    /// H.264 conversions on the graphics card: state shown, card in use.
    hw: RwLock<(crate::dto::HwAccelDto, Option<crate::hwaccel::HwEncoder>)>,
    /// One test conversion at a time.
    hw_test: tokio::sync::Mutex<()>,
}

impl ApiState {
    /// Creates the state. `auth` is `None` when authentication is disabled.
    pub fn new(
        cameras: Vec<CameraSource>,
        recordings_dir: PathBuf,
        events: EventBus,
        auth: Option<(String, Secret)>,
        live_settings: LiveSettings,
    ) -> Arc<Self> {
        Arc::new(Self {
            cameras: RwLock::new(cameras),
            destinations: RwLock::new(vec![DestinationInfo {
                id: LOCAL_DESTINATION.to_owned(),
                name: "Disque local".to_owned(),
                root: recordings_dir.clone(),
                require_mount: false,
                managed: false,
                retention: RetentionConfig::default(),
                remote: None,
                upload: None,
            }]),
            control: OnceLock::new(),
            recordings_dir,
            events,
            auth,
            started_at: SystemTime::now(),
            exe: ExeIdentity::current(),
            started: Instant::now(),
            live: Mutex::new(Live::default()),
            storage_cache: tokio::sync::Mutex::new(None),
            durations: Mutex::new(HashMap::new()),
            shutdown: CancellationToken::new(),
            live_settings: RwLock::new(live_settings),
            transcoders: Arc::new(Transcoders::default()),
            playback: crate::playback::PlaybackConverter::default(),
            logs: OnceLock::new(),
            database: OnceLock::new(),
            ffmpeg_available: OnceLock::new(),
            ai_provider: RwLock::new(None),
            updates: RwLock::new(crate::dto::UpdateStatus {
                current: env!("CARGO_PKG_VERSION").to_owned(),
                install: "other".to_owned(),
                ..Default::default()
            }),
            low_space_percent: std::sync::atomic::AtomicU8::new(90),
            https: std::sync::atomic::AtomicBool::new(false),
            has_users: std::sync::atomic::AtomicBool::new(false),
            login_guard: crate::auth::LoginGuard::default(),
            low_space: RwLock::new(Vec::new()),
            storage_problems: RwLock::new(Vec::new()),
            default_destination: RwLock::new(LOCAL_DESTINATION.to_owned()),
            hw: RwLock::new((crate::dto::HwAccelDto::default(), None)),
            hw_test: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        // A poisoned lock only means a sampler panicked mid-update: the data is still usable.
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replaces the camera list (after a camera was added, changed or removed).
    pub fn set_cameras(&self, cameras: Vec<CameraSource>) {
        *self.cameras.write().unwrap_or_else(|e| e.into_inner()) = cameras;
    }

    /// Enables events and detection settings. Returns false if already set.
    pub fn set_database(&self, database: Database) -> bool {
        self.database.set(database).is_ok()
    }

    /// Live view settings in effect, with the graphics card to convert on.
    pub(crate) fn live_settings(&self) -> LiveSettings {
        let mut s = self
            .live_settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        s.hw = self.hw.read().unwrap_or_else(|e| e.into_inner()).1.clone();
        s
    }

    /// State of the conversions on the graphics card.
    pub(crate) fn hwaccel_dto(&self) -> crate::dto::HwAccelDto {
        self.hw.read().unwrap_or_else(|e| e.into_inner()).0.clone()
    }

    /// Applies `live.hwaccel`: with `auto`, a test conversion on each usable
    /// card (Intel, AMD); conversions use the first that works, the processor
    /// otherwise. Runs for a few seconds at most per card: spawn it.
    pub async fn configure_hwaccel(self: Arc<Self>, mode: rustnvr_core::config::HwAccelMode) {
        use crate::dto::HwAccelDto;
        use crate::hwaccel::NoEncoder;
        let _one = self.hw_test.lock().await;
        let set = |dto: HwAccelDto, hw| {
            *self.hw.write().unwrap_or_else(|e| e.into_inner()) = (dto, hw);
        };
        if mode == rustnvr_core::config::HwAccelMode::Off {
            set(HwAccelDto::default(), None);
            tracing::info!("H.264 conversions on the processor (live.hwaccel = off)");
            return;
        }
        let auto = HwAccelDto {
            mode: "auto",
            ..HwAccelDto::default()
        };
        set(
            HwAccelDto {
                state: "testing",
                ..auto.clone()
            },
            None,
        );
        let gpus = tokio::task::spawn_blocking(|| crate::gpu::GpuSampler::default().sample())
            .await
            .unwrap_or_default();
        let ffmpeg = self
            .live_settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ffmpeg
            .clone();
        match crate::hwaccel::detect(&ffmpeg, &gpus, crate::hwaccel::NvidiaHints::read()).await {
            Ok(hw) => {
                tracing::info!(
                    card = %hw.card,
                    api = hw.api.as_str(),
                    device = hw.device.as_deref().unwrap_or("-"),
                    driver = hw.driver.as_deref().unwrap_or("default"),
                    "H.264 conversions on the graphics card"
                );
                set(
                    HwAccelDto {
                        state: "active",
                        api: Some(hw.api.as_str()),
                        card: Some(hw.card.clone()),
                        device: hw.device.clone(),
                        driver: hw.driver.clone(),
                        ..auto
                    },
                    Some(hw),
                );
            }
            Err(NoEncoder::NoCard) => {
                tracing::info!(
                    "no graphics card usable by RustNVR: H.264 conversions on the processor"
                );
                set(
                    HwAccelDto {
                        state: "no_card",
                        ..auto
                    },
                    None,
                );
            }
            Err(NoEncoder::Failed(reason)) => {
                tracing::warn!(%reason, "test conversion on the graphics card failed: H.264 conversions on the processor");
                set(
                    HwAccelDto {
                        state: "failed",
                        detail: Some(reason),
                        ..auto
                    },
                    None,
                );
            }
        }
    }

    /// A conversion failed on the card and is redone on the processor: shown
    /// with the state (the card stays in use for the next ones).
    pub(crate) fn note_hw_failure(&self, what: &str, reason: &str) {
        tracing::warn!(what, %reason, "conversion on the graphics card failed: redone on the processor");
        let mut hw = self.hw.write().unwrap_or_else(|e| e.into_inner());
        hw.0.detail = Some(format!(
            "dernière conversion refaite sur le processeur ({what}) : {reason}"
        ));
    }

    /// Replaces the live view settings (new conversions use them).
    pub fn set_live_settings(&self, settings: LiveSettings) {
        *self
            .live_settings
            .write()
            .unwrap_or_else(|e| e.into_inner()) = settings;
    }

    /// Makes the recent log messages available to `GET /logs`.
    pub fn set_logs(&self, logs: Arc<crate::logs::LogBuffer>) {
        let _ = self.logs.set(logs);
    }

    /// Directory of the H.264 copies of recordings (in the NVR data directory).
    pub fn set_playback_cache_dir(&self, dir: std::path::PathBuf) {
        self.playback.set_dir(dir);
    }

    /// Directory of the exported extracts (next to the H.264 cache).
    pub(crate) fn export_dir(&self) -> std::path::PathBuf {
        self.playback.export_dir()
    }

    /// Records whether FFmpeg works (snapshots, motion detection).
    pub fn set_ffmpeg_available(&self, available: bool) {
        let _ = self.ffmpeg_available.set(available);
    }

    /// Records the result of a check for new versions.
    pub fn set_update_status(&self, status: crate::dto::UpdateStatus) {
        *self.updates.write().unwrap_or_else(|e| e.into_inner()) = status;
    }

    /// Last check for new versions.
    pub fn update_status(&self) -> crate::dto::UpdateStatus {
        self.updates
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Records the object recognition provider in use, if any.
    pub fn set_ai_provider(&self, provider: Option<String>) {
        *self.ai_provider.write().unwrap_or_else(|e| e.into_inner()) = provider;
    }

    /// Usage (percent) above which a destination is short of space.
    pub fn set_low_space_percent(&self, percent: u8) {
        self.low_space_percent.store(percent, Ordering::Relaxed);
    }

    pub(crate) fn low_space_percent(&self) -> u8 {
        self.low_space_percent.load(Ordering::Relaxed)
    }

    /// Records the destinations that cannot be used (name, problem), shown
    /// by `GET /system`.
    pub fn set_storage_problems(&self, problems: Vec<(String, String)>) {
        *self
            .storage_problems
            .write()
            .unwrap_or_else(|e| e.into_inner()) = problems;
    }

    /// Records the destinations short of space (shown by `GET /system`).
    pub fn set_low_space(&self, names: Vec<String>) {
        *self.low_space.write().unwrap_or_else(|e| e.into_inner()) = names;
    }

    pub(crate) fn ffmpeg_ok(&self) -> bool {
        self.ffmpeg_available.get().copied().unwrap_or(false)
    }

    /// Enables camera management through the API. Returns false if a
    /// controller was already registered.
    pub fn set_control(&self, control: Arc<dyn NvrControl>) -> bool {
        self.control.set(control).is_ok()
    }

    /// Replaces the destination list (local disk first).
    pub fn set_destinations(&self, destinations: Vec<DestinationInfo>) {
        *self.destinations.write().unwrap_or_else(|e| e.into_inner()) = destinations;
    }

    /// Snapshot of the destinations.
    pub(crate) fn destinations(&self) -> Vec<DestinationInfo> {
        self.destinations
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Records the destination used by default (`storage.default_destination`).
    pub fn set_default_destination(&self, id: String) {
        *self
            .default_destination
            .write()
            .unwrap_or_else(|e| e.into_inner()) = id;
    }

    /// Destination used by default: the local disk if it no longer exists
    /// (as when recording is turned on).
    pub(crate) fn default_destination(&self) -> String {
        let id = self
            .default_destination
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let exists = self
            .destinations
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|d| d.id == id);
        if exists {
            id
        } else {
            LOCAL_DESTINATION.to_owned()
        }
    }

    /// Snapshot of the camera list.
    pub(crate) fn cameras(&self) -> Vec<CameraSource> {
        self.cameras
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn camera(&self, id: &str) -> Option<CameraSource> {
        self.cameras
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|c| c.id.as_str() == id)
            .cloned()
    }

    pub(crate) fn system_dto(&self) -> SystemDto {
        let cameras = self.cameras();
        let live = self.live();
        let online = cameras
            .iter()
            .filter(|c| {
                c.stream
                    .as_ref()
                    .is_some_and(|s| s.state.borrow().is_streaming())
            })
            .count();
        SystemDto {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at_ms: unix_ms(self.started_at),
            uptime_seconds: self.started.elapsed().as_secs(),
            auth_enabled: self.auth_required(),
            user: self.auth.as_ref().map(|(u, _)| u.clone()),
            cpu_percent: live.cpu_percent,
            cpu_history: live.cpu_history.iter().copied().collect(),
            memory_total_bytes: live.memory.map(|m| m.total_bytes),
            memory_used_bytes: live.memory.map(|m| m.used_bytes),
            process_cpu_percent: live.process_cpu_percent,
            process_rss_bytes: live.process_rss_bytes,
            service_cpu_percent: live.service_cpu_percent,
            service_memory_bytes: live.service.map(|g| g.memory_bytes),
            service_memory_limit_bytes: live.service.and_then(|g| g.memory_limit_bytes),
            service_scope: live.service.map(|g| g.scope.as_str()),
            cameras_total: cameras.len(),
            cameras_enabled: cameras.iter().filter(|c| c.enabled).count(),
            cameras_online: online,
            low_space: self
                .low_space
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            storage_problems: self
                .storage_problems
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(name, problem)| crate::dto::StorageProblemDto {
                    name: name.clone(),
                    problem: problem.clone(),
                })
                .collect(),
            restart_needed: self.exe.as_ref().is_some_and(ExeIdentity::replaced),
            update_available: {
                let u = self.updates.read().unwrap_or_else(|e| e.into_inner());
                u.latest.clone().filter(|_| u.available)
            },
            gpus: live.gpus.clone(),
            hwaccel: self.hwaccel_dto(),
        }
    }

    pub(crate) fn camera_dto(&self, c: &CameraSource) -> CameraDto {
        let rates = self.live().rates.get(&c.id).copied();
        let mut dto = CameraDto {
            id: c.id.to_string(),
            name: c.name.clone().unwrap_or_else(|| c.id.to_string()),
            enabled: c.enabled,
            recording_enabled: c.recording_enabled,
            has_sub_stream: c.has_sub_stream,
            endpoint: c.endpoint.clone(),
            status: CameraStatus::Disabled,
            attempt: None,
            retry_in_ms: None,
            last_error: None,
            online_since_ms: None,
            codec: None,
            width: None,
            height: None,
            fps: None,
            bitrate_kbps: None,
            stream: StreamStatsDto::default(),
            sub_status: c.sub_stream.as_ref().map(|s| status_of(&s.state.borrow())),
            managed: c.edit.is_some(),
            edit: c.edit.as_ref().map(|e| CameraEditDto {
                main_url: e.main_url.clone(),
                sub_url: e.sub_url.clone(),
                username: e.username.clone(),
                has_password: e.has_password,
                transport: match e.transport {
                    RtspTransport::Tcp => "tcp",
                    RtspTransport::Udp => "udp",
                },
            }),
            record_destinations: c.record_destinations.clone(),
            record_mode: c.record_mode.as_str(),
            retention_days: c.retention.retention_days,
            max_size_gb: c.retention.max_size_gb,
            motion: {
                let status = c
                    .motion
                    .as_ref()
                    .map(|m| m.borrow().clone())
                    .unwrap_or_default();
                MotionDto {
                    enabled: c.motion_enabled,
                    running: status.running,
                    active: status.active,
                    error: status.error,
                }
            },
            record_audio: c.record_audio,
            privacy_masks: c.privacy_masks.clone(),
            audio: None,
            // Totals over all destinations.
            recorder: (!c.recorders.is_empty()).then(|| {
                let sum = |f: fn(&RecorderMetrics) -> u64| c.recorders.iter().map(|r| f(r)).sum();
                RecorderStatsDto {
                    segments: sum(|r| r.segments.load(Ordering::Relaxed)),
                    frames_written: sum(|r| r.frames_written.load(Ordering::Relaxed)),
                    bytes_written: sum(|r| r.bytes_written.load(Ordering::Relaxed)),
                    frames_skipped: sum(|r| r.frames_skipped.load(Ordering::Relaxed)),
                    write_errors: sum(|r| r.write_errors.load(Ordering::Relaxed)),
                    discontinuities: sum(|r| r.discontinuities.load(Ordering::Relaxed)),
                }
            }),
        };
        let Some(stream) = &c.stream else {
            return dto;
        };
        let m = stream.metrics.snapshot();
        dto.stream = StreamStatsDto {
            frames: m.frames,
            key_frames: m.key_frames,
            bytes: m.bytes,
            lost_packets: m.lost_packets,
            sessions: m.sessions,
            failures: m.failures,
            dropped_frames: m.dropped_frames,
        };
        match &*stream.state.borrow() {
            StreamState::Idle => dto.status = CameraStatus::Idle,
            StreamState::Stopped => dto.status = CameraStatus::Stopped,
            StreamState::Connecting { attempt } => {
                dto.status = CameraStatus::Connecting;
                dto.attempt = Some(*attempt);
            }
            StreamState::Retrying {
                attempt,
                retry_in,
                error,
            } => {
                dto.status = CameraStatus::Retrying;
                dto.attempt = Some(*attempt);
                dto.retry_in_ms = Some(retry_in.as_millis() as u64);
                dto.last_error = Some(error.clone());
            }
            StreamState::Streaming {
                codec,
                width,
                height,
                since,
                audio,
                ..
            } => {
                dto.status = CameraStatus::Online;
                dto.audio = audio.clone();
                dto.codec = Some(codec.to_string());
                dto.width = Some(*width);
                dto.height = Some(*height);
                dto.online_since_ms = Some(unix_ms(*since));
                if let Some((fps, kbps)) = rates {
                    dto.fps = Some(fps);
                    dto.bitrate_kbps = Some(kbps);
                }
            }
        }
        dto
    }
}

fn status_of(state: &StreamState) -> CameraStatus {
    match state {
        StreamState::Idle => CameraStatus::Idle,
        StreamState::Connecting { .. } => CameraStatus::Connecting,
        StreamState::Streaming { .. } => CameraStatus::Online,
        StreamState::Retrying { .. } => CameraStatus::Retrying,
        StreamState::Stopped => CameraStatus::Stopped,
    }
}

/// Milliseconds since the Unix epoch (negative before it).
pub(crate) fn unix_ms(t: SystemTime) -> i64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// Updates CPU, memory and per-camera rates every [`SAMPLE_PERIOD`].
pub(crate) async fn run_sampler(state: Arc<ApiState>) {
    let mut cpu = CpuTimes::read();
    let mut proc = ProcSample::read();
    let mut group = GroupSample::read();
    let mut gpus = crate::gpu::GpuSampler::default();
    let mut prev: HashMap<CameraId, (Instant, u64, u64)> = HashMap::new();
    let mut ticker = tokio::time::interval(SAMPLE_PERIOD);
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let cpu_now = CpuTimes::read();
        let proc_now = ProcSample::read();
        let memory = MemInfo::read();
        let group_now = GroupSample::read();
        let gpus_now = gpus.sample();
        let mut rates = HashMap::new();
        let now = Instant::now();
        for c in &state.cameras() {
            let Some(s) = &c.stream else { continue };
            let m = s.metrics.snapshot();
            if let Some((t, frames, bytes)) = prev.get(&c.id) {
                let secs = now.duration_since(*t).as_secs_f64().max(0.001);
                let fps = (m.frames.saturating_sub(*frames)) as f64 / secs;
                let kbps = (m.bytes.saturating_sub(*bytes)) as f64 * 8.0 / secs / 1000.0;
                rates.insert(c.id.clone(), (fps as f32, kbps as u32));
            }
            prev.insert(c.id.clone(), (now, m.frames, m.bytes));
        }
        let mut live = state.live();
        live.cpu_percent = match (cpu_now, cpu) {
            (Some(n), Some(p)) => n.percent_since(&p).map(|v| v as f32),
            _ => None,
        };
        if let Some(v) = live.cpu_percent {
            live.cpu_history.push_back(v);
            if live.cpu_history.len() > CPU_HISTORY {
                live.cpu_history.pop_front();
            }
        }
        live.process_cpu_percent = match (proc_now, proc) {
            (Some(n), Some(p)) => Some(n.cpu_percent_since(&p) as f32),
            _ => None,
        };
        live.process_rss_bytes = proc_now.map(|p| p.rss_bytes);
        live.service_cpu_percent = match (group_now, group) {
            (Some(n), Some(p)) => n.cpu_percent_since(&p).map(|v| v as f32),
            _ => None,
        };
        live.service = group_now;
        live.memory = memory;
        live.rates = rates;
        live.gpus = gpus_now;
        drop(live);
        cpu = cpu_now;
        proc = proc_now;
        group = group_now;
    }
}

/// Identity of the running executable file, to notice when it is replaced
/// on disk (an update installed while the server runs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExeIdentity {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    inode: u64,
}

impl ExeIdentity {
    /// Identity of the file at `path` now.
    fn of(path: PathBuf) -> Option<Self> {
        let m = std::fs::metadata(&path).ok()?;
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(&m);
        #[cfg(not(unix))]
        let inode = 0;
        Some(Self {
            modified: m.modified().ok(),
            len: m.len(),
            inode,
            path,
        })
    }

    /// The running executable, at startup.
    pub(crate) fn current() -> Option<Self> {
        Self::of(std::env::current_exe().ok()?)
    }

    /// Whether the file at the same path is no longer this one.
    pub(crate) fn replaced(&self) -> bool {
        Self::of(self.path.clone()).is_some_and(|now| now != *self)
    }
}

#[cfg(test)]
mod tests {
    use super::ExeIdentity;

    #[test]
    fn notices_a_replaced_executable() {
        let dir = tempfile::tempdir().expect("tmp");
        let exe = dir.path().join("rustnvr");
        std::fs::write(&exe, b"version 1").expect("write");
        let started = ExeIdentity::of(exe.clone()).expect("identity");
        assert!(!started.replaced());
        // Installed like `install` does: a new file renamed over the old one.
        let new = dir.path().join("rustnvr.new");
        std::fs::write(&new, b"version 2, longer").expect("write");
        std::fs::rename(&new, &exe).expect("rename");
        assert!(started.replaced());
        // A missing file is not reported (nothing to restart into).
        std::fs::remove_file(&exe).expect("remove");
        assert!(!started.replaced());
    }
}
