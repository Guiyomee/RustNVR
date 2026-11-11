//! Camera and storage management from the API.
//!
//! The API crate does not know how cameras are started or settings stored:
//! the application implements [`NvrControl`] and registers it with
//! [`crate::ApiState::set_control`]. Without it, management routes answer 503.

use rustnvr_core::config::{
    AiConfig, MotionConfig, RecordMode, RecordingConfig, RemoteConfig, RtspTransport,
};
use rustnvr_core::{DetectedObject, MotionSettings, RtspTarget, Secret};
use rustnvr_stream::SessionOptions;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

/// Boxed future returned by [`NvrControl`] methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One RTSP stream as entered in the interface.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamInput {
    /// `rtsp://` URL, without credentials.
    pub url: String,
    /// User name.
    #[serde(default)]
    pub username: Option<String>,
    /// Password. On update, `null`/absent keeps the stored password.
    #[serde(default)]
    pub password: Option<Secret>,
    /// RTP transport.
    #[serde(default)]
    pub transport: RtspTransport,
}

/// A camera as entered in the interface.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraInput {
    /// Identifier (creation only; taken from the URL on update).
    #[serde(default)]
    pub id: Option<String>,
    /// Display name.
    pub name: String,
    /// Connect and record this camera.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Continuous recording of the main stream.
    #[serde(default = "yes")]
    pub record: bool,
    /// Main stream (recorded); absent for a USB camera.
    #[serde(default)]
    pub main: Option<StreamInput>,
    /// A USB webcam instead of RTSP streams.
    #[serde(default)]
    pub usb: Option<rustnvr_core::config::UsbConfig>,
    /// Optional sub stream (live view).
    #[serde(default)]
    pub sub: Option<StreamInput>,
    /// Where to record (`local` and/or destination ids).
    #[serde(default = "local_only")]
    pub record_destinations: Vec<String>,
    /// Continuous or motion-triggered recording.
    #[serde(default)]
    pub record_mode: RecordMode,
    /// Per-camera retention, in days (`null`: only the destination's).
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// Per-camera size limit per destination, in GB.
    #[serde(default)]
    pub max_size_gb: Option<u32>,
    /// Record the sound; absent keeps the current choice (on for a new camera).
    #[serde(default)]
    pub record_audio: Option<bool>,
    /// Port of the ONVIF service, for PTZ (`null`: none).
    #[serde(default)]
    pub onvif_port: Option<u16>,
    /// User name RTSP clients must give for this camera, instead of the
    /// general one of the republication (`null` or empty: the general one).
    #[serde(default)]
    pub restream_username: Option<String>,
    /// Its password; absent keeps the stored one.
    #[serde(default)]
    pub restream_password: Option<Secret>,
}

/// A storage destination as entered in the interface. For `local`, only the
/// retention fields are used.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationInput {
    /// Identifier (creation only).
    #[serde(default)]
    pub id: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Directory on the mounted share.
    #[serde(default)]
    pub path: Option<String>,
    /// Require a mounted filesystem other than the system disk.
    #[serde(default = "yes")]
    pub require_mount: bool,
    /// Retention in days (`null`: no age limit).
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// Maximum size in GB (`null`: no size limit).
    #[serde(default)]
    pub max_size_gb: Option<u32>,
    /// Free space to keep (GB).
    #[serde(default)]
    pub min_free_gb: Option<u32>,
    /// S3 or SFTP upload; secrets left out are kept from the previous settings.
    #[serde(default)]
    pub remote: Option<RemoteConfig>,
}

fn yes() -> bool {
    true
}

fn local_only() -> Vec<String> {
    vec![rustnvr_core::config::LOCAL_DESTINATION.to_owned()]
}

/// Why a management operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// The input is invalid; the message is shown to the user.
    Invalid(String),
    /// No such camera.
    NotFound(String),
    /// The identifier is already used.
    Conflict(String),
    /// The camera is declared in the configuration file and cannot be changed from the API.
    ReadOnly(String),
    /// Storage or startup failure.
    Internal(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::Invalid(m)
            | ControlError::NotFound(m)
            | ControlError::Conflict(m)
            | ControlError::ReadOnly(m)
            | ControlError::Internal(m) => f.write_str(m),
        }
    }
}

/// Implemented by the application to manage cameras and storage at run time.
pub trait NvrControl: Send + Sync {
    /// Adds and starts a camera.
    fn create(&self, input: CameraInput) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Replaces a managed camera's settings and restarts it.
    fn update(&self, id: String, input: CameraInput) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Stops and removes a managed camera. Its recordings stay on disk.
    fn delete(&self, id: String) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Main stream endpoint and options of a camera, to test its connection.
    fn target(&self, id: String) -> BoxFuture<'_, Option<(RtspTarget, SessionOptions)>>;
    /// Adds a storage destination.
    fn create_destination(
        &self,
        input: DestinationInput,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Changes a destination added from the interface, or the retention of `local`.
    fn update_destination(
        &self,
        id: String,
        input: DestinationInput,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Removes a destination that no camera records to. Its files are not deleted.
    fn delete_destination(&self, id: String) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Motion detection settings of a camera (defaults if never set).
    fn motion(&self, id: String) -> BoxFuture<'_, Result<MotionSettings, ControlError>>;
    /// Stores and applies the motion detection settings of a camera
    /// (detection restarts; the camera stream is not interrupted).
    fn set_motion(
        &self,
        id: String,
        settings: MotionSettings,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Switches the recording of the sound of a camera (stored with the
    /// camera). Off applies at once; on starts with the next key frame.
    fn set_audio(&self, id: String, enabled: bool) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Pauses motion detection on every camera, or resumes it (kept across
    /// restarts; the settings of each camera do not change).
    fn set_detection_paused(&self, paused: bool) -> BoxFuture<'_, Result<(), ControlError>>;
    /// MQTT settings (never the password) and connection state.
    fn mqtt(&self) -> BoxFuture<'_, MqttView>;
    /// Stores the MQTT settings and reconnects.
    fn set_mqtt(&self, input: MqttInput) -> BoxFuture<'_, Result<(), ControlError>>;
    /// General settings in effect.
    fn settings(&self) -> BoxFuture<'_, SettingsView>;
    /// Stores and applies general settings (cameras restart if recording
    /// settings changed, detections if detection settings changed).
    fn set_settings(&self, input: SettingsInput) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Object recognition settings and state.
    fn ai(&self) -> BoxFuture<'_, AiView>;
    /// Checks, stores (`ai.ui.toml`) and applies object recognition settings,
    /// without restarting.
    fn set_ai(&self, settings: AiConfig) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Analyses a current picture of `camera` with the running provider.
    fn ai_test(&self, camera: String) -> BoxFuture<'_, Result<AiTestResult, ControlError>>;
    /// Checks a CodeProject.AI / DeepStack server (not stored) with a test
    /// pattern.
    fn ai_probe(&self, input: AiProbeInput) -> BoxFuture<'_, AiProbeResult>;
    /// Downloads a model of the built-in recognition (checked by its SHA-256)
    /// into the data directory, unless it is already there.
    fn ai_download_model(&self, id: String) -> BoxFuture<'_, Result<AiModelView, ControlError>>;
    /// Webhooks in effect.
    fn webhooks(&self) -> BoxFuture<'_, Vec<rustnvr_core::config::WebhookConfig>>;
    /// Checks, stores (`webhooks.ui.toml`) and applies the webhooks.
    fn set_webhooks(
        &self,
        hooks: Vec<rustnvr_core::config::WebhookConfig>,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
    /// Calls `hook` once with a sample event.
    fn test_webhook(
        &self,
        hook: rustnvr_core::config::WebhookConfig,
    ) -> BoxFuture<'_, WebhookTestResult>;
    /// Checks the watched repository for a newer version now.
    fn check_updates(&self) -> BoxFuture<'_, crate::dto::UpdateStatus>;
    /// Turns the check for new versions on or off (`updates.ui.toml`), then
    /// checks.
    fn set_updates(
        &self,
        check: bool,
    ) -> BoxFuture<'_, Result<crate::dto::UpdateStatus, ControlError>>;
    /// Who is inside according to each line counting the presence:
    /// camera, line, count.
    fn presence(&self) -> BoxFuture<'_, Vec<(String, String, u64)>>;
    /// Days with kept pictures (`ai.keep_pictures`), the most recent first.
    fn captures(&self) -> BoxFuture<'_, Result<Vec<CaptureDay>, ControlError>>;
    /// Kept pictures of `camera` on `day`.
    fn capture_pictures(
        &self,
        camera: String,
        day: String,
    ) -> BoxFuture<'_, Result<Vec<CapturePicture>, ControlError>>;
    /// The JPEG of a kept picture.
    fn capture_picture(
        &self,
        camera: String,
        day: String,
        file: String,
    ) -> BoxFuture<'_, Result<Vec<u8>, ControlError>>;
    /// Deletes the kept pictures of `camera` on `day`; how many.
    fn delete_captures(
        &self,
        camera: String,
        day: String,
    ) -> BoxFuture<'_, Result<usize, ControlError>>;
    /// Analyses again the kept pictures of `camera` on `day` (only `file`
    /// when given) with the current recognition settings.
    fn replay(
        &self,
        camera: String,
        day: String,
        file: Option<String>,
    ) -> BoxFuture<'_, Result<ReplayResult, ControlError>>;
    /// Starts the presence count of a line again from zero.
    fn reset_presence(
        &self,
        camera: String,
        line: String,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
    /// A backup of the configuration (files and detection settings),
    /// encrypted with `passphrase`.
    fn backup(&self, passphrase: String) -> BoxFuture<'_, Result<Vec<u8>, ControlError>>;
    /// Opens a backup and tells what restoring it would do.
    fn backup_inspect(
        &self,
        data: Vec<u8>,
        passphrase: String,
    ) -> BoxFuture<'_, Result<BackupPreview, ControlError>>;
    /// Restores the files `files` of a backup, and its detection settings
    /// when `motion`; used from the next start of the server.
    fn backup_restore(
        &self,
        data: Vec<u8>,
        passphrase: String,
        files: Vec<String>,
        motion: bool,
    ) -> BoxFuture<'_, Result<BackupRestored, ControlError>>;
    /// The presets of a motorised camera (ONVIF PTZ); fails when the camera
    /// has no ONVIF service configured or announces no PTZ.
    fn ptz_presets(&self, camera: String) -> BoxFuture<'_, Result<Vec<PtzPreset>, ControlError>>;
    /// Moves a motorised camera, or changes its presets.
    fn ptz_command(
        &self,
        camera: String,
        command: PtzCommand,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
}

/// A position stored on a motorised camera.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PtzPreset {
    /// Its token (`POST …/ptz/presets/{token}/goto`).
    pub token: String,
    /// Its name.
    pub name: String,
}

/// Speeds of a continuous move, each from -1 to 1 (absent: 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PtzMove {
    /// Left (< 0) or right (> 0).
    #[serde(default)]
    pub pan: f32,
    /// Down (< 0) or up (> 0).
    #[serde(default)]
    pub tilt: f32,
    /// Out (< 0) or in (> 0).
    #[serde(default)]
    pub zoom: f32,
}

/// What to do with a motorised camera.
#[derive(Clone, Debug, PartialEq)]
pub enum PtzCommand {
    /// Moves until [`PtzCommand::Stop`] (or a few seconds at most).
    Move(PtzMove),
    /// Stops moving.
    Stop,
    /// Goes to a preset (its token).
    Goto(String),
    /// Stores the current position as a new preset with this name.
    SetPreset(String),
    /// Deletes a preset (its token).
    RemovePreset(String),
}

/// Object recognition settings and state (`GET /api/v1/ai`).
#[derive(Clone, Debug, Serialize)]
pub struct AiView {
    /// Settings in effect, as in `[ai]` (API keys are only referenced by the
    /// name of their environment variable).
    pub settings: AiConfig,
    /// Provider running (`onnx`, `deepstack_http`), if any.
    pub active: Option<String>,
    /// Where its analyses run (`CPU`, `GPU`…): always `CPU` for the built-in
    /// recognition, as last reported by CodeProject.AI, unknown otherwise.
    pub device: Option<String>,
    /// Why the configured provider is not running.
    pub error: Option<String>,
    /// The built-in recognition is compiled in.
    pub onnx_available: bool,
    /// Motion detection can run (database and FFmpeg): recognition needs it.
    pub detection_ready: bool,
    /// The settings come from the interface (`ai.ui.toml`).
    pub managed: bool,
    /// Models of the built-in recognition that can be downloaded.
    pub models: Vec<AiModelView>,
}

/// Answer of a webhook to `POST /api/v1/webhooks/test`.
#[derive(Clone, Debug, Serialize)]
pub struct WebhookTestResult {
    /// HTTP status answered, if the address answered.
    pub status: Option<u16>,
    /// Time taken.
    pub ms: u64,
    /// Why the call failed (no answer, error status).
    pub error: Option<String>,
}

/// A downloadable model (`GET /api/v1/ai`, `POST /api/v1/ai/models/{id}`).
#[derive(Clone, Debug, Serialize)]
pub struct AiModelView {
    /// Identifier (`yolox_nano`).
    pub id: String,
    /// Name shown.
    pub name: String,
    /// Size of the file.
    pub bytes: u64,
    /// Licence of the model.
    pub license: String,
    /// What to expect (speed, accuracy).
    pub hint: String,
    /// Where it is (or will be) on the server.
    pub path: String,
    /// The file is there.
    pub downloaded: bool,
}

/// One object found by `POST /api/v1/ai/test`.
#[derive(Clone, Debug, Serialize)]
pub struct AiTestObject {
    /// What was found.
    #[serde(flatten)]
    pub object: DetectedObject,
    /// It passes the filters (confidence, labels, camera objects).
    pub kept: bool,
}

/// Server checked by `POST /api/v1/ai/probe`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiProbeInput {
    /// Base address (`http://127.0.0.1:32168`).
    pub url: String,
    /// Environment variable holding the API key, if any.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

/// Answer of `POST /api/v1/ai/probe`.
#[derive(Clone, Debug, Serialize)]
pub struct AiProbeResult {
    /// The server answered and its object detection works.
    pub ok: bool,
    /// Time taken.
    pub ms: u64,
    /// Where the detection ran (`CPU`, `GPU`…), when the server tells.
    pub device: Option<String>,
    /// Why it failed, in French (never the address).
    pub error: Option<String>,
}

/// Result of `POST /api/v1/ai/test?camera=…`.
#[derive(Clone, Debug, Serialize)]
pub struct AiTestResult {
    /// Duration of the analysis.
    pub ms: u64,
    /// Where it ran (`CPU`, `GPU`…), when the provider tells.
    pub device: Option<String>,
    /// Everything found, most confident first.
    pub objects: Vec<AiTestObject>,
    /// The analysed picture, as a `data:image/jpeg;base64,…` URL.
    pub image: String,
}

/// General settings sent by the interface (`PUT /api/v1/settings`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsInput {
    /// `[recording]`.
    pub recording: RecordingConfig,
    /// `[motion]`.
    pub motion: MotionConfig,
    /// `live.transcode`.
    pub live_transcode: bool,
    /// `live.max_bitrate_kbps`.
    pub live_max_bitrate_kbps: u32,
    /// `live.hwaccel`; absent keeps the current one.
    #[serde(default)]
    pub live_hwaccel: Option<rustnvr_core::config::HwAccelMode>,
    /// `storage.low_space_percent`.
    pub low_space_percent: u8,
    /// `storage.default_destination`; absent keeps the current one.
    #[serde(default)]
    pub default_destination: Option<String>,
    /// `[restream]` (RTSP republication); absent keeps the current one.
    #[serde(default)]
    pub restream: Option<RestreamInput>,
}

/// RTSP republication settings sent by the interface.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestreamInput {
    /// Serve the cameras over RTSP.
    pub enabled: bool,
    /// Listening address.
    pub listen: String,
    /// Most clients at a time.
    pub max_clients: u32,
    /// Most clients at a time from one address; absent keeps the current one.
    #[serde(default)]
    pub max_clients_per_address: Option<u32>,
    /// Listen on the network without credentials.
    #[serde(default)]
    pub allow_unauthenticated_remote: bool,
    /// User name asked to RTSP clients (`null` or empty: none).
    #[serde(default)]
    pub username: Option<String>,
    /// New password; absent or `null` keeps the stored one, `""` removes it.
    #[serde(default)]
    pub password: Option<Secret>,
}

/// RTSP republication settings shown by the interface. Never contains the
/// password.
#[derive(Clone, Debug, Serialize)]
pub struct RestreamView {
    /// Serve the cameras over RTSP.
    pub enabled: bool,
    /// Listening address.
    pub listen: String,
    /// Most clients at a time.
    pub max_clients: u32,
    /// Most clients at a time from one address.
    pub max_clients_per_address: u32,
    /// Listen on the network without credentials.
    pub allow_unauthenticated_remote: bool,
    /// User name asked to RTSP clients.
    pub username: Option<String>,
    /// Whether a password is configured (directly or through an environment
    /// variable).
    pub has_password: bool,
}

/// General settings shown by the interface (`GET /api/v1/settings`).
#[derive(Clone, Debug, Serialize)]
pub struct SettingsView {
    /// `[recording]`.
    pub recording: RecordingConfig,
    /// `[motion]`.
    pub motion: MotionConfig,
    /// `live.transcode`.
    pub live_transcode: bool,
    /// `live.max_bitrate_kbps`.
    pub live_max_bitrate_kbps: u32,
    /// `live.hwaccel` (state of the card: `GET /system`, `hwaccel`).
    pub live_hwaccel: rustnvr_core::config::HwAccelMode,
    /// `storage.low_space_percent`.
    pub low_space_percent: u8,
    /// `storage.default_destination`: destination of cameras whose recording
    /// is enabled without choosing one.
    pub default_destination: String,
    /// `[restream]`: RTSP republication of the cameras.
    pub restream: RestreamView,
    /// Why the republication is enabled but not served (address in use…).
    pub restream_error: Option<String>,
    /// Read-only here: `live.ffmpeg`.
    pub ffmpeg: String,
    /// Settings were changed from the interface (`settings.ui.toml`).
    pub managed: bool,
}

/// MQTT settings sent by the interface (`PUT /api/v1/mqtt`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MqttInput {
    /// Connect to the broker.
    pub enabled: bool,
    /// Broker host.
    pub host: String,
    /// Broker port.
    pub port: u16,
    /// Encrypt the connection (TLS).
    #[serde(default)]
    pub tls: bool,
    /// PEM file of the broker's certificate authority, on the server.
    #[serde(default)]
    pub ca_file: Option<String>,
    /// User name.
    #[serde(default)]
    pub username: Option<String>,
    /// New password; absent or `null` keeps the stored one, `""` removes it.
    #[serde(default)]
    pub password: Option<Secret>,
    /// MQTT client identifier.
    pub client_id: String,
    /// Root of RustNVR topics.
    pub topic_prefix: String,
    /// Publish Home Assistant discovery messages.
    pub discovery: bool,
    /// Home Assistant discovery prefix.
    pub discovery_prefix: String,
}

/// MQTT settings and state shown by the interface. Never contains the password.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MqttView {
    /// Connect to the broker.
    pub enabled: bool,
    /// Broker host.
    pub host: String,
    /// Broker port.
    pub port: u16,
    /// TLS.
    pub tls: bool,
    /// Certificate authority file, if any.
    pub ca_file: Option<String>,
    /// User name.
    pub username: Option<String>,
    /// Whether a password is configured (directly or through an environment variable).
    pub has_password: bool,
    /// MQTT client identifier.
    pub client_id: String,
    /// Root of RustNVR topics.
    pub topic_prefix: String,
    /// Home Assistant discovery.
    pub discovery: bool,
    /// Home Assistant discovery prefix.
    pub discovery_prefix: String,
    /// Settings come from the web interface (`mqtt.ui.toml`) rather than `config.toml`.
    pub managed: bool,
    /// Connected to the broker.
    pub connected: bool,
    /// Last connection error, if not connected.
    pub error: Option<String>,
}

/// A file of a backup compared with the current one.
#[derive(Clone, Debug, Serialize)]
pub struct BackupFile {
    /// File name (`config.toml`, `cameras.ui.toml`…).
    pub name: String,
    /// `same`, `changed`, `new` (only in the backup) or `not_in_backup`
    /// (kept as it is).
    pub state: &'static str,
    /// Why restoring this file (alone, with the current others) would not
    /// load, e.g. a model file missing here.
    pub problem: Option<String>,
}

/// A camera of the configuration once a backup is restored.
#[derive(Clone, Debug, Serialize)]
pub struct BackupCamera {
    /// Identifier.
    pub id: String,
    /// Display name.
    pub name: String,
}

/// What restoring a backup would do (`POST /api/v1/backup/inspect`).
#[derive(Clone, Debug, Serialize)]
pub struct BackupPreview {
    /// When the backup was made, in milliseconds since the Unix epoch.
    pub created_ms: i64,
    /// Version of RustNVR that made it.
    pub rustnvr: String,
    /// Each file.
    pub files: Vec<BackupFile>,
    /// Cameras once every file of the backup is restored.
    pub cameras: Vec<BackupCamera>,
    /// Cameras whose detection settings (zones, lines, masks) are in it.
    pub motion: Vec<String>,
    /// Why the restored configuration would not load (empty when it would).
    pub problems: Vec<String>,
}

/// What a restore did (`POST /api/v1/backup/restore`).
#[derive(Clone, Debug, Serialize)]
pub struct BackupRestored {
    /// Files written.
    pub files: Vec<String>,
    /// Cameras whose detection settings were restored.
    pub motion: usize,
    /// Always true: the server uses them from its next start.
    pub restart_needed: bool,
}

/// Kept pictures of one camera on one day (`GET /api/v1/ai/captures`).
#[derive(Clone, Debug, Serialize)]
pub struct CaptureDay {
    /// Camera identifier.
    pub camera: String,
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    /// Pictures.
    pub count: usize,
    /// Size on disk.
    pub bytes: u64,
}

/// A kept picture and what the recognition found in it.
#[derive(Clone, Debug, Serialize)]
pub struct CapturePicture {
    /// File name (`GET /api/v1/ai/captures/{camera}/{day}/{file}`).
    pub file: String,
    /// When it was analysed.
    pub at_ms: i64,
    /// Event and picture number within it, if it was an event picture.
    pub event: Option<i64>,
    /// Picture number within the event.
    pub picture: Option<u32>,
    /// Crossing line it was checking, if any.
    pub line: Option<String>,
    /// Recognition used.
    pub provider: String,
    /// Time taken.
    pub ms: u64,
    /// Objects found.
    pub found: Vec<rustnvr_core::DetectedObject>,
    /// Labels that counted for the camera (objects, zones, line).
    pub kept: Vec<String>,
}

/// A kept picture analysed again (`POST /api/v1/ai/replay`).
#[derive(Clone, Debug, Serialize)]
pub struct ReplayPicture {
    /// The picture as kept (what was found then).
    pub before: CapturePicture,
    /// Objects found now, with the current recognition settings.
    pub found: Vec<rustnvr_core::DetectedObject>,
    /// Labels that count now (current objects, zones, line).
    pub kept: Vec<String>,
    /// Time taken now.
    pub ms: u64,
    /// The labels that count differ from then.
    pub changed: bool,
}

/// The recognition replayed on the kept pictures of a day.
#[derive(Clone, Debug, Serialize)]
pub struct ReplayResult {
    /// Recognition used now.
    pub provider: String,
    /// Every picture replayed, in time order.
    pub pictures: Vec<ReplayPicture>,
    /// Pictures whose counted labels changed.
    pub changed: usize,
    /// Pictures of the day left out (beyond the limit).
    pub skipped: usize,
}
