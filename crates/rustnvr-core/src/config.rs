//! TOML configuration, validated as a whole at startup.
//!
//! Durations are plain integers whose unit is part of the field name
//! (`_seconds`, `_millis`) so that the file is unambiguous without a
//! duration-parsing dependency.

use crate::camera::CameraId;
use crate::rtsp::{RtspTarget, RtspTargetError};
use crate::secret::Secret;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Root of the configuration file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where data is stored.
    #[serde(default)]
    pub storage: StorageConfig,
    /// Recording settings shared by all cameras.
    #[serde(default)]
    pub recording: RecordingConfig,
    /// Reconnection policy shared by all streams.
    #[serde(default)]
    pub reconnect: ReconnectConfig,
    /// Periodic metrics reporting.
    #[serde(default)]
    pub metrics: MetricsConfig,
    /// Web interface and HTTP API.
    #[serde(default)]
    pub web: WebConfig,
    /// Live view options.
    #[serde(default)]
    pub live: LiveConfig,
    /// Motion detection (shared settings; each camera is configured in the web interface).
    #[serde(default)]
    pub motion: MotionConfig,
    /// MQTT client and Home Assistant discovery.
    #[serde(default)]
    pub mqtt: MqttConfig,
    /// Optional object recognition on event pictures (disabled by default).
    #[serde(default)]
    pub ai: AiConfig,
    /// Check for new versions on a Git forge (disabled by default).
    #[serde(default)]
    pub updates: UpdatesConfig,
    /// Addresses called on detection events (`[[webhooks]]`).
    #[serde(default)]
    pub webhooks: Vec<WebhookConfig>,
    /// Cameras, declared as `[[cameras]]` tables (plus those added from the
    /// web interface, see [`MANAGED_CAMERAS_FILE`]).
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
    /// File holding the cameras managed from the web interface, next to the
    /// configuration file. `None` when the configuration did not come from a file.
    #[serde(skip)]
    pub managed_cameras_file: Option<PathBuf>,
    /// File holding the storage settings changed from the web interface.
    #[serde(skip)]
    pub managed_storage_file: Option<PathBuf>,
    /// File holding the MQTT settings changed from the web interface.
    #[serde(skip)]
    pub managed_mqtt_file: Option<PathBuf>,
    /// File holding the AI settings changed from the web interface.
    #[serde(skip)]
    pub managed_ai_file: Option<PathBuf>,
    /// File holding the update settings changed from the web interface.
    #[serde(skip)]
    pub managed_updates_file: Option<PathBuf>,
    /// File holding the webhooks changed from the web interface.
    #[serde(skip)]
    pub managed_webhooks_file: Option<PathBuf>,
    /// File holding the general settings changed from the web interface.
    #[serde(skip)]
    pub managed_settings_file: Option<PathBuf>,
    /// The configuration file itself (`None` when it did not come from a file).
    #[serde(skip)]
    pub config_file: Option<PathBuf>,
}

/// Name of the file, next to the configuration file, holding the general
/// settings changed in the web interface; its sections replace those of the
/// configuration file.
pub const MANAGED_SETTINGS_FILE: &str = "settings.ui.toml";

/// Contents of [`MANAGED_SETTINGS_FILE`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSettings {
    /// Replaces `[recording]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingConfig>,
    /// Replaces `[motion]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion: Option<MotionConfig>,
    /// Replaces `live.transcode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_transcode: Option<bool>,
    /// Replaces `live.max_bitrate_kbps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_max_bitrate_kbps: Option<u32>,
    /// Replaces `live.hwaccel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_hwaccel: Option<HwAccelMode>,
    /// Replaces `storage.low_space_percent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_space_percent: Option<u8>,
    /// Replaces `storage.default_destination`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_destination: Option<String>,
}

impl ManagedSettings {
    /// Applies these settings over a configuration.
    pub fn apply(&self, c: &mut Config) {
        if let Some(r) = &self.recording {
            c.recording = r.clone();
        }
        if let Some(m) = &self.motion {
            c.motion = m.clone();
        }
        if let Some(t) = self.live_transcode {
            c.live.transcode = t;
        }
        if let Some(b) = self.live_max_bitrate_kbps {
            c.live.max_bitrate_kbps = b;
        }
        if let Some(h) = self.live_hwaccel {
            c.live.hwaccel = h;
        }
        if let Some(p) = self.low_space_percent {
            c.storage.low_space_percent = p;
        }
        if let Some(d) = &self.default_destination {
            c.storage.default_destination = d.clone();
        }
    }
}

/// Name of the file, next to the configuration file, where cameras added from
/// the web interface are stored. It contains camera passwords: RustNVR writes
/// it readable by its owner only.
pub const MANAGED_CAMERAS_FILE: &str = "cameras.ui.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedCamerasFile {
    #[serde(default)]
    cameras: Vec<CameraConfig>,
}

/// Storage locations.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Root directory of recordings on the local disk (destination
    /// [`LOCAL_DESTINATION`]). Created if missing.
    #[serde(default = "default_recordings_dir")]
    pub recordings_dir: PathBuf,
    /// Local recordings older than this many days are deleted.
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// The oldest local recordings are deleted beyond this size (GB = 10^9 bytes).
    #[serde(default)]
    pub max_size_gb: Option<u32>,
    /// Free space kept on the local disk (GB), deleting the oldest recordings if needed.
    #[serde(default)]
    pub min_free_gb: Option<u32>,
    /// A destination is reported short of space above this usage (percent).
    #[serde(default = "default_low_space_percent")]
    pub low_space_percent: u8,
    /// Destination used when recording is enabled on a camera without
    /// choosing one (`local` by default). An unknown id falls back to `local`.
    #[serde(default = "default_destination_id")]
    pub default_destination: String,
    /// Other places where cameras can record, typically a NAS share mounted
    /// in Linux (SMB/CIFS or NFS), declared as `[[storage.destinations]]`
    /// (plus those added from the web interface, see [`MANAGED_STORAGE_FILE`]).
    #[serde(default)]
    pub destinations: Vec<DestinationConfig>,
    /// Database and event thumbnails, on the local disk. Default:
    /// `<recordings_dir>/.rustnvr`.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            recordings_dir: default_recordings_dir(),
            retention_days: None,
            max_size_gb: None,
            min_free_gb: None,
            low_space_percent: default_low_space_percent(),
            default_destination: default_destination_id(),
            destinations: Vec::new(),
            data_dir: None,
        }
    }
}

fn default_low_space_percent() -> u8 {
    90
}

fn default_destination_id() -> String {
    "local".to_owned()
}

fn default_recordings_dir() -> PathBuf {
    PathBuf::from("recordings")
}

impl StorageConfig {
    /// Directory of the database and event thumbnails.
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .unwrap_or_else(|| self.recordings_dir.join(".rustnvr"))
    }

    /// Retention of the local recordings.
    pub fn local_retention(&self) -> RetentionConfig {
        RetentionConfig {
            retention_days: self.retention_days,
            max_size_gb: self.max_size_gb,
            min_free_gb: self.min_free_gb,
        }
    }
}

/// How long recordings are kept on a destination. Without any limit,
/// recordings are never deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionConfig {
    /// Delete segments older than this many days.
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// Delete the oldest segments when recordings exceed this size (GB = 10^9 bytes).
    #[serde(default)]
    pub max_size_gb: Option<u32>,
    /// Keep at least this much free space (GB) on the filesystem, deleting
    /// the oldest segments if needed (checked every minute). Destinations only.
    #[serde(default)]
    pub min_free_gb: Option<u32>,
}

impl RetentionConfig {
    /// Invalid values, prefixed with `what`.
    pub fn problems(&self, what: &str) -> Vec<String> {
        let mut p = Vec::new();
        if self.retention_days.is_some_and(|d| d == 0 || d > 3650) {
            p.push(format!("{what}: retention_days must be between 1 and 3650"));
        }
        if self.max_size_gb == Some(0) {
            p.push(format!("{what}: max_size_gb must be at least 1"));
        }
        if self.min_free_gb == Some(0) {
            p.push(format!("{what}: min_free_gb must be at least 1"));
        }
        p
    }
}

/// Name of the file, next to the configuration file, where storage settings
/// changed from the web interface are kept (destinations and retention).
pub const MANAGED_STORAGE_FILE: &str = "storage.ui.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedStorageFile {
    /// Retention of the local disk, overriding `[storage]` of the configuration file.
    #[serde(default)]
    local: Option<RetentionConfig>,
    #[serde(default)]
    destinations: Vec<DestinationConfig>,
}

/// Identifier of the local recordings directory as a destination.
pub const LOCAL_DESTINATION: &str = "local";

/// A recording destination other than the local disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationConfig {
    /// Identifier used by cameras (`record.destinations`), `[a-z0-9_-]`.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Directory on the mounted share (e.g. `/mnt/nas/rustnvr`). Never created
    /// by RustNVR: it must exist on the mounted filesystem.
    pub path: PathBuf,
    /// Refuse to write unless `path` is on a mounted filesystem other than the
    /// system disk. Protects against writing into the empty mount point of an
    /// unmounted share (which would silently fill the system disk).
    #[serde(default = "default_true")]
    pub require_mount: bool,
    /// Recordings older than this many days are deleted from this destination.
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// The oldest recordings are deleted beyond this size (GB = 10^9 bytes).
    #[serde(default)]
    pub max_size_gb: Option<u32>,
    /// Free space kept on the share (GB), deleting the oldest recordings if needed.
    #[serde(default)]
    pub min_free_gb: Option<u32>,
    /// Upload finished segments to S3 or SFTP. `path` is then the local outbox
    /// where they wait to be sent (deleted once sent).
    #[serde(default)]
    pub remote: Option<RemoteConfig>,
    /// True when added from the web interface ([`MANAGED_STORAGE_FILE`]).
    #[serde(skip)]
    pub managed: bool,
}

impl DestinationConfig {
    /// Retention on this destination.
    pub fn retention(&self) -> RetentionConfig {
        RetentionConfig {
            retention_days: self.retention_days,
            max_size_gb: self.max_size_gb,
            min_free_gb: self.min_free_gb,
        }
    }

    /// Problems of this destination alone (uniqueness is checked by the configuration).
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if crate::camera::CameraId::new(self.id.as_str()).is_err() {
            p.push(format!(
                "storage destination {:?}: id must use [a-z0-9_-]",
                self.id
            ));
        }
        if self.id == LOCAL_DESTINATION {
            p.push(format!(
                "storage destination id {LOCAL_DESTINATION:?} is reserved for the local disk"
            ));
        }
        if !self.path.is_absolute() {
            p.push(format!(
                "storage destination {:?}: path must be absolute",
                self.id
            ));
        }
        p.extend(
            self.retention()
                .problems(&format!("storage destination {:?}", self.id)),
        );
        if let Some(r) = &self.remote {
            p.extend(
                r.problems()
                    .into_iter()
                    .map(|m| format!("storage destination {:?}: {m}", self.id)),
            );
        }
        p
    }
}

/// Remote storage of a destination.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RemoteConfig {
    /// S3-compatible object storage.
    S3(S3Config),
    /// SFTP server.
    Sftp(SftpConfig),
}

/// S3-compatible bucket (Amazon S3, MinIO, Backblaze B2, Wasabi…).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct S3Config {
    /// Endpoint URL of an S3-compatible service; absent for Amazon S3.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Region.
    #[serde(default = "default_s3_region")]
    pub region: String,
    /// Bucket.
    pub bucket: String,
    /// Key prefix.
    #[serde(default)]
    pub prefix: String,
    /// Access key identifier.
    pub access_key_id: String,
    /// Secret access key. Prefer `secret_access_key_env`.
    #[serde(default)]
    pub secret_access_key: Option<Secret>,
    /// Environment variable holding the secret access key.
    #[serde(default)]
    pub secret_access_key_env: Option<String>,
}

fn default_s3_region() -> String {
    "us-east-1".to_owned()
}

/// SFTP server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpConfig {
    /// Host name or address.
    pub host: String,
    /// SSH port.
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// User name.
    pub username: String,
    /// Password. Prefer `password_env`, or a key.
    #[serde(default)]
    pub password: Option<Secret>,
    /// Environment variable holding the password.
    #[serde(default)]
    pub password_env: Option<String>,
    /// Private key file (OpenSSH format, not passphrase-protected).
    #[serde(default)]
    pub private_key: Option<PathBuf>,
    /// Remote directory (created if needed).
    #[serde(default)]
    pub remote_dir: String,
    /// Expected server key fingerprint (`SHA256:…`); without it, the key
    /// of the first connection is kept and required afterwards.
    #[serde(default)]
    pub host_key: Option<String>,
}

fn default_ssh_port() -> u16 {
    22
}

impl RemoteConfig {
    /// Short name: `s3` or `sftp`.
    pub fn kind(&self) -> &'static str {
        match self {
            RemoteConfig::S3(_) => "s3",
            RemoteConfig::Sftp(_) => "sftp",
        }
    }

    fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        match self {
            RemoteConfig::S3(s) => {
                if s.bucket.trim().is_empty() {
                    p.push("S3 bucket is required".to_owned());
                }
                if s.access_key_id.trim().is_empty() {
                    p.push("S3 access_key_id is required".to_owned());
                }
                if s.secret_access_key.is_some() == s.secret_access_key_env.is_some() {
                    p.push(
                        "set exactly one of S3 secret_access_key and secret_access_key_env"
                            .to_owned(),
                    );
                }
                if s.endpoint
                    .as_ref()
                    .is_some_and(|e| !(e.starts_with("https://") || e.starts_with("http://")))
                {
                    p.push("S3 endpoint must start with https:// (or http://)".to_owned());
                }
            }
            RemoteConfig::Sftp(s) => {
                if s.host.trim().is_empty() || s.username.trim().is_empty() {
                    p.push("SFTP host and username are required".to_owned());
                }
                let methods = usize::from(s.password.is_some())
                    + usize::from(s.password_env.is_some())
                    + usize::from(s.private_key.is_some());
                if methods != 1 {
                    p.push(
                        "set exactly one of SFTP password, password_env and private_key".to_owned(),
                    );
                }
                if s.host_key
                    .as_ref()
                    .is_some_and(|k| !k.starts_with("SHA256:"))
                {
                    p.push("SFTP host_key must be a SHA256:… fingerprint".to_owned());
                }
            }
        }
        p
    }
}

/// Recording settings.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct RecordingConfig {
    /// Target segment (file) duration. Segments are cut on the first key
    /// frame after this duration, so actual durations depend on the GOP length.
    pub segment_seconds: u32,
    /// Target fMP4 fragment duration: the maximum media lost on a crash.
    pub fragment_millis: u32,
    /// Capacity, in frames, of the queue between ingestion and the recorder.
    /// When full, frames are dropped for the recorder only and the current
    /// segment is closed.
    pub queue_frames: usize,
    /// A timestamp jump larger than this starts a new segment.
    pub max_timestamp_jump_seconds: u32,
    /// Delay before retrying after a write error (e.g. disk full). Doubles on
    /// consecutive failures, up to 16 times this value.
    pub retry_after_error_seconds: u32,
    /// Motion-triggered recording: video kept before each event (from the
    /// previous key frame, so up to one group of pictures more).
    pub pre_roll_seconds: u32,
    /// Motion-triggered recording: video kept after each event.
    pub post_roll_seconds: u32,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            segment_seconds: 60,
            fragment_millis: 1000,
            queue_frames: 256,
            max_timestamp_jump_seconds: 10,
            retry_after_error_seconds: 5,
            pre_roll_seconds: 5,
            post_roll_seconds: 5,
        }
    }
}

impl RecordingConfig {
    /// Every invalid value (empty when valid).
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        let mut check = |ok: bool, msg: &str| {
            if !ok {
                p.push(msg.to_owned());
            }
        };
        check(
            (5..=3600).contains(&self.segment_seconds),
            "recording.segment_seconds must be between 5 and 3600",
        );
        check(
            (100..=10_000).contains(&self.fragment_millis),
            "recording.fragment_millis must be between 100 and 10000",
        );
        check(
            u64::from(self.fragment_millis) < u64::from(self.segment_seconds) * 1000,
            "recording.fragment_millis must be shorter than recording.segment_seconds",
        );
        check(
            (8..=8192).contains(&self.queue_frames),
            "recording.queue_frames must be between 8 and 8192",
        );
        check(
            (1..=300).contains(&self.max_timestamp_jump_seconds),
            "recording.max_timestamp_jump_seconds must be between 1 and 300",
        );
        check(
            (1..=600).contains(&self.retry_after_error_seconds),
            "recording.retry_after_error_seconds must be between 1 and 600",
        );
        check(
            self.pre_roll_seconds <= 60,
            "recording.pre_roll_seconds must be at most 60",
        );
        check(
            self.post_roll_seconds <= 600,
            "recording.post_roll_seconds must be at most 600",
        );
        p
    }

    /// Segment duration.
    pub fn segment_duration(&self) -> Duration {
        Duration::from_secs(self.segment_seconds.into())
    }
    /// Fragment duration.
    pub fn fragment_duration(&self) -> Duration {
        Duration::from_millis(self.fragment_millis.into())
    }
    /// Maximum timestamp jump.
    pub fn max_timestamp_jump(&self) -> Duration {
        Duration::from_secs(self.max_timestamp_jump_seconds.into())
    }
    /// Video kept before motion events.
    pub fn pre_roll(&self) -> Duration {
        Duration::from_secs(self.pre_roll_seconds.into())
    }
    /// Video kept after motion events.
    pub fn post_roll(&self) -> Duration {
        Duration::from_secs(self.post_roll_seconds.into())
    }
    /// Initial retry delay after a write error.
    pub fn retry_after_error(&self) -> Duration {
        Duration::from_secs(self.retry_after_error_seconds.into())
    }
}

/// Exponential backoff between connection attempts.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReconnectConfig {
    /// First retry delay.
    pub initial_millis: u32,
    /// Maximum retry delay.
    pub max_seconds: u32,
    /// A session that lasted at least this long resets the backoff.
    pub stable_after_seconds: u32,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_millis: 1000,
            max_seconds: 30,
            stable_after_seconds: 30,
        }
    }
}

/// Periodic metrics reporting in the logs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MetricsConfig {
    /// Interval between metrics log lines; 0 disables them.
    pub log_interval_seconds: u32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            log_interval_seconds: 60,
        }
    }
}

/// Live view in the browser.
///
/// The live view forwards the camera stream unchanged when the browser can
/// decode it. Otherwise (typically H.265 in Firefox) the browser may ask for an
/// H.264 conversion made by FFmpeg: it runs only while someone watches, once
/// per camera stream whatever the number of viewers, and never touches
/// recording.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct LiveConfig {
    /// Allow on-demand H.264 conversion for browsers that cannot decode the camera codec.
    pub transcode: bool,
    /// FFmpeg executable used for conversion.
    pub ffmpeg: String,
    /// Bit rate cap of the converted stream.
    pub max_bitrate_kbps: u32,
    /// H.264 conversions (live, playback, exports) on the graphics card.
    pub hwaccel: HwAccelMode,
}

/// Use of the graphics card for the H.264 conversions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HwAccelMode {
    /// The card (VAAPI: Intel Quick Sync, AMD) when one passes a test
    /// conversion at startup; the processor otherwise.
    #[default]
    Auto,
    /// Always the processor.
    Off,
}

impl LiveConfig {
    /// Every invalid value (empty when valid).
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if !(100..=20_000).contains(&self.max_bitrate_kbps) {
            p.push("live.max_bitrate_kbps must be between 100 and 20000".to_owned());
        }
        if self.ffmpeg.trim().is_empty() {
            p.push("live.ffmpeg must not be empty".to_owned());
        }
        p
    }
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            transcode: true,
            ffmpeg: "ffmpeg".to_owned(),
            max_bitrate_kbps: 2000,
            hwaccel: HwAccelMode::Auto,
        }
    }
}

/// Motion detection, shared by all cameras.
///
/// Detection is enabled per camera in the web interface (it is off by
/// default). FFmpeg (`live.ffmpeg`) decodes the camera's sub stream (or main
/// stream without one) into a small grey picture analysed a few times per
/// second; recording is never touched.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct MotionConfig {
    /// Pictures analysed per second.
    pub fps: u32,
    /// Width of the analysed picture in pixels (the height keeps the aspect ratio).
    pub width: u32,
    /// An event ends after this many seconds without motion.
    pub post_seconds: u32,
    /// Longer events are split.
    pub max_event_seconds: u32,
    /// Events (and their thumbnails) older than this are deleted.
    pub events_retention_days: u32,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            fps: 5,
            width: 320,
            post_seconds: 10,
            max_event_seconds: 600,
            events_retention_days: 30,
        }
    }
}

impl MotionConfig {
    /// Every invalid value (empty when valid).
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if !(1..=15).contains(&self.fps) {
            p.push("motion.fps must be between 1 and 15".to_owned());
        }
        if !(160..=640).contains(&self.width) {
            p.push("motion.width must be between 160 and 640".to_owned());
        }
        if !(1..=300).contains(&self.post_seconds) {
            p.push("motion.post_seconds must be between 1 and 300".to_owned());
        }
        if !(30..=3600).contains(&self.max_event_seconds) {
            p.push("motion.max_event_seconds must be between 30 and 3600".to_owned());
        }
        if !(1..=3650).contains(&self.events_retention_days) {
            p.push("motion.events_retention_days must be between 1 and 3650".to_owned());
        }
        p
    }
}

/// Name of the file, next to the configuration file, holding the MQTT
/// settings entered in the web interface. When it exists it replaces `[mqtt]`
/// of the configuration file. It may contain the broker password: RustNVR
/// writes it readable by its owner only.
pub const MANAGED_MQTT_FILE: &str = "mqtt.ui.toml";

/// Name of the file, next to the configuration file, holding the AI settings
/// entered in the web interface; it replaces `[ai]` of the configuration.
pub const MANAGED_AI_FILE: &str = "ai.ui.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedAiFile {
    ai: AiConfig,
}

/// Name of the file, next to the configuration file, holding the update
/// settings entered in the web interface; it replaces `[updates]`.
pub const MANAGED_UPDATES_FILE: &str = "updates.ui.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedUpdatesFile {
    updates: UpdatesConfig,
}

/// Name of the file, next to the configuration file, holding the webhooks
/// entered in the web interface; it replaces `[[webhooks]]`. Readable by its
/// owner only (addresses often carry a token).
pub const MANAGED_WEBHOOKS_FILE: &str = "webhooks.ui.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedWebhooksFile {
    #[serde(default)]
    webhooks: Vec<WebhookConfig>,
}

/// What a webhook is called for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    /// Motion detected: sent once the picture of the event is taken.
    Motion,
    /// Objects recognized (optional AI), with the boxes drawn.
    Objects,
    /// The event ended.
    End,
    /// A crossing line was crossed (with the object seen at the line, for
    /// lines limited to objects).
    Crossing,
    /// A recording destination became unusable (share not mounted, not
    /// writable…) or usable again. Not limited by cameras nor objects.
    Storage,
    /// A camera has sent no picture for a minute (offline), or is back
    /// after that. Limited by cameras, not by objects.
    Camera,
}

fn default_webhook_events() -> Vec<WebhookEvent> {
    vec![WebhookEvent::Motion, WebhookEvent::Objects]
}

/// An address called (HTTP POST, JSON) on detection events.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// Name shown in the interface.
    pub name: String,
    /// `http://` or `https://` address (may carry a token: never logged).
    pub url: String,
    /// Calls are made.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Cameras concerned (identifiers); empty: all.
    #[serde(default)]
    pub cameras: Vec<String>,
    /// Only events with one of these objects (labels); empty: every event.
    /// With objects, « motion » calls are skipped, and crossings count only
    /// when one of these objects was seen at the line.
    #[serde(default)]
    pub objects: Vec<String>,
    /// What the webhook is called for.
    #[serde(default = "default_webhook_events")]
    pub events: Vec<WebhookEvent>,
    /// The JPEG picture is sent too (base64 in the JSON).
    #[serde(default = "default_true")]
    pub image: bool,
}

impl WebhookConfig {
    /// Configuration errors.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let what = format!("webhook {:?}", self.name);
        if self.name.trim().is_empty() {
            out.push("webhook : nom manquant".to_owned());
        }
        if !(self.url.starts_with("http://") || self.url.starts_with("https://"))
            || self.url.len() < 10
        {
            out.push(format!("{what} : adresse http:// ou https:// attendue"));
        }
        if self.events.is_empty() {
            out.push(format!("{what} : choisissez au moins un type d'appel"));
        }
        out
    }

    /// Host of the address, for logs (never the path nor the query).
    pub fn host(&self) -> &str {
        let rest = self.url.split_once("://").map_or(&*self.url, |(_, r)| r);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        authority.rsplit('@').next().unwrap_or_default()
    }

    /// The address is a Discord webhook: calls are sent as Discord messages
    /// (an embed, the picture attached) instead of RustNVR's JSON, which
    /// Discord refuses.
    pub fn is_discord(&self) -> bool {
        let host = self.host().to_ascii_lowercase();
        let host = host.split(':').next().unwrap_or_default();
        let path = self
            .url
            .split_once("://")
            .map_or("", |(_, r)| r.find('/').map_or("", |i| &r[i..]));
        matches!(
            host,
            "discord.com" | "discordapp.com" | "ptb.discord.com" | "canary.discord.com"
        ) && path.starts_with("/api/webhooks/")
    }
}

/// Check for new versions (`[updates]`): the latest release published on the
/// repository is compared with the running version. Nothing is installed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct UpdatesConfig {
    /// Web address of the repository (Gitea, Forgejo or GitHub), e.g.
    /// `https://gitea.example.org/owner/RustNVR`. None: no check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Environment variable holding an access token, for a private repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
}

impl UpdatesConfig {
    /// Configuration errors.
    pub fn problems(&self) -> Vec<String> {
        match &self.repository {
            Some(r) => latest_release_api(r)
                .err()
                .map(|e| format!("updates.repository: {e}"))
                .into_iter()
                .collect(),
            None => Vec::new(),
        }
    }
}

/// API address of the latest release of the repository at `repository`
/// (`https://host[/prefix]/owner/name`): GitHub's API for github.com, the
/// Gitea / Forgejo API (`/api/v1`) otherwise.
pub fn latest_release_api(repository: &str) -> Result<String, String> {
    let r = repository.trim().trim_end_matches('/');
    let (scheme, rest) = r
        .split_once("://")
        .filter(|(s, _)| matches!(*s, "http" | "https"))
        .ok_or("adresse http:// ou https:// attendue")?;
    let (host, path) = rest
        .split_once('/')
        .ok_or("propriétaire et dépôt manquants")?;
    if host.is_empty() || host.contains('@') {
        return Err(
            "adresse sans identifiant ni mot de passe attendue (token_env pour un jeton)".into(),
        );
    }
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let [prefix @ .., owner, name] = segments.as_slice() else {
        return Err("propriétaire et dépôt manquants (…/propriétaire/dépôt)".into());
    };
    let name = name.trim_end_matches(".git");
    if [*owner, name]
        .iter()
        .any(|s| s.is_empty() || s.contains(['?', '#', ' ']))
    {
        return Err("propriétaire ou dépôt invalide".into());
    }
    Ok(if host.eq_ignore_ascii_case("github.com") {
        format!("https://api.github.com/repos/{owner}/{name}/releases/latest")
    } else {
        let prefix: String = prefix.iter().map(|p| format!("/{p}")).collect();
        format!("{scheme}://{host}{prefix}/api/v1/repos/{owner}/{name}/releases/latest")
    })
}

/// Whether version `candidate` (`v0.2.0`, `0.2.0-rc1`…) is newer than
/// `current`. Unreadable versions are never newer.
pub fn is_newer_version(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<([u64; 3], bool)> {
        let v = v.trim().trim_start_matches(['v', 'V']);
        let (numbers, pre) = match v.split_once(['-', '+']) {
            Some((n, rest)) => (n, v.as_bytes()[n.len()] == b'-' && !rest.is_empty()),
            None => (v, false),
        };
        let mut parts = numbers.split('.').map(|p| p.parse::<u64>());
        let triple = [
            parts.next()?.ok()?,
            parts.next().unwrap_or(Ok(0)).ok()?,
            parts.next().unwrap_or(Ok(0)).ok()?,
        ];
        parts.next().is_none().then_some((triple, pre))
    }
    match (parse(candidate), parse(current)) {
        // Same numbers: a final version is newer than its pre-releases.
        (Some((a, a_pre)), Some((b, b_pre))) => a > b || (a == b && b_pre && !a_pre),
        _ => false,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedMqttFile {
    mqtt: MqttConfig,
}

/// MQTT client, with Home Assistant MQTT discovery.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MqttConfig {
    /// Connect to the broker.
    pub enabled: bool,
    /// Broker host name or address.
    pub host: String,
    /// Broker port (1883 without TLS, usually 8883 with TLS).
    pub port: u16,
    /// Encrypt the connection (TLS). The broker certificate is checked
    /// against `ca_file`, or the system certificate authorities.
    pub tls: bool,
    /// PEM file of the certificate authority of the broker (self-signed setups).
    pub ca_file: Option<PathBuf>,
    /// User name, if the broker requires authentication.
    pub username: Option<String>,
    /// Password. Prefer `password_env`.
    pub password: Option<Secret>,
    /// Environment variable holding the password.
    pub password_env: Option<String>,
    /// MQTT client identifier (unique per broker).
    pub client_id: String,
    /// Root of RustNVR topics, e.g. `rustnvr/<camera>/motion`.
    pub topic_prefix: String,
    /// Publish Home Assistant discovery messages.
    pub discovery: bool,
    /// Home Assistant discovery prefix.
    pub discovery_prefix: String,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "localhost".to_owned(),
            port: 1883,
            tls: false,
            ca_file: None,
            username: None,
            password: None,
            password_env: None,
            client_id: "rustnvr".to_owned(),
            topic_prefix: "rustnvr".to_owned(),
            discovery: true,
            discovery_prefix: "homeassistant".to_owned(),
        }
    }
}

/// True for a topic prefix usable as the start of a topic name.
fn valid_topic_prefix(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 128
        && !p.starts_with('/')
        && !p.ends_with('/')
        && !p.contains(['#', '+', '\0'])
}

impl MqttConfig {
    /// Broker password, from `password` or `password_env`.
    pub fn resolve_password(
        &self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Secret>, String> {
        match (&self.password, &self.password_env) {
            (Some(_), Some(_)) => {
                Err("mqtt.password and mqtt.password_env are mutually exclusive".into())
            }
            (Some(p), None) => Ok(Some(p.clone()).filter(|p| !p.is_empty())),
            (None, Some(var)) => env(var).map(|v| Some(Secret::new(v))).ok_or_else(|| {
                format!("mqtt.password_env: environment variable {var:?} is not set")
            }),
            (None, None) => Ok(None),
        }
    }

    /// Every problem of these settings (none when disabled, except syntax).
    pub fn problems(&self, env: impl Fn(&str) -> Option<String>) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut p = Vec::new();
        if self.host.trim().is_empty() {
            p.push("mqtt.host must not be empty".to_owned());
        }
        if self.port == 0 {
            p.push("mqtt.port must not be 0".to_owned());
        }
        if self.ca_file.is_some() && !self.tls {
            p.push("mqtt.ca_file requires mqtt.tls = true".to_owned());
        }
        if self.client_id.trim().is_empty() || self.client_id.len() > 64 {
            p.push("mqtt.client_id must have 1 to 64 characters".to_owned());
        }
        if !valid_topic_prefix(&self.topic_prefix) {
            p.push(
                "mqtt.topic_prefix must be a topic without wildcards nor leading/trailing '/'"
                    .to_owned(),
            );
        }
        if self.discovery && !valid_topic_prefix(&self.discovery_prefix) {
            p.push(
                "mqtt.discovery_prefix must be a topic without wildcards nor leading/trailing '/'"
                    .to_owned(),
            );
        }
        match self.resolve_password(env) {
            Err(e) => p.push(e),
            Ok(Some(_)) if self.username.as_deref().is_none_or(str::is_empty) => {
                p.push("mqtt: a password requires mqtt.username".to_owned());
            }
            Ok(_) => {}
        }
        p
    }
}

/// Optional object recognition ("AI") on event pictures. Disabled by
/// default: RustNVR ships no model; a provider must be configured.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AiConfig {
    /// Recognition service; `None` (default) disables recognition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AiProviderConfig>,
    /// Objects below this confidence (0..1) are ignored.
    pub min_score: f32,
    /// Labels to keep (e.g. `["person", "car"]`); empty keeps all.
    pub labels: Vec<String>,
    /// Maximum duration of one analysis.
    pub timeout_seconds: u64,
    /// Pictures analysed per event at most: the first one 1 s after the
    /// start, then one every `interval_seconds` while the event lasts and
    /// nothing is recognized.
    pub max_pictures: u32,
    /// Seconds between two analysed pictures of an event.
    pub interval_seconds: u64,
    /// Keep every analysed picture, with what was found, in the default
    /// destination (`.captures`), to replay the recognition on them later.
    /// Deleted with the recordings (`retention_days`).
    pub keep_pictures: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: None,
            min_score: 0.5,
            labels: Vec::new(),
            timeout_seconds: 10,
            max_pictures: 5,
            interval_seconds: 3,
            keep_pictures: false,
        }
    }
}

/// An object recognition service.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AiProviderConfig {
    /// HTTP API of DeepStack / CodeProject.AI Server
    /// (`POST <url>/v1/vision/detection`), run separately.
    DeepstackHttp {
        /// Base URL, e.g. `http://127.0.0.1:32168`.
        url: String,
        /// Environment variable holding an API key, sent as `api_key`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key_env: Option<String>,
    },
    /// Built-in recognition: a YOLOX ONNX model run inside RustNVR (needs
    /// the `onnx` build feature, on by default).
    Onnx {
        /// Path of the model file, e.g. `/var/lib/rustnvr/yolox_nano.onnx`.
        model: PathBuf,
    },
}

impl AiConfig {
    /// Analysis timeout.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    /// Every problem of these settings.
    pub fn problems(&self, env: impl Fn(&str) -> Option<String>) -> Vec<String> {
        let mut p = Vec::new();
        if !(0.0..=1.0).contains(&self.min_score) {
            p.push("ai.min_score must be between 0 and 1".to_owned());
        }
        if !(1..=120).contains(&self.timeout_seconds) {
            p.push("ai.timeout_seconds must be between 1 and 120".to_owned());
        }
        if !(1..=20).contains(&self.max_pictures) {
            p.push("ai.max_pictures must be between 1 and 20".to_owned());
        }
        if !(1..=60).contains(&self.interval_seconds) {
            p.push("ai.interval_seconds must be between 1 and 60".to_owned());
        }
        if self.labels.iter().any(|l| l.trim().is_empty()) {
            p.push("ai.labels must not contain empty labels".to_owned());
        }
        match &self.provider {
            None => {}
            Some(AiProviderConfig::Onnx { model }) => {
                if !model.is_file() {
                    p.push(format!(
                        "ai.provider.model: no model file at {}",
                        model.display()
                    ));
                }
            }
            Some(AiProviderConfig::DeepstackHttp { url, api_key_env }) => {
                match url::Url::parse(url) {
                    Ok(u) if !u.username().is_empty() || u.password().is_some() => p.push(
                        "ai.provider.url must not contain credentials (use api_key_env)".to_owned(),
                    ),
                    Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => {}
                    _ => p.push("ai.provider.url must be an http(s) URL".to_owned()),
                }
                if let Some(var) = api_key_env
                    && env(var).is_none()
                {
                    p.push(format!(
                        "ai.provider.api_key_env: environment variable {var:?} is not set"
                    ));
                }
            }
        }
        p
    }
}

/// Web interface and HTTP API.
///
/// Authentication is HTTP Basic, which only protects credentials on the wire
/// when combined with TLS (a reverse proxy, until built-in TLS exists). For
/// that reason the default address is loopback, and listening on another
/// address without authentication is refused unless explicitly allowed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WebConfig {
    /// Serve the interface and the API.
    pub enabled: bool,
    /// Listening address, e.g. `127.0.0.1:8080` or `0.0.0.0:8080`.
    pub listen: String,
    /// Directory containing the built frontend (`index.html`).
    pub static_dir: PathBuf,
    /// User name for Basic authentication. Without it, access is unauthenticated.
    pub username: Option<String>,
    /// Password. Prefer `password_env`.
    pub password: Option<Secret>,
    /// Environment variable holding the password.
    pub password_env: Option<String>,
    /// Allow listening on a non-loopback address without authentication.
    pub allow_unauthenticated_remote: bool,
    /// HTTPS certificate (PEM, full chain).
    pub tls_cert: Option<PathBuf>,
    /// HTTPS private key (PEM).
    pub tls_key: Option<PathBuf>,
    /// Serve HTTPS with a self-signed certificate generated in the data
    /// directory (browsers warn until it is accepted).
    pub tls_self_signed: bool,
    /// More names or addresses for the self-signed certificate (VPN
    /// address, DNS name…), besides localhost, the host name and this
    /// machine's network address.
    pub tls_names: Vec<String>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "127.0.0.1:8080".to_owned(),
            static_dir: PathBuf::from("web/dist"),
            username: None,
            password: None,
            password_env: None,
            allow_unauthenticated_remote: false,
            tls_cert: None,
            tls_key: None,
            tls_self_signed: false,
            tls_names: Vec::new(),
        }
    }
}

impl WebConfig {
    /// Parsed listening address.
    pub fn listen_addr(&self) -> Result<std::net::SocketAddr, String> {
        self.listen.parse().map_err(|_| {
            format!(
                "web.listen {:?} is not an address like 127.0.0.1:8080",
                self.listen
            )
        })
    }

    /// Credentials for Basic authentication, or `None` if authentication is disabled.
    pub fn resolve_auth(
        &self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<(String, Secret)>, String> {
        let password = match (&self.password, &self.password_env) {
            (Some(_), Some(_)) => {
                return Err("web.password and web.password_env are mutually exclusive".into());
            }
            (Some(p), None) => Some(p.clone()),
            (None, Some(var)) => Some(Secret::new(env(var).ok_or_else(|| {
                format!("web.password_env: environment variable {var:?} is not set")
            })?)),
            (None, None) => None,
        };
        match (&self.username, password) {
            (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Ok(Some((u.clone(), p))),
            (None, None) => Ok(None),
            _ => Err("web.username and a non-empty web password must be set together".into()),
        }
    }

    fn problems(&self, env: impl Fn(&str) -> Option<String>) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut problems = Vec::new();
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            problems.push("web.tls_cert and web.tls_key must be set together".to_owned());
        }
        if self.tls_self_signed && self.tls_cert.is_some() {
            problems.push("web.tls_self_signed and web.tls_cert are mutually exclusive".to_owned());
        }
        if let Some(bad) = self.tls_names.iter().find(|n| {
            n.trim().is_empty()
                || n.contains(['/', ' ', ':']) && n.parse::<std::net::IpAddr>().is_err()
        }) {
            problems.push(format!(
                "web.tls_names: {bad:?} is neither a host name nor an IP address"
            ));
        }
        let auth = self
            .resolve_auth(env)
            .map_err(|e| problems.push(e))
            .ok()
            .flatten();
        match self.listen_addr() {
            Err(e) => problems.push(e),
            Ok(addr)
                if !addr.ip().is_loopback()
                    && auth.is_none()
                    && !self.allow_unauthenticated_remote =>
            {
                problems.push(format!(
                    "web.listen {addr} is reachable from the network but no authentication is \
                     configured: set web.username and web.password_env, or set \
                     web.allow_unauthenticated_remote = true to accept the risk"
                ));
            }
            Ok(_) => {}
        }
        problems
    }
}

/// One camera.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraConfig {
    /// Stable identifier, used in paths and topics.
    pub id: CameraId,
    /// Human-readable name.
    #[serde(default)]
    pub name: Option<String>,
    /// Disabled cameras are validated but not connected.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Main (full resolution) stream, recorded.
    pub main: StreamConfig,
    /// Optional low resolution stream, reserved for detection (not connected yet).
    #[serde(default)]
    pub sub: Option<StreamConfig>,
    /// Recording options for this camera.
    #[serde(default)]
    pub record: CameraRecordConfig,
    /// True for cameras stored in [`MANAGED_CAMERAS_FILE`] (editable from the
    /// web interface); false for cameras declared in the configuration file.
    #[serde(skip)]
    pub managed: bool,
}

impl CameraConfig {
    /// Problems of this camera alone (identifier uniqueness is checked by
    /// [`Config::validate_with`]). Environment variables are only required
    /// for enabled cameras, which are the ones that connect.
    pub fn problems(&self, env: impl Fn(&str) -> Option<String>) -> Vec<String> {
        let id = self.id.as_str();
        let mut problems = Vec::new();
        if self.name.as_ref().is_some_and(|n| n.chars().count() > 128) {
            problems.push(format!("camera {id:?}: name longer than 128 characters"));
        }
        let streams =
            std::iter::once(("main", &self.main)).chain(self.sub.as_ref().map(|s| ("sub", s)));
        for (role, s) in streams {
            let prefix = format!("camera {id:?} {role} stream");
            if !(1..=120).contains(&s.connect_timeout_seconds) {
                problems.push(format!(
                    "{prefix}: connect_timeout_seconds must be between 1 and 120"
                ));
            }
            if !(1..=300).contains(&s.stall_timeout_seconds) {
                problems.push(format!(
                    "{prefix}: stall_timeout_seconds must be between 1 and 300"
                ));
            }
            let env_lookup = |name: &str| {
                if self.enabled {
                    env(name)
                } else {
                    Some(String::new())
                }
            };
            if let Err(e) = s.resolve_with(env_lookup) {
                problems.push(format!("{prefix}: {e}"));
            }
        }
        problems
    }
}

/// Per-camera recording options.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraRecordConfig {
    /// Recording of the main stream.
    pub enabled: bool,
    /// Record all the time, or only around motion events.
    pub mode: RecordMode,
    /// Where to record: [`LOCAL_DESTINATION`] and/or ids of
    /// `[[storage.destinations]]`. Each destination gets its own copy, written
    /// directly. A camera recording only to remote destinations falls back
    /// to the local disk while they are unavailable.
    pub destinations: Vec<String>,
    /// Recordings of this camera older than this many days are deleted (on
    /// every destination), in addition to the destination's own retention.
    pub retention_days: Option<u32>,
    /// The oldest recordings of this camera are deleted beyond this size
    /// (GB = 10^9 bytes), on each destination.
    pub max_size_gb: Option<u32>,
    /// Record the sound of the camera with the video, when it sends AAC
    /// audio (on by default).
    pub audio: bool,
}

impl Default for CameraRecordConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: RecordMode::Continuous,
            destinations: vec![LOCAL_DESTINATION.to_owned()],
            retention_days: None,
            max_size_gb: None,
            audio: true,
        }
    }
}

impl CameraRecordConfig {
    /// Retention limits of this camera.
    pub fn retention(&self) -> RetentionConfig {
        RetentionConfig {
            retention_days: self.retention_days,
            max_size_gb: self.max_size_gb,
            min_free_gb: None,
        }
    }
}

/// When a camera records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordMode {
    /// All the time.
    #[default]
    Continuous,
    /// Only during motion events, plus `recording.pre_roll_seconds` before
    /// and `recording.post_roll_seconds` after. Needs motion detection
    /// enabled for the camera.
    Motion,
}

impl RecordMode {
    /// Name used in files and the API.
    pub fn as_str(self) -> &'static str {
        match self {
            RecordMode::Continuous => "continuous",
            RecordMode::Motion => "motion",
        }
    }
}

/// RTP transport requested from the camera.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RtspTransport {
    /// RTP interleaved in the RTSP TCP connection (default, most reliable).
    #[default]
    Tcp,
    /// RTP over UDP. Experimental in the RTSP client: no reordering, LAN only.
    Udp,
}

/// One RTSP stream of a camera.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamConfig {
    /// `rtsp://` URL. Credentials may be embedded but dedicated fields are preferred.
    pub url: String,
    /// User name.
    #[serde(default)]
    pub username: Option<String>,
    /// Password. Prefer `password_env` to keep it out of the file.
    #[serde(default)]
    pub password: Option<Secret>,
    /// Name of an environment variable holding the password.
    #[serde(default)]
    pub password_env: Option<String>,
    /// RTP transport.
    #[serde(default)]
    pub transport: RtspTransport,
    /// Timeout of each of the DESCRIBE, SETUP and PLAY steps.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: u32,
    /// The connection is considered dead after this long without a frame.
    #[serde(default = "default_stall_timeout")]
    pub stall_timeout_seconds: u32,
}

impl std::fmt::Debug for StreamConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The raw URL may contain credentials or tokens: never print it.
        f.debug_struct("StreamConfig")
            .field("url", &"<redacted>")
            .field("username", &self.username.as_ref().map(|_| "<set>"))
            .field("password", &self.password)
            .field("password_env", &self.password_env)
            .field("transport", &self.transport)
            .field("connect_timeout_seconds", &self.connect_timeout_seconds)
            .field("stall_timeout_seconds", &self.stall_timeout_seconds)
            .finish()
    }
}

fn default_true() -> bool {
    true
}
fn default_connect_timeout() -> u32 {
    10
}
fn default_stall_timeout() -> u32 {
    10
}

impl StreamConfig {
    /// Connection timeout per RTSP step.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_seconds.into())
    }

    /// Stall timeout.
    pub fn stall_timeout(&self) -> Duration {
        Duration::from_secs(self.stall_timeout_seconds.into())
    }

    /// Builds the RTSP target, reading `password_env` from the process environment.
    pub fn resolve(&self) -> Result<RtspTarget, String> {
        self.resolve_with(|name| std::env::var(name).ok())
    }

    /// Builds the RTSP target, looking environment variables up with `env`.
    pub fn resolve_with(&self, env: impl Fn(&str) -> Option<String>) -> Result<RtspTarget, String> {
        let password = match (&self.password, &self.password_env) {
            (Some(_), Some(_)) => {
                return Err("`password` and `password_env` are mutually exclusive".into());
            }
            (Some(p), None) => Some(p.clone()),
            (None, Some(var)) => match env(var) {
                Some(p) => Some(Secret::new(p)),
                None => return Err(format!("environment variable {var:?} is not set")),
            },
            (None, None) => None,
        };
        RtspTarget::parse(&self.url, self.username.as_deref(), password)
            .map_err(|e: RtspTargetError| e.to_string())
    }
}

/// Configuration loading or validation failure.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read configuration file {path}: {source}")]
    Read {
        /// Path of the file.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The file is not valid TOML or does not match the schema.
    #[error("invalid configuration file (line {line}): {message}")]
    Parse {
        /// 1-based line of the error, 0 if unknown.
        line: usize,
        /// Parser message (without the source excerpt, which could contain a secret).
        message: String,
    },
    /// The file parsed but some values are invalid. All problems are listed.
    #[error("invalid configuration:\n  - {}", .0.join("\n  - "))]
    Invalid(Vec<String>),
}

fn parse_toml<T: serde::de::DeserializeOwned>(
    text: &str,
    file: Option<&str>,
) -> Result<T, ConfigError> {
    toml::from_str(text).map_err(|e| ConfigError::Parse {
        line: e
            .span()
            .map(|s| text[..s.start.min(text.len())].lines().count().max(1))
            .unwrap_or(0),
        message: match file {
            Some(f) => format!("{f}: {}", e.message()),
            None => e.message().to_owned(),
        },
    })
}

impl Config {
    /// Reads, parses and validates a configuration file, together with the
    /// cameras managed from the web interface ([`MANAGED_CAMERAS_FILE`] in the
    /// same directory, if it exists).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::load_with_env(path, |name| std::env::var(name).ok())
    }

    /// [`Self::load`] with a custom environment lookup.
    pub fn load_with_env(
        path: &Path,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let read = |p: &Path| {
            std::fs::read_to_string(p).map_err(|source| ConfigError::Read {
                path: p.to_owned(),
                source,
            })
        };
        let text = read(path)?;
        let mut config: Config = parse_toml(&text, None)?;
        let managed_path = path.with_file_name(MANAGED_CAMERAS_FILE);
        if managed_path.is_file() {
            let managed: ManagedCamerasFile =
                parse_toml(&read(&managed_path)?, Some(MANAGED_CAMERAS_FILE))?;
            config
                .cameras
                .extend(managed.cameras.into_iter().map(|mut c| {
                    c.managed = true;
                    c
                }));
        }
        config.managed_cameras_file = Some(managed_path);
        config.config_file = Some(path.to_owned());
        let storage_path = path.with_file_name(MANAGED_STORAGE_FILE);
        if storage_path.is_file() {
            let managed: ManagedStorageFile =
                parse_toml(&read(&storage_path)?, Some(MANAGED_STORAGE_FILE))?;
            if let Some(local) = managed.local {
                config.storage.retention_days = local.retention_days;
                config.storage.max_size_gb = local.max_size_gb;
                config.storage.min_free_gb = local.min_free_gb;
            }
            config
                .storage
                .destinations
                .extend(managed.destinations.into_iter().map(|mut d| {
                    d.managed = true;
                    d
                }));
        }
        config.managed_storage_file = Some(storage_path);
        let mqtt_path = path.with_file_name(MANAGED_MQTT_FILE);
        if mqtt_path.is_file() {
            let managed: ManagedMqttFile = parse_toml(&read(&mqtt_path)?, Some(MANAGED_MQTT_FILE))?;
            config.mqtt = managed.mqtt;
        }
        config.managed_mqtt_file = Some(mqtt_path);
        let ai_path = path.with_file_name(MANAGED_AI_FILE);
        if ai_path.is_file() {
            let managed: ManagedAiFile = parse_toml(&read(&ai_path)?, Some(MANAGED_AI_FILE))?;
            config.ai = managed.ai;
        }
        config.managed_ai_file = Some(ai_path);
        let updates_path = path.with_file_name(MANAGED_UPDATES_FILE);
        if updates_path.is_file() {
            let managed: ManagedUpdatesFile =
                parse_toml(&read(&updates_path)?, Some(MANAGED_UPDATES_FILE))?;
            config.updates = managed.updates;
        }
        config.managed_updates_file = Some(updates_path);
        let webhooks_path = path.with_file_name(MANAGED_WEBHOOKS_FILE);
        if webhooks_path.is_file() {
            let managed: ManagedWebhooksFile =
                parse_toml(&read(&webhooks_path)?, Some(MANAGED_WEBHOOKS_FILE))?;
            config.webhooks = managed.webhooks;
        }
        config.managed_webhooks_file = Some(webhooks_path);
        let settings_path = path.with_file_name(MANAGED_SETTINGS_FILE);
        if settings_path.is_file() {
            let managed: ManagedSettings =
                parse_toml(&read(&settings_path)?, Some(MANAGED_SETTINGS_FILE))?;
            managed.apply(&mut config);
        }
        config.managed_settings_file = Some(settings_path);
        config.validate_with(env)?;
        Ok(config)
    }

    /// Parses and validates a configuration, resolving environment variables
    /// from the process environment.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Self::from_toml_with_env(text, |name| std::env::var(name).ok())
    }

    /// Parses and validates a configuration with a custom environment lookup.
    pub fn from_toml_with_env(
        text: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let config: Config = parse_toml(text, None)?;
        config.validate_with(env)?;
        Ok(config)
    }

    /// Checks every value, reporting all problems at once.
    pub fn validate_with(&self, env: impl Fn(&str) -> Option<String>) -> Result<(), ConfigError> {
        let mut problems = Vec::new();
        let mut check = |ok: bool, msg: &str| {
            if !ok {
                problems.push(msg.to_owned());
            }
        };

        check(
            !self.storage.recordings_dir.as_os_str().is_empty(),
            "storage.recordings_dir must not be empty",
        );
        for p in self.recording.problems() {
            check(false, &p);
        }
        let b = &self.reconnect;
        check(
            (10..=60_000).contains(&b.initial_millis),
            "reconnect.initial_millis must be between 10 and 60000",
        );
        check(
            (1..=3600).contains(&b.max_seconds)
                && u64::from(b.max_seconds) * 1000 >= u64::from(b.initial_millis),
            "reconnect.max_seconds must be between 1 and 3600 and not below initial_millis",
        );
        check(
            b.stable_after_seconds <= 3600,
            "reconnect.stable_after_seconds must be at most 3600",
        );
        check(
            self.metrics.log_interval_seconds == 0 || self.metrics.log_interval_seconds >= 5,
            "metrics.log_interval_seconds must be 0 (disabled) or at least 5",
        );

        problems.extend(self.storage.local_retention().problems("storage"));
        if !(50..=99).contains(&self.storage.low_space_percent) {
            problems.push("storage.low_space_percent must be between 50 and 99".to_owned());
        }
        let mut dest_ids = HashSet::from([LOCAL_DESTINATION]);
        for d in &self.storage.destinations {
            problems.extend(d.problems());
            if d.id != LOCAL_DESTINATION && !dest_ids.insert(d.id.as_str()) {
                problems.push(format!(
                    "storage destination {:?} is declared more than once",
                    d.id
                ));
            }
        }
        let mut seen = HashSet::new();
        for cam in &self.cameras {
            for d in &cam.record.destinations {
                if !dest_ids.contains(d.as_str()) {
                    problems.push(format!(
                        "camera {:?}: unknown recording destination {d:?} (declare it in [[storage.destinations]])",
                        cam.id.as_str()
                    ));
                }
            }
            if cam.record.enabled && cam.record.destinations.is_empty() {
                problems.push(format!(
                    "camera {:?}: recording enabled without destination",
                    cam.id.as_str()
                ));
            }
            let id = cam.id.as_str();
            if !seen.insert(id) {
                let place = if cam.managed {
                    format!(" (in {MANAGED_CAMERAS_FILE})")
                } else {
                    String::new()
                };
                problems.push(format!("camera {id:?} is declared more than once{place}"));
            }
            problems.extend(cam.problems(&env));
            problems.extend(
                cam.record
                    .retention()
                    .problems(&format!("camera {:?} record", cam.id.as_str())),
            );
        }
        problems.extend(self.web.problems(&env));
        problems.extend(self.live.problems());
        problems.extend(self.motion.problems());
        problems.extend(self.mqtt.problems(&env));
        problems.extend(self.ai.problems(&env));
        problems.extend(self.updates.problems());
        for w in &self.webhooks {
            problems.extend(w.problems());
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Invalid(problems))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn minimal_config_uses_defaults() {
        let c = Config::from_toml_with_env(
            r#"
            [[cameras]]
            id = "garage"
            main = { url = "rtsp://10.0.0.2/stream1" }
            "#,
            no_env,
        )
        .expect("valid");
        assert_eq!(c.recording.segment_seconds, 60);
        assert_eq!(c.cameras.len(), 1);
        assert!(c.cameras[0].enabled);
        assert!(c.cameras[0].record.enabled);
        assert_eq!(c.cameras[0].main.transport, RtspTransport::Tcp);
    }

    #[test]
    fn password_from_env() {
        let text = r#"
            [[cameras]]
            id = "garage"
            main = { url = "rtsp://10.0.0.2/s", username = "admin", password_env = "CAM_PW" }
        "#;
        let c = Config::from_toml_with_env(text, |n| (n == "CAM_PW").then(|| "pw".to_owned()))
            .expect("valid");
        let target = c.cameras[0]
            .main
            .resolve_with(|n| (n == "CAM_PW").then(|| "pw".to_owned()))
            .expect("resolves");
        assert_eq!(
            target.credentials().map(|c| c.password.expose()),
            Some("pw")
        );

        let err = Config::from_toml_with_env(text, no_env).expect_err("missing env");
        assert!(err.to_string().contains("CAM_PW"), "{err}");
    }

    #[test]
    fn reports_all_problems() {
        let err = Config::from_toml_with_env(
            r#"
            [recording]
            segment_seconds = 1
            fragment_millis = 5

            [[cameras]]
            id = "a"
            main = { url = "http://x/" }

            [[cameras]]
            id = "a"
            main = { url = "rtsp://x/", password = "p", password_env = "P" }
            "#,
            no_env,
        )
        .expect_err("invalid");
        let ConfigError::Invalid(problems) = err else {
            panic!("expected Invalid, got {err:?}");
        };
        assert!(problems.len() >= 5, "{problems:#?}");
    }

    #[test]
    fn parse_errors_do_not_echo_source() {
        let err = Config::from_toml_with_env(
            "[[cameras]]\nid = \"a\"\nmain = { url = \"rtsp://u:SECRET@x/\", bogus = 1 }\n",
            no_env,
        )
        .expect_err("unknown field");
        let msg = err.to_string();
        assert!(!msg.contains("SECRET"), "{msg}");
        assert!(msg.contains("line 3"), "{msg}");
    }

    #[test]
    fn debug_hides_url_and_password() {
        let c = Config::from_toml_with_env(
            "[[cameras]]\nid = \"a\"\nmain = { url = \"rtsp://u:SECRET@x/\" }\n",
            no_env,
        )
        .expect("valid");
        assert!(!format!("{c:?}").contains("SECRET"));
    }

    #[test]
    fn disabled_camera_does_not_need_env() {
        Config::from_toml_with_env(
            r#"
            [[cameras]]
            id = "a"
            enabled = false
            main = { url = "rtsp://x/", username = "u", password_env = "MISSING" }
            "#,
            no_env,
        )
        .expect("valid");
    }

    #[test]
    fn web_defaults_to_loopback_without_auth() {
        let c = Config::from_toml_with_env("", no_env).expect("valid");
        assert!(c.web.enabled);
        assert!(c.web.listen_addr().expect("addr").ip().is_loopback());
        assert_eq!(c.web.resolve_auth(no_env), Ok(None));
    }

    #[test]
    fn web_refuses_unauthenticated_remote_listen() {
        let err = Config::from_toml_with_env("[web]\nlisten = \"0.0.0.0:8080\"\n", no_env)
            .expect_err("must refuse");
        assert!(err.to_string().contains("no authentication"), "{err}");
        Config::from_toml_with_env(
            "[web]\nlisten = \"0.0.0.0:8080\"\nallow_unauthenticated_remote = true\n",
            no_env,
        )
        .expect("explicitly allowed");
        let c = Config::from_toml_with_env(
            "[web]\nlisten = \"0.0.0.0:8080\"\nusername = \"admin\"\npassword_env = \"PW\"\n",
            |n| (n == "PW").then(|| "pw".to_owned()),
        )
        .expect("authenticated");
        let auth = c
            .web
            .resolve_auth(|n| (n == "PW").then(|| "pw".to_owned()))
            .expect("auth")
            .expect("enabled");
        assert_eq!((auth.0.as_str(), auth.1.expose()), ("admin", "pw"));
    }

    #[test]
    fn web_rejects_half_configured_auth() {
        let err = Config::from_toml_with_env("[web]\nusername = \"admin\"\n", no_env)
            .expect_err("password missing");
        assert!(err.to_string().contains("together"), "{err}");
    }

    #[test]
    fn loads_cameras_managed_from_the_interface() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            "[[cameras]]
id = \"a\"
main = { url = \"rtsp://x/a\" }
",
        )
        .expect("write");
        let managed = dir.path().join(MANAGED_CAMERAS_FILE);
        std::fs::write(
            &managed,
            "[[cameras]]
id = \"b\"
name = \"B\"
[cameras.main]
url = \"rtsp://x/b\"
username = \"u\"
password = \"p\"
",
        )
        .expect("write");
        let c = Config::load_with_env(&config, no_env).expect("valid");
        assert_eq!(c.cameras.len(), 2);
        assert!(!c.cameras[0].managed);
        assert!(c.cameras[1].managed);
        assert_eq!(c.managed_cameras_file.as_deref(), Some(managed.as_path()));
        let target = c.cameras[1].main.resolve_with(no_env).expect("target");
        assert_eq!(target.credentials().map(|c| c.password.expose()), Some("p"));

        std::fs::write(
            &managed,
            "[[cameras]]
id = \"a\"
main = { url = \"rtsp://x/b\" }
",
        )
        .expect("write");
        let err = Config::load_with_env(&config, no_env).expect_err("duplicate");
        assert!(err.to_string().contains(MANAGED_CAMERAS_FILE), "{err}");
    }

    #[test]
    fn ai_is_disabled_by_default_and_validated() {
        let none = Config::from_toml_with_env("", no_env).expect("empty");
        assert_eq!(none.ai.provider, None);

        let ok = Config::from_toml_with_env(
            "[ai]\nprovider = { type = \"deepstack_http\", url = \"http://127.0.0.1:32168\" }\nlabels = [\"person\"]\n",
            no_env,
        )
        .expect("valid");
        assert!(matches!(
            ok.ai.provider,
            Some(AiProviderConfig::DeepstackHttp { .. })
        ));

        let err = Config::from_toml_with_env(
            "[ai]\nprovider = { type = \"deepstack_http\", url = \"http://u:SECRET@h/\", api_key_env = \"K\" }\nmin_score = 2\n",
            no_env,
        )
        .expect_err("invalid");
        let msg = err.to_string();
        assert!(msg.contains("must not contain credentials"), "{msg}");
        assert!(msg.contains("ai.min_score"), "{msg}");
        assert!(msg.contains("\"K\" is not set"), "{msg}");
        assert!(!msg.contains("SECRET"), "{msg}");
    }

    #[test]
    fn validates_recording_destinations() {
        let ok = Config::from_toml_with_env(
            r#"
            [[storage.destinations]]
            id = "nas"
            name = "NAS"
            path = "/mnt/nas/rustnvr"

            [[cameras]]
            id = "a"
            main = { url = "rtsp://x/" }
            record = { enabled = true, destinations = ["local", "nas"] }

            [[cameras]]
            id = "b"
            main = { url = "rtsp://x/" }
            "#,
            no_env,
        )
        .expect("valid");
        assert_eq!(ok.cameras[0].record.destinations, ["local", "nas"]);
        assert_eq!(ok.cameras[1].record.destinations, ["local"], "default");
        assert!(ok.storage.destinations[0].require_mount, "default");

        let err = Config::from_toml_with_env(
            r#"
            [[storage.destinations]]
            id = "local"
            path = "relative/dir"

            [[cameras]]
            id = "a"
            main = { url = "rtsp://x/" }
            record = { destinations = ["nope"] }
            "#,
            no_env,
        )
        .expect_err("invalid");
        let msg = err.to_string();
        assert!(
            msg.contains("reserved")
                && msg.contains("absolute")
                && msg.contains("unknown recording destination"),
            "{msg}"
        );

        let err = Config::from_toml_with_env(
            "[storage]
retention_days = 0
",
            no_env,
        )
        .expect_err("invalid");
        assert!(err.to_string().contains("retention_days"), "{err}");
        let c = Config::from_toml_with_env(
            "[storage]
retention_days = 30
max_size_gb = 500
",
            no_env,
        )
        .expect("valid");
        assert_eq!(c.storage.local_retention().retention_days, Some(30));
    }
}

#[cfg(test)]
mod updates_tests {
    use super::*;

    #[test]
    fn finds_the_release_api_of_a_repository() {
        assert_eq!(
            latest_release_api("https://gitea.example.org/Guiyomee/RustNVR").as_deref(),
            Ok("https://gitea.example.org/api/v1/repos/Guiyomee/RustNVR/releases/latest")
        );
        // Forge in a sub-path, `.git` and trailing slash.
        assert_eq!(
            latest_release_api("http://nas:3000/git/me/nvr.git/").as_deref(),
            Ok("http://nas:3000/git/api/v1/repos/me/nvr/releases/latest")
        );
        assert_eq!(
            latest_release_api("https://github.com/owner/rustnvr").as_deref(),
            Ok("https://api.github.com/repos/owner/rustnvr/releases/latest")
        );
        for bad in [
            "gitea.example.org/me/nvr",
            "ftp://host/me/nvr",
            "https://host/nvr",
            "https://user:pass@host/me/nvr",
        ] {
            assert!(latest_release_api(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer_version("v0.2.0", "0.1.0"));
        assert!(is_newer_version("0.1.1", "0.1.0"));
        assert!(is_newer_version("1.0", "0.9.9"));
        assert!(!is_newer_version("v0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.0.9", "0.1.0"));
        // A final version is newer than its pre-releases, not the reverse.
        assert!(is_newer_version("0.2.0", "0.2.0-rc1"));
        assert!(!is_newer_version("0.2.0-rc1", "0.2.0"));
        assert!(!is_newer_version("nightly", "0.1.0"));
        // Calendar versions: a release tag with a leading zero is the
        // program's version without it.
        assert!(!is_newer_version("v2026.11.01", "2026.11.1"));
        assert!(is_newer_version("v2026.11.02", "2026.11.1"));
        assert!(is_newer_version("v2026.12.01", "2026.11.1"));
    }
}

#[cfg(test)]
mod webhook_tests {
    use super::*;

    fn hook(url: &str) -> WebhookConfig {
        WebhookConfig {
            name: "n8n".into(),
            url: url.into(),
            enabled: true,
            cameras: Vec::new(),
            objects: Vec::new(),
            events: default_webhook_events(),
            image: true,
        }
    }

    #[test]
    fn checks_webhooks_and_hides_their_secrets() {
        let h = hook("https://user:pw@n8n.local:5678/webhook/abc?token=s3cret");
        assert!(h.problems().is_empty());
        assert_eq!(h.host(), "n8n.local:5678");
        assert_eq!(hook("http://10.0.0.2/x").host(), "10.0.0.2");
        assert!(!hook("ftp://host/x").problems().is_empty());
        let none = WebhookConfig {
            events: Vec::new(),
            ..hook("http://host/x")
        };
        assert!(!none.problems().is_empty());
        // Defaults when read from a file.
        let parsed: ManagedWebhooksFile =
            toml::from_str("[[webhooks]]\nname = \"a\"\nurl = \"http://h/x\"\n").expect("toml");
        assert_eq!(parsed.webhooks[0].events, default_webhook_events());
        assert!(parsed.webhooks[0].enabled && parsed.webhooks[0].image);
    }

    #[test]
    fn recognizes_discord_webhooks() {
        assert!(hook("https://discord.com/api/webhooks/123/abc").is_discord());
        assert!(hook("https://Discord.com:443/api/webhooks/123/abc?wait=true").is_discord());
        assert!(hook("https://discordapp.com/api/webhooks/123/abc").is_discord());
        assert!(hook("https://ptb.discord.com/api/webhooks/123/abc").is_discord());
        assert!(!hook("https://discord.com/channels/123").is_discord());
        assert!(!hook("https://n8n.local/api/webhooks/123").is_discord());
        assert!(!hook("https://discord.com.evil.example/api/webhooks/1/a").is_discord());
    }
}
