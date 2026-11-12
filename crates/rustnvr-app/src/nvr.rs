//! Wiring of ingestion, recording, retention, motion detection, MQTT and the
//! web interface; cameras, storage destinations, detection and MQTT settings
//! can be changed at run time through the API.

mod backups;
mod detection;
mod index;
mod night;
mod ptz;
mod replay;
mod restream;
mod updates;
mod usb;
mod webhooks;

use crate::{camera_store, storage_store};
use anyhow::Context as _;
use detection::MqttRuntime;
use rustnvr_api::control::BoxFuture;
use rustnvr_api::sysinfo::{GroupSample, ProcSample};
use rustnvr_api::{
    ApiServer, ApiState, CameraEditInfo, CameraInput, CameraSource, ControlError, DestinationInfo,
    DestinationInput, LiveSettings, MqttInput, MqttView, NvrControl, SettingsInput, SettingsView,
    StreamInput, StreamSource,
};
use rustnvr_core::config::{
    CameraConfig, CameraRecordConfig, Config, DestinationConfig, LOCAL_DESTINATION, OnvifConfig,
    ReconnectConfig, RecordMode, RecordingConfig, RemoteConfig, RetentionConfig, StreamConfig,
};
use rustnvr_core::{CameraId, EventBus, MotionSettings, NvrEvent, RtspTarget, StreamRole};
use rustnvr_motion::{MotionHandle, MotionOptions, MotionUpdate, spawn_motion};
use rustnvr_recorder::retention::{
    Protected, RetentionPolicy, enforce, enforce_camera, free_space,
};
use rustnvr_recorder::{
    Destination, DestinationStore, MotionGate, RecordTrigger, RecorderCore, RecorderHandle,
    RecorderSettings, recover_tree, spawn_recorder,
};
use rustnvr_remote::{UploadStatus, UploaderHandle};
use rustnvr_storage::Database;
use rustnvr_stream::{
    BackoffConfig, FrameFanout, LiveTap, SessionOptions, StreamHandle, StreamMetricsSnapshot,
    SupervisorConfig, spawn_supervisor,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// How long shutdown waits for each supervisor and recorder.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
/// Identifiers that would clash with API routes (`/cameras/probe`).
const RESERVED_IDS: &[&str] = &["probe"];
/// Retention runs shortly after startup, then at this interval.
const RETENTION_INTERVAL: Duration = Duration::from_secs(600);
const RETENTION_FIRST_RUN: Duration = Duration::from_secs(30);
/// Free space is checked at this interval.
const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// The running parts of one enabled camera.
struct CameraRuntime {
    id: CameraId,
    /// Stops this camera only.
    cancel: CancellationToken,
    stream: StreamHandle,
    /// One recorder per destination.
    recorders: Vec<RecorderHandle>,
    live: LiveTap,
    /// Sub stream, connected for the live view only (never recorded).
    sub: Option<(StreamHandle, LiveTap)>,
    /// Motion detection, when enabled.
    motion: Option<MotionHandle>,
    /// Record the sound (switch shared by the recorders).
    record_audio: Arc<AtomicBool>,
    /// The main stream was started with its audio.
    audio_requested: bool,
}

impl CameraRuntime {
    /// Stream analysed by motion detection and used for thumbnails: the
    /// sub stream when there is one (lighter to decode).
    fn detection_tap(&self) -> LiveTap {
        self.sub
            .as_ref()
            .map_or_else(|| self.live.clone(), |(_, t)| t.clone())
    }

    /// Stops the streams, lets the recorders close their segments; false on timeout.
    async fn stop(mut self) -> bool {
        // Detection first, so that an ongoing event ends cleanly.
        if let Some(m) = self.motion.take() {
            m.stop().await;
        }
        self.cancel.cancel();
        let mut clean = true;
        if let Some((sub, _)) = self.sub
            && tokio::time::timeout(SHUTDOWN_TIMEOUT, sub.task)
                .await
                .is_err()
        {
            warn!(camera = %self.id, "sub stream supervisor did not stop in time");
            clean = false;
        }
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, self.stream.task)
            .await
            .is_err()
        {
            warn!(camera = %self.id, "stream supervisor did not stop in time");
            clean = false;
        }
        // The supervisor owned the only senders of the recorder queues, so the
        // recorders now drain them, close their segments and exit.
        for recorder in self.recorders {
            if !recorder.wait(SHUTDOWN_TIMEOUT).await {
                warn!(camera = %self.id, "recorder did not finish in time; startup recovery will repair its segment");
                clean = false;
            }
        }
        clean
    }
}

/// Storage settings in effect.
#[derive(Clone)]
struct Storage {
    local_dir: PathBuf,
    local_retention: RetentionConfig,
    destinations: Vec<DestinationConfig>,
}

impl Storage {
    fn local(&self) -> Destination {
        Destination {
            id: LOCAL_DESTINATION.to_owned(),
            name: "Disque local".to_owned(),
            root: self.local_dir.clone(),
            require_mount: false,
        }
    }

    fn destination(&self, id: &str) -> Option<Destination> {
        if id == LOCAL_DESTINATION {
            return Some(self.local());
        }
        self.destinations
            .iter()
            .find(|d| d.id == id)
            .map(|d| Destination {
                id: d.id.clone(),
                name: d.name.clone().unwrap_or_else(|| d.id.clone()),
                root: d.path.clone(),
                // The outbox of a remote destination is local.
                require_mount: d.require_mount && d.remote.is_none(),
            })
    }

    /// Where the analysed pictures are kept: the default destination when it
    /// is `usable`, else the local recordings directory (also for a remote
    /// destination, whose root is an outbox), with its retention days.
    fn captures(&self, default_id: &str, usable: impl Fn(&str) -> bool) -> (PathBuf, Option<u32>) {
        if let Some(d) = self
            .destinations
            .iter()
            .find(|d| d.id == default_id && d.remote.is_none())
            && usable(&d.id)
        {
            return (d.path.clone(), d.retention_days);
        }
        (self.local_dir.clone(), self.local_retention.retention_days)
    }

    /// Every destination with its retention policy, local first.
    fn all(&self) -> Vec<(Destination, RetentionConfig)> {
        std::iter::once((self.local(), self.local_retention))
            .chain(
                self.destinations
                    .iter()
                    .filter_map(|d| Some((self.destination(&d.id)?, d.retention()))),
            )
            .collect()
    }

    fn infos(&self) -> Vec<DestinationInfo> {
        let mut out = vec![DestinationInfo {
            id: LOCAL_DESTINATION.to_owned(),
            name: "Disque local".to_owned(),
            root: self.local_dir.clone(),
            require_mount: false,
            managed: false,
            retention: self.local_retention,
            remote: None,
            upload: None,
        }];
        out.extend(self.destinations.iter().map(|d| DestinationInfo {
            id: d.id.clone(),
            name: d.name.clone().unwrap_or_else(|| d.id.clone()),
            root: d.path.clone(),
            require_mount: d.require_mount,
            managed: d.managed,
            retention: d.retention(),
            remote: d.remote.clone(),
            upload: None,
        }));
        out
    }
}

/// Everything needed to start a camera at any time.
struct Engine {
    events: EventBus,
    recording: std::sync::RwLock<RecordingConfig>,
    reconnect: ReconnectConfig,
    cancel: CancellationToken,
    motion: std::sync::RwLock<MotionOptions>,
    /// Motion-triggered recording, by camera.
    triggers: std::sync::Mutex<HashMap<CameraId, Arc<RecordTrigger>>>,
    /// Where detections go; `None` when detection is unavailable (no
    /// database or no FFmpeg) and after shutdown.
    motion_tx: std::sync::Mutex<Option<mpsc::Sender<MotionUpdate>>>,
    /// Motion detection paused on every camera (the settings of each camera
    /// stay); kept across restarts by a file in the data directory.
    detection_paused: AtomicBool,
    /// The API once started (shared with [`Shared`]): USB cameras encode
    /// on the graphics card it found.
    api: Arc<OnceLock<Arc<ApiState>>>,
}

impl Engine {
    /// Recording settings in effect.
    fn recording(&self) -> RecordingConfig {
        self.recording
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Detection settings in effect.
    fn motion_options(&self) -> MotionOptions {
        self.motion
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Recording trigger of a camera recording on motion.
    fn trigger(&self, camera: &CameraId) -> Option<Arc<RecordTrigger>> {
        self.triggers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(camera)
            .cloned()
    }

    fn motion_sender(&self) -> Option<mpsc::Sender<MotionUpdate>> {
        self.motion_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Starts detection on a running camera if its settings enable it.
    fn start_motion(&self, rt: &mut CameraRuntime, settings: Option<&MotionSettings>) {
        let Some(settings) = settings.filter(|s| s.enabled) else {
            return;
        };
        if self.detection_paused.load(Ordering::Relaxed) {
            debug!(camera = %rt.id, "motion detection paused");
            return;
        }
        let Some(sink) = self.motion_sender() else {
            warn!(camera = %rt.id, "motion detection unavailable (database or FFmpeg missing)");
            return;
        };
        rt.motion = Some(spawn_motion(
            rt.id.clone(),
            rt.detection_tap(),
            settings.clone(),
            self.motion_options(),
            sink,
            rt.cancel.child_token(),
        ));
    }

    fn supervise(
        &self,
        cam: &CameraId,
        role: StreamRole,
        cfg: &StreamConfig,
        fanout: FrameFanout,
        cancel: &CancellationToken,
        audio: bool,
    ) -> anyhow::Result<StreamHandle> {
        let target = cfg
            .resolve()
            .map_err(|e| anyhow::anyhow!("camera {cam} {role} stream: {e}"))?;
        Ok(spawn_supervisor(
            SupervisorConfig {
                camera: cam.clone(),
                role,
                target,
                session: SessionOptions {
                    audio,
                    ..SessionOptions::from_config(cfg)
                },
                backoff: BackoffConfig::from(&self.reconnect),
            },
            fanout,
            self.events.clone(),
            cancel.child_token(),
        ))
    }

    /// Starts an enabled camera, with one recorder per recording destination
    /// and motion detection if enabled.
    fn start(
        &self,
        cam: &CameraConfig,
        storage: &Storage,
        motion: Option<&MotionSettings>,
    ) -> anyhow::Result<CameraRuntime> {
        let cancel = self.cancel.child_token();
        let mut fanout = FrameFanout::new();
        let live = fanout.live_tap();
        let recording = self.recording();
        let mut recorders = Vec::new();
        let record_audio = Arc::new(AtomicBool::new(cam.record.audio));
        // Sound for the recording and for the live view (both streams).
        let audio_requested = cam.record.audio;
        // Motion-triggered recording: one trigger per camera, set by detection.
        let trigger = (cam.record.enabled && cam.record.mode == RecordMode::Motion).then(|| {
            let t = Arc::new(RecordTrigger::default());
            self.triggers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(cam.id.clone(), t.clone());
            t
        });
        if cam.record.enabled {
            let records_locally = cam
                .record
                .destinations
                .iter()
                .any(|d| d == LOCAL_DESTINATION);
            for dest_id in &cam.record.destinations {
                let dest = storage.destination(dest_id).ok_or_else(|| {
                    anyhow::anyhow!("camera {}: unknown destination {dest_id:?}", cam.id)
                })?;
                // A camera recording only to remote destinations keeps
                // recording locally while they are unavailable.
                let fallback = (dest.id != LOCAL_DESTINATION && !records_locally)
                    .then(|| storage.local_dir.clone());
                let store = DestinationStore::new(&cam.id, dest, fallback);
                let (rx, _) =
                    fanout.subscribe(format!("recorder:{dest_id}"), recording.queue_frames);
                let core = RecorderCore::new(
                    cam.id.clone(),
                    RecorderSettings::from(&recording),
                    Box::new(store),
                )
                .with_audio_switch(record_audio.clone());
                recorders.push(
                    spawn_recorder(
                        core,
                        rx,
                        self.events.clone(),
                        trigger
                            .as_ref()
                            .map(|t| MotionGate::new(t.clone(), recording.pre_roll())),
                    )
                    .with_context(|| format!("cannot start recorder thread for {}", cam.id))?,
                );
            }
        }
        let sub = match &cam.sub {
            Some(sub_cfg) => {
                let mut sub_fanout = FrameFanout::new();
                let tap = sub_fanout.live_tap();
                Some((
                    self.supervise(
                        &cam.id,
                        StreamRole::Sub,
                        sub_cfg,
                        sub_fanout,
                        &cancel,
                        audio_requested,
                    )?,
                    tap,
                ))
            }
            None => None,
        };
        let stream = match &cam.usb {
            // A webcam: FFmpeg reads it and writes H.264.
            Some(usb) => rustnvr_stream::spawn_device_supervisor(
                rustnvr_stream::DeviceConfig {
                    camera: cam.id.clone(),
                    label: usb.device.clone(),
                    ffmpeg: self.motion_options().ffmpeg,
                    command: usb::command_source(self.api.clone(), usb.clone()),
                    stall_timeout: cam.main.stall_timeout(),
                    backoff: BackoffConfig::from(&self.reconnect),
                },
                fanout,
                self.events.clone(),
                cancel.child_token(),
            ),
            None => self.supervise(
                &cam.id,
                StreamRole::Main,
                &cam.main,
                fanout,
                &cancel,
                audio_requested,
            )?,
        };
        let mut rt = CameraRuntime {
            id: cam.id.clone(),
            cancel,
            stream,
            recorders,
            live,
            sub,
            motion: None,
            record_audio,
            audio_requested,
        };
        self.start_motion(&mut rt, motion);
        Ok(rt)
    }
}

/// Configured cameras and storage, and the running cameras.
struct State {
    configs: Vec<CameraConfig>,
    running: HashMap<CameraId, CameraRuntime>,
    storage: Storage,
    /// Motion detection settings, by camera (absent: defaults, disabled).
    motion: HashMap<CameraId, MotionSettings>,
    /// Cameras paused from the interface or MQTT: stopped until RustNVR
    /// restarts (never stored) or the pause is lifted.
    paused: HashSet<CameraId>,
    /// Cameras whose night vision is known (see `night`): true in night vision.
    night: HashMap<CameraId, bool>,
}

fn stream_source(h: &StreamHandle) -> StreamSource {
    StreamSource {
        state: h.state.clone(),
        metrics: h.metrics.clone(),
    }
}

impl State {
    /// Whether `cam` should run: enabled, and not paused.
    fn runs(&self, cam: &CameraConfig) -> bool {
        cam.enabled && !self.paused.contains(&cam.id)
    }

    /// What the API shows for each camera.
    fn sources(&self) -> Vec<CameraSource> {
        self.configs
            .iter()
            .map(|cam| {
                let runtime = self.running.get(&cam.id);
                // Only the host and port are exposed; disabled cameras may lack their secrets.
                let main_target = cam.main.resolve_with(|_| Some(String::new())).ok();
                CameraSource {
                    id: cam.id.clone(),
                    name: cam.name.clone(),
                    enabled: cam.enabled,
                    paused: self.paused.contains(&cam.id),
                    night: self.night.get(&cam.id).copied(),
                    recording_enabled: cam.record.enabled,
                    has_sub_stream: cam.sub.is_some(),
                    endpoint: match &cam.usb {
                        Some(usb) => format!("usb:{}", usb.device),
                        None => main_target
                            .as_ref()
                            .map_or_else(|| "?".to_owned(), RtspTarget::redacted),
                    },
                    stream: runtime.map(|r| stream_source(&r.stream)),
                    recorders: runtime.map_or_else(Vec::new, |r| {
                        r.recorders.iter().map(|h| h.metrics().clone()).collect()
                    }),
                    record_mode: cam.record.mode,
                    retention: cam.record.retention(),
                    record_destinations: if cam.record.enabled {
                        cam.record.destinations.clone()
                    } else {
                        Vec::new()
                    },
                    motion_enabled: self.motion.get(&cam.id).is_some_and(|m| m.enabled),
                    record_audio: cam.record.audio,
                    privacy_masks: self
                        .motion
                        .get(&cam.id)
                        .map(|m| m.mask_rects())
                        .unwrap_or_default(),
                    ptz: cam.onvif.is_some(),
                    motion: runtime.and_then(|r| r.motion.as_ref().map(|m| m.status.clone())),
                    live: runtime.map(|r| r.live.clone()),
                    sub_stream: runtime.and_then(|r| r.sub.as_ref().map(|(h, _)| stream_source(h))),
                    live_sub: runtime.and_then(|r| r.sub.as_ref().map(|(_, t)| t.clone())),
                    edit: cam.managed.then(|| CameraEditInfo {
                        main_url: main_target
                            .as_ref()
                            .map_or_else(String::new, |t| t.url().to_string()),
                        sub_url: cam
                            .sub
                            .as_ref()
                            .and_then(|s| s.resolve_with(|_| Some(String::new())).ok())
                            .map(|t| t.url().to_string()),
                        username: cam.main.username.clone(),
                        has_password: cam.main.password.as_ref().is_some_and(|p| !p.is_empty()),
                        transport: cam.main.transport,
                        onvif_port: cam.onvif.as_ref().map(|o| o.port),
                        usb: cam.usb.clone(),
                        restream_username: cam.restream.as_ref().map(|r| r.username.clone()),
                        restream_has_password: cam.restream.as_ref().is_some_and(|r| {
                            r.password.as_ref().is_some_and(|p| !p.is_empty())
                                || r.password_env.is_some()
                        }),
                    }),
                }
            })
            .collect()
    }
}

/// State shared by the NVR, its background tasks and the API controller.
struct Shared {
    engine: Engine,
    state: tokio::sync::Mutex<State>,
    api: Arc<OnceLock<Arc<ApiState>>>,
    managed_cameras_file: Option<PathBuf>,
    managed_storage_file: Option<PathBuf>,
    managed_mqtt_file: Option<PathBuf>,
    /// Events and detection settings; `None` if the database could not be opened.
    database: Option<Database>,
    /// FFmpeg executable (snapshots, detection), and whether it works.
    ffmpeg: String,
    ffmpeg_ok: bool,
    /// Optional object recognition on event pictures, replaced when the
    /// settings change from the interface.
    ai: std::sync::RwLock<Option<rustnvr_ai::Analyzer>>,
    /// `[ai]` settings in effect, and why the provider is not running.
    ai_settings: std::sync::Mutex<(rustnvr_core::config::AiConfig, Option<String>)>,
    managed_ai_file: Option<PathBuf>,
    /// `[updates]` settings in effect (watched repository).
    updates: std::sync::Mutex<rustnvr_core::config::UpdatesConfig>,
    managed_updates_file: Option<PathBuf>,
    /// One check for new versions at a time.
    update_lock: tokio::sync::Mutex<()>,
    /// Webhooks in effect (`[[webhooks]]` or `webhooks.ui.toml`).
    webhooks: std::sync::RwLock<Vec<rustnvr_core::config::WebhookConfig>>,
    managed_webhooks_file: Option<PathBuf>,
    /// The configuration file (backups).
    config_file: Option<PathBuf>,
    mqtt: tokio::sync::Mutex<MqttRuntime>,
    /// General settings in effect (`[recording]`, `[motion]`, live view,
    /// low-space threshold), editable from the interface.
    general: std::sync::Mutex<detection::GeneralSettings>,
    managed_settings_file: Option<PathBuf>,
    /// RTSP republication server, while it is served.
    restream: tokio::sync::Mutex<Option<rustnvr_restream::RestreamServer>>,
    /// Why the republication is enabled but not served.
    restream_error: std::sync::Mutex<Option<String>>,
    /// Destinations short of space (names), updated every minute.
    low_space: std::sync::Mutex<Vec<String>>,
    /// Destinations that cannot be used (name, problem), updated every
    /// minute.
    storage_problems: std::sync::Mutex<Vec<(String, String)>>,
    /// Where analysed pictures are kept (`ai.keep_pictures`) and for how
    /// many days; `None` until the first storage check.
    captures_root: std::sync::Mutex<Option<(PathBuf, Option<u32>)>>,
    /// Database, thumbnails and outboxes of remote destinations.
    data_dir: PathBuf,
    /// Manual events (Home Assistant button), written with the detections;
    /// `None` without database.
    manual: Option<mpsc::Sender<detection::Manual>>,
    /// One uploader per S3 / SFTP destination.
    uploaders: tokio::sync::Mutex<HashMap<String, UploaderHandle>>,
    /// Their states, for the API.
    upload_status: std::sync::Mutex<HashMap<String, watch::Receiver<UploadStatus>>>,
    /// ONVIF PTZ sessions of motorised cameras, opened on first use.
    ptz: ptz::Sessions,
}

impl Shared {
    /// Updates the API's view and notifies WebSocket clients.
    /// General settings in effect.
    /// The object recognition in effect, if any.
    fn ai(&self) -> Option<rustnvr_ai::Analyzer> {
        self.ai.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn general(&self) -> detection::GeneralSettings {
        self.general
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Destinations for the API, with the upload state of remote ones.
    fn destination_infos(&self, state: &State) -> Vec<DestinationInfo> {
        let status = self.upload_status.lock().unwrap_or_else(|e| e.into_inner());
        let mut infos = state.storage.infos();
        for i in &mut infos {
            i.upload = status.get(&i.id).cloned();
        }
        infos
    }

    fn publish(&self, state: &State) {
        if let Some(api) = self.api.get() {
            api.set_detection_paused(self.engine.detection_paused.load(Ordering::Relaxed));
            api.set_cameras(state.sources());
            api.set_destinations(self.destination_infos(state));
        }
        let _ = self.engine.events.send(NvrEvent::CamerasChanged);
    }

    fn save_cameras(&self, cameras: &[CameraConfig]) -> Result<(), ControlError> {
        let path = self.managed_cameras_file.as_ref().ok_or_else(|| {
            ControlError::Internal("no configuration file to store cameras next to".into())
        })?;
        camera_store::save(path, cameras.iter().filter(|c| c.managed)).map_err(|e| {
            warn!(path = %path.display(), error = %e, "cannot save managed cameras");
            ControlError::Internal(format!("cannot write {}: {e}", path.display()))
        })
    }

    fn save_storage(&self, storage: &Storage) -> Result<(), ControlError> {
        let path = self.managed_storage_file.as_ref().ok_or_else(|| {
            ControlError::Internal("no configuration file to store settings next to".into())
        })?;
        storage_store::save(
            path,
            storage.local_retention,
            storage.destinations.iter().filter(|d| d.managed),
        )
        .map_err(|e| {
            warn!(path = %path.display(), error = %e, "cannot save storage settings");
            ControlError::Internal(format!("cannot write {}: {e}", path.display()))
        })
    }
}

/// Records camera `raw_id` all the time, on motion, or not at all (`None`),
/// and restarts it (Home Assistant selector).
async fn set_record_mode(
    shared: &Arc<Shared>,
    raw_id: &str,
    mode: Option<RecordMode>,
) -> Result<(), ControlError> {
    let mut state = shared.state.lock().await;
    let index = state
        .configs
        .iter()
        .position(|c| c.id.as_str() == raw_id)
        .ok_or_else(|| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
    let previous = state.configs[index].clone();
    let enabled = mode.is_some();
    let new_mode = mode.unwrap_or(previous.record.mode);
    if previous.record.enabled == enabled && previous.record.mode == new_mode {
        return Ok(());
    }
    if !previous.managed {
        return Err(ControlError::ReadOnly(format!(
            "la caméra {raw_id:?} est déclarée dans config.toml : réglez « record = {{ enabled = …, mode = … }} » dans ce fichier puis redémarrez RustNVR"
        )));
    }
    let mut cam = previous.clone();
    cam.record.enabled = enabled;
    cam.record.mode = new_mode;
    // Recording again without a destination: the default one.
    if enabled && cam.record.destinations.is_empty() {
        let default = shared.general().default_destination;
        cam.record.destinations = vec![if state.storage.destination(&default).is_some() {
            default
        } else {
            LOCAL_DESTINATION.to_owned()
        }];
    }
    state.configs[index] = cam.clone();
    if let Err(e) = shared.save_cameras(&state.configs) {
        state.configs[index] = previous;
        return Err(e);
    }
    if let Some(old) = state.running.remove(&cam.id) {
        old.stop().await;
    }
    if state.runs(&cam) {
        let rt = shared
            .engine
            .start(&cam, &state.storage, state.motion.get(&cam.id))
            .map_err(internal)?;
        state.running.insert(cam.id.clone(), rt);
    }
    if mode == Some(RecordMode::Motion) && !state.motion.get(&cam.id).is_some_and(|m| m.enabled) {
        warn!(camera = %cam.id, "recording on motion without motion detection: nothing will be recorded");
    }
    info!(camera = %cam.id, recording = enabled, mode = new_mode.as_str(), "recording mode changed");
    shared.publish(&state);
    Ok(())
}

/// Repairs interrupted segments on every available destination.
async fn recover_all(storage: &Storage) {
    for (dest, _) in storage.all() {
        if let Some(reason) = dest.unavailable_reason() {
            warn!(destination = %dest.id, %reason, "destination unavailable at startup: recovery skipped");
            continue;
        }
        let root = dest.root.clone();
        let Ok(report) = tokio::task::spawn_blocking(move || recover_tree(&root)).await else {
            continue;
        };
        if !report.recovered.is_empty() || !report.deleted.is_empty() {
            info!(destination = %dest.id, recovered = report.recovered.len(), deleted = report.deleted.len(), "recovered interrupted segments");
        }
        for (path, error) in &report.failed {
            warn!(path = %path.display(), %error, "could not recover segment");
        }
    }
}

/// A running NVR.
pub struct Nvr {
    shared: Arc<Shared>,
    background: Vec<JoinHandle<()>>,
    /// Writes detections to the database; ends after the last detection stops.
    detection_events: Option<JoinHandle<()>>,
    web: Option<ApiServer>,
}

impl Nvr {
    /// Recovers interrupted segments, then starts one supervisor (and one
    /// recorder per destination) per enabled camera. Events are published on `events`.
    pub async fn start(config: &Config, events: EventBus) -> anyhow::Result<Self> {
        let root = config.storage.recordings_dir.clone();
        std::fs::create_dir_all(&root)
            .with_context(|| format!("cannot create recordings directory {}", root.display()))?;
        let storage = Storage {
            local_dir: root.clone(),
            local_retention: config.storage.local_retention(),
            destinations: config.storage.destinations.clone(),
        };
        recover_all(&storage).await;

        // Recording does not depend on the database: without it, only
        // events and motion detection are unavailable.
        let data_dir = config.storage.data_dir();
        let database = match Database::open(&data_dir).await {
            Ok(db) => Some(db),
            Err(e) => {
                warn!(dir = %data_dir.display(), error = %e, "cannot open the database: events and motion detection unavailable");
                None
            }
        };
        let ffmpeg_ok = detection::ffmpeg_works(&config.live.ffmpeg).await;
        if !ffmpeg_ok {
            warn!(ffmpeg = %config.live.ffmpeg, "FFmpeg cannot be started: motion detection, snapshots and H.264 live conversion unavailable");
        }
        let motion_settings: HashMap<CameraId, MotionSettings> = match &database {
            Some(db) => db
                .motion_settings()
                .await
                .unwrap_or_else(|e| {
                    warn!(error = %e, "cannot read motion detection settings");
                    HashMap::new()
                })
                .into_iter()
                .filter_map(|(id, s)| Some((CameraId::new(id).ok()?, s)))
                .collect(),
            None => HashMap::new(),
        };
        let (motion_tx, motion_rx) = mpsc::channel(256);
        let (manual_tx, manual_rx) = mpsc::channel(16);
        let detection_paused = data_dir.join(detection::PAUSED_FILE).exists();
        if detection_paused {
            warn!(
                "motion detection paused on every camera (Home Assistant or the interface resumes it)"
            );
        }
        let detection_ready = database.is_some() && ffmpeg_ok;
        let (ai, ai_error) = detection::start_ai(&config.ai, detection_ready);

        let engine = Engine {
            events: events.clone(),
            recording: std::sync::RwLock::new(config.recording.clone()),
            reconnect: config.reconnect.clone(),
            cancel: CancellationToken::new(),
            motion: std::sync::RwLock::new(MotionOptions::new(&config.motion, &config.live.ffmpeg)),
            motion_tx: std::sync::Mutex::new(detection_ready.then_some(motion_tx)),
            detection_paused: AtomicBool::new(detection_paused),
            triggers: std::sync::Mutex::new(HashMap::new()),
            api: Arc::new(OnceLock::new()),
        };
        let mut running = HashMap::new();
        for cam in config.cameras.iter().filter(|c| c.enabled) {
            running.insert(
                cam.id.clone(),
                engine.start(cam, &storage, motion_settings.get(&cam.id))?,
            );
        }
        let disabled = config.cameras.iter().filter(|c| !c.enabled).count();
        let detecting = running.values().filter(|r| r.motion.is_some()).count();
        info!(cameras = running.len(), disabled, motion_detection = detecting, destinations = storage.destinations.len() + 1, recordings = %root.display(), "NVR started");

        let api = engine.api.clone();
        let shared = Arc::new(Shared {
            engine,
            state: tokio::sync::Mutex::new(State {
                configs: config.cameras.clone(),
                running,
                storage,
                motion: motion_settings,
                paused: HashSet::new(),
                night: HashMap::new(),
            }),
            api,
            managed_cameras_file: config.managed_cameras_file.clone(),
            managed_storage_file: config.managed_storage_file.clone(),
            managed_mqtt_file: config.managed_mqtt_file.clone(),
            database: database.clone(),
            ffmpeg: config.live.ffmpeg.clone(),
            ffmpeg_ok,
            ai: std::sync::RwLock::new(ai),
            ai_settings: std::sync::Mutex::new((config.ai.clone(), ai_error)),
            managed_ai_file: config.managed_ai_file.clone(),
            updates: std::sync::Mutex::new(config.updates.clone()),
            managed_updates_file: config.managed_updates_file.clone(),
            update_lock: tokio::sync::Mutex::new(()),
            webhooks: std::sync::RwLock::new(config.webhooks.clone()),
            managed_webhooks_file: config.managed_webhooks_file.clone(),
            config_file: config.config_file.clone(),
            general: std::sync::Mutex::new(detection::GeneralSettings::from_config(config)),
            managed_settings_file: config.managed_settings_file.clone(),
            low_space: std::sync::Mutex::new(Vec::new()),
            storage_problems: std::sync::Mutex::new(Vec::new()),
            captures_root: std::sync::Mutex::new(None),
            restream: tokio::sync::Mutex::new(None),
            restream_error: std::sync::Mutex::new(None),
            data_dir: data_dir.clone(),
            manual: database.is_some().then_some(manual_tx),
            uploaders: tokio::sync::Mutex::new(HashMap::new()),
            upload_status: std::sync::Mutex::new(HashMap::new()),
            ptz: ptz::Sessions::default(),
            mqtt: tokio::sync::Mutex::new(MqttRuntime {
                managed: config
                    .managed_mqtt_file
                    .as_ref()
                    .is_some_and(|p| p.is_file()),
                config: config.mqtt.clone(),
                handle: None,
            }),
        });

        let cancel = &shared.engine.cancel;
        let mut background = vec![
            tokio::spawn(log_events(events.subscribe(), cancel.child_token())),
            tokio::spawn(run_retention(shared.clone(), cancel.child_token())),
            tokio::spawn(run_disk_monitor(shared.clone(), cancel.child_token())),
            tokio::spawn(updates::run_checks(shared.clone(), cancel.child_token())),
            tokio::spawn(webhooks::run(shared.clone(), cancel.child_token())),
            tokio::spawn(detection::run_presence_midnight(
                shared.clone(),
                cancel.child_token(),
            )),
            tokio::spawn(ptz::read_all(shared.clone())),
            tokio::spawn(night::run(shared.clone(), cancel.child_token())),
        ];
        if let Some(db) = &database {
            background.push(tokio::spawn(index::run_indexer(
                shared.clone(),
                db.clone(),
                cancel.child_token(),
            )));
        }
        // Ends once every detection has stopped (see `shutdown`).
        let detection_events = database.map(|db| {
            tokio::spawn(detection::run_events(
                shared.clone(),
                db,
                motion_rx,
                manual_rx,
            ))
        });
        detection::restart_mqtt(&shared).await;
        detection::restart_uploaders(&shared).await;
        if config.metrics.log_interval_seconds > 0 {
            let interval = Duration::from_secs(config.metrics.log_interval_seconds.into());
            background.push(tokio::spawn(log_metrics(
                shared.clone(),
                interval,
                cancel.child_token(),
            )));
        }
        let web = if config.web.enabled {
            Some(start_web(config, &shared, events).await?)
        } else {
            info!("web interface disabled");
            None
        };
        // After the web interface: it shows the port.
        restream::restart(&shared).await;
        Ok(Self {
            shared,
            background,
            detection_events,
            web,
        })
    }

    /// Address of the web interface, if enabled.
    pub fn web_addr(&self) -> Option<std::net::SocketAddr> {
        self.web.as_ref().map(ApiServer::local_addr)
    }

    /// Address of the RTSP republication, while it is served.
    pub async fn restream_addr(&self) -> Option<std::net::SocketAddr> {
        restream::addr(&self.shared).await
    }

    /// Stops every stream, waits for recorders to close their segments, and
    /// returns false if some component did not stop in time.
    pub async fn shutdown(self) -> bool {
        info!("shutting down");
        if let Some(web) = self.web {
            web.shutdown().await;
        }
        restream::stop(&self.shared).await;
        self.shared.engine.cancel.cancel();
        let running: Vec<CameraRuntime> = self
            .shared
            .state
            .lock()
            .await
            .running
            .drain()
            .map(|(_, r)| r)
            .collect();
        let mut clean = true;
        for cam in running {
            clean &= cam.stop().await;
        }
        // Every detection has stopped and reported its last event: closing
        // the channel lets the event writer finish.
        drop(
            self.shared
                .engine
                .motion_tx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(),
        );
        if let Some(task) = self.detection_events
            && tokio::time::timeout(SHUTDOWN_TIMEOUT, task).await.is_err()
        {
            warn!("event writer did not finish in time");
        }
        for task in self.background {
            let _ = task.await;
        }
        for (_, u) in self.shared.uploaders.lock().await.drain() {
            u.stop().await;
        }
        // Last: MQTT has published the final states, then says `offline`.
        if let Some(h) = self.shared.mqtt.lock().await.handle.take() {
            h.stop().await;
        }
        if let Some(db) = &self.shared.database {
            db.close().await;
        }
        info!(clean, "NVR stopped");
        clean
    }
}

/// Periods the user protected from deletion (none without database).
async fn protected_ranges(shared: &Shared) -> Protected {
    let ranges = match &shared.database {
        Some(db) => match db.protected_ranges().await {
            Ok(list) => list
                .into_iter()
                .map(|p| (p.camera, p.start_ms, p.end_ms))
                .collect(),
            Err(e) => {
                // Better delete nothing than a protected recording.
                warn!(error = %e, "cannot read protected recordings: deletion skipped");
                return Protected {
                    everything: true,
                    ..Protected::default()
                };
            }
        },
        None => Vec::new(),
    };
    // Segments end on a key frame after their nominal length: margin.
    let segment_ms = i64::from(shared.general().recording.segment_seconds) * 2_000 + 60_000;
    Protected {
        ranges,
        segment_ms,
        everything: false,
    }
}

/// Applies retention to every available destination with a policy.
async fn run_retention(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut wait = RETENTION_FIRST_RUN;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
        wait = RETENTION_INTERVAL;
        let (targets, cameras) = {
            let state = shared.state.lock().await;
            let cameras: Vec<(CameraId, RetentionPolicy)> = state
                .configs
                .iter()
                .map(|c| (c.id.clone(), RetentionPolicy::from(c.record.retention())))
                .filter(|(_, p)| p.is_active())
                .collect();
            (state.storage.all(), cameras)
        };
        let protected = Arc::new(protected_ranges(&shared).await);
        // Per-camera limits first (on every destination), then the
        // destinations' own limits.
        for (dest, _) in &targets {
            if cameras.is_empty() || !dest.is_available() {
                continue;
            }
            for (camera, policy) in &cameras {
                let dir = dest.root.join(camera.as_str());
                let policy = *policy;
                let protected = protected.clone();
                let Ok(report) = tokio::task::spawn_blocking(move || {
                    enforce_camera(&dir, &policy, SystemTime::now(), &protected)
                })
                .await
                else {
                    continue;
                };
                if report.deleted > 0 {
                    info!(%camera, destination = %dest.id, deleted = report.deleted, freed_mb = report.freed_bytes / 1_000_000, "retention: old recordings of the camera deleted");
                }
                for e in report.errors.iter().take(5) {
                    warn!(%camera, destination = %dest.id, error = %e, "retention: cannot delete");
                }
            }
        }
        for (dest, retention) in targets {
            let policy = RetentionPolicy::from(retention);
            if !policy.is_active() {
                continue;
            }
            if let Some(reason) = dest.unavailable_reason() {
                warn!(destination = %dest.id, %reason, "retention skipped: destination unavailable");
                continue;
            }
            let root = dest.root.clone();
            let protected = protected.clone();
            let Ok(report) = tokio::task::spawn_blocking(move || {
                enforce(&root, &policy, SystemTime::now(), &protected)
            })
            .await
            else {
                continue;
            };
            if report.deleted > 0 {
                info!(destination = %dest.id, deleted = report.deleted, freed_mb = report.freed_bytes / 1_000_000, kept_mb = report.kept_bytes / 1_000_000, "retention: old recordings deleted");
            }
            for e in report.errors.iter().take(5) {
                warn!(destination = %dest.id, error = %e, "retention: cannot delete");
            }
        }
        if let Some(db) = &shared.database {
            let before = SystemTime::now()
                .checked_sub(Duration::from_secs(
                    u64::from(shared.general().motion.events_retention_days) * 86_400,
                ))
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
            if let Ok(n) = db.purge_sessions(unix_now_ms()).await
                && n > 0
            {
                info!(sessions = n, "expired login sessions removed");
            }
            let audit_before = unix_now_ms()
                - i64::try_from(rustnvr_api::auth::AUDIT_DAYS * 86_400_000).unwrap_or(i64::MAX);
            if let Err(e) = db
                .purge_audit(audit_before, rustnvr_api::auth::AUDIT_LINES)
                .await
            {
                warn!(error = %e, "cannot purge the activity log");
            }
            match db.delete_events_before(before).await {
                Ok(0) => {}
                Ok(n) => info!(events = n, "retention: old events deleted"),
                Err(e) => warn!(error = %e, "retention: cannot delete old events"),
            }
            if let Err(e) = db.delete_crossings_before(before).await {
                warn!(error = %e, "retention: cannot delete old crossings");
            }
        }
    }
}

/// Camera and storage management for the API.
struct Controller(Arc<Shared>);

fn stream_from_input(input: StreamInput, previous: Option<&StreamConfig>) -> StreamConfig {
    let username = input
        .username
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty());
    // No password in the input keeps the stored one (it is never sent back).
    let password = match input.password {
        Some(p) => (!p.is_empty()).then_some(p),
        None => previous.and_then(|p| p.password.clone()),
    };
    StreamConfig {
        url: input.url.trim().to_owned(),
        password: username.as_ref().and(password),
        username,
        password_env: None,
        transport: input.transport,
        connect_timeout_seconds: previous.map_or(10, |p| p.connect_timeout_seconds),
        stall_timeout_seconds: previous.map_or(10, |p| p.stall_timeout_seconds),
    }
}

fn config_from_input(
    id: CameraId,
    input: CameraInput,
    previous: Option<&CameraConfig>,
    storage: &Storage,
    default_destination: &str,
) -> Result<CameraConfig, ControlError> {
    let name = input.name.trim().to_owned();
    if name.is_empty() {
        return Err(ControlError::Invalid("le nom est obligatoire".into()));
    }
    let mut destinations = input.record_destinations;
    destinations.dedup();
    // Recording enabled without a choice: the default destination (or the
    // local disk if it no longer exists).
    if input.record && destinations.is_empty() {
        destinations.push(if storage.destination(default_destination).is_some() {
            default_destination.to_owned()
        } else {
            LOCAL_DESTINATION.to_owned()
        });
    }
    if let Some(unknown) = destinations
        .iter()
        .find(|d| storage.destination(d).is_none())
    {
        return Err(ControlError::Invalid(format!(
            "point de stockage inconnu : {unknown:?}"
        )));
    }
    // A USB webcam, or RTSP streams.
    let usb = input.usb.map(|mut u| {
        u.device = u.device.trim().to_owned();
        u
    });
    let (main, sub) = match (&usb, input.main) {
        (Some(_), _) => (StreamConfig::default(), None),
        (None, Some(main)) => (
            stream_from_input(main, previous.map(|p| &p.main)),
            // The form has one user name and one password for both streams:
            // a sub stream added (or put back) later without typing the
            // password again takes the stored one of the main stream, when
            // it is the same account.
            input.sub.filter(|s| !s.url.trim().is_empty()).map(|s| {
                let stored = previous.and_then(|p| {
                    p.sub.as_ref().or_else(|| {
                        let same =
                            s.username.as_deref().map(str::trim) == p.main.username.as_deref();
                        same.then_some(&p.main)
                    })
                });
                let mut sub = stream_from_input(s, stored);
                // Timeouts of the main stream are not those of the sub.
                if previous.is_none_or(|p| p.sub.is_none()) {
                    let defaults = StreamConfig::default();
                    sub.connect_timeout_seconds = defaults.connect_timeout_seconds;
                    sub.stall_timeout_seconds = defaults.stall_timeout_seconds;
                }
                sub
            }),
        ),
        (None, None) => {
            return Err(ControlError::Invalid(
                "il faut l'adresse RTSP du flux principal, ou une webcam USB".into(),
            ));
        }
    };
    // Credentials of the republication for this camera: none without a user
    // name; no password in the input keeps the stored one.
    let restream = input
        .restream_username
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
        .map(|username| {
            let stored = previous.and_then(|p| p.restream.as_ref());
            let (password, password_env) = match input.restream_password {
                Some(p) => (Some(p), None),
                None => (
                    stored.and_then(|s| s.password.clone()),
                    stored.and_then(|s| s.password_env.clone()),
                ),
            };
            rustnvr_core::config::CameraRestreamConfig {
                username,
                password,
                password_env,
            }
        });
    let cam = CameraConfig {
        id,
        name: Some(name),
        enabled: input.enabled,
        main,
        sub,
        usb: usb.clone(),
        record: CameraRecordConfig {
            enabled: input.record,
            mode: input.record_mode,
            destinations,
            retention_days: input.retention_days,
            max_size_gb: input.max_size_gb,
            audio: input
                .record_audio
                .unwrap_or_else(|| previous.is_none_or(|p| p.record.audio)),
        },
        // The port from the interface; an address or a user set by hand in
        // the file is kept. Not for a webcam.
        onvif: input.onvif_port.filter(|_| usb.is_none()).map(|port| {
            let mut onvif = previous
                .and_then(|p| p.onvif.clone())
                .unwrap_or(OnvifConfig {
                    port,
                    url: None,
                    username: None,
                    password: None,
                    password_env: None,
                });
            onvif.port = port;
            onvif
        }),
        restream,
        managed: true,
    };
    let mut problems = cam.problems(|_| None);
    problems.extend(cam.record.retention().problems("rétention de la caméra"));
    if problems.is_empty() {
        Ok(cam)
    } else {
        Err(ControlError::Invalid(problems.join("; ")))
    }
}

fn internal(e: anyhow::Error) -> ControlError {
    ControlError::Internal(format!("{e:#}"))
}

/// Keeps the secrets of `previous` that the interface did not send again.
fn merge_remote_secrets(remote: &mut RemoteConfig, previous: Option<&RemoteConfig>) {
    match (remote, previous) {
        (RemoteConfig::S3(s), Some(RemoteConfig::S3(p)))
            if s.secret_access_key.is_none() && s.secret_access_key_env.is_none() =>
        {
            s.secret_access_key.clone_from(&p.secret_access_key);
            s.secret_access_key_env.clone_from(&p.secret_access_key_env);
        }
        (RemoteConfig::Sftp(s), Some(RemoteConfig::Sftp(p)))
            if s.password.is_none() && s.password_env.is_none() && s.private_key.is_none() =>
        {
            s.password.clone_from(&p.password);
            s.password_env.clone_from(&p.password_env);
            s.private_key.clone_from(&p.private_key);
        }
        _ => {}
    }
}

/// Validates a destination entered in the interface; the directory must
/// exist (and be a mounted share when `require_mount`). For S3 and SFTP the
/// directory is the local outbox, `<data_dir>/outbox/<id>` by default.
fn destination_from_input(
    id: String,
    input: DestinationInput,
    previous: Option<&DestinationConfig>,
    data_dir: &Path,
) -> Result<DestinationConfig, ControlError> {
    let mut remote = input.remote;
    if let Some(r) = &mut remote {
        merge_remote_secrets(r, previous.and_then(|p| p.remote.as_ref()));
    }
    let path = input
        .path
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .or_else(|| {
            remote
                .is_some()
                .then(|| data_dir.join("outbox").join(&id).display().to_string())
        })
        .ok_or_else(|| ControlError::Invalid("le chemin est obligatoire".into()))?;
    if remote.is_some() {
        std::fs::create_dir_all(&path)
            .map_err(|e| ControlError::Invalid(format!("dossier d'attente {path} : {e}")))?;
    }
    let d = DestinationConfig {
        id,
        name: input
            .name
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty()),
        path: PathBuf::from(path),
        // The outbox of a remote destination is local.
        require_mount: input.require_mount && remote.is_none(),
        retention_days: input.retention_days,
        max_size_gb: input.max_size_gb,
        min_free_gb: input.min_free_gb,
        remote,
        managed: true,
    };
    let problems = d.problems();
    if !problems.is_empty() {
        return Err(ControlError::Invalid(problems.join("; ")));
    }
    let check = Destination {
        id: d.id.clone(),
        name: String::new(),
        root: d.path.clone(),
        require_mount: d.require_mount,
    };
    if let Some(reason) = check.unavailable_reason() {
        return Err(ControlError::Invalid(format!(
            "{reason}. Montez d'abord le partage réseau dans Linux, ou décochez « partage réseau obligatoire » pour un second disque local."
        )));
    }
    if !is_writable(&d.path) {
        return Err(ControlError::Invalid(format!(
            "RustNVR ne peut pas écrire dans {}",
            d.path.display()
        )));
    }
    Ok(d)
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".rustnvr-write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

impl Controller {
    /// Restarts the running cameras that record to `dest_id`.
    async fn restart_users(&self, state: &mut State, dest_id: &str) -> Result<(), ControlError> {
        let users: Vec<CameraConfig> = state
            .configs
            .iter()
            .filter(|c| {
                state.runs(c)
                    && c.record.enabled
                    && c.record.destinations.iter().any(|d| d == dest_id)
            })
            .cloned()
            .collect();
        for cam in users {
            if let Some(old) = state.running.remove(&cam.id) {
                old.stop().await;
            }
            let rt = self
                .0
                .engine
                .start(&cam, &state.storage, state.motion.get(&cam.id))
                .map_err(internal)?;
            state.running.insert(cam.id.clone(), rt);
        }
        Ok(())
    }
}

impl NvrControl for Controller {
    fn create(&self, input: CameraInput) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let raw_id = input.id.clone().unwrap_or_default();
            let id = CameraId::new(raw_id).map_err(|e| ControlError::Invalid(e.to_string()))?;
            if RESERVED_IDS.contains(&id.as_str()) {
                return Err(ControlError::Invalid(format!(
                    "l'identifiant {id:?} est réservé"
                )));
            }
            let mut state = self.0.state.lock().await;
            let cam = config_from_input(
                id.clone(),
                input,
                None,
                &state.storage,
                &self.0.general().default_destination,
            )?;
            if state.configs.iter().any(|c| c.id == id) {
                return Err(ControlError::Conflict(format!(
                    "l'identifiant {id:?} est déjà utilisé"
                )));
            }
            state.configs.push(cam.clone());
            if let Err(e) = self.0.save_cameras(&state.configs) {
                state.configs.pop();
                return Err(e);
            }
            if cam.enabled {
                match self
                    .0
                    .engine
                    .start(&cam, &state.storage, state.motion.get(&cam.id))
                {
                    Ok(rt) => {
                        state.running.insert(id.clone(), rt);
                    }
                    Err(e) => {
                        state.configs.pop();
                        let _ = self.0.save_cameras(&state.configs);
                        return Err(internal(e));
                    }
                }
            }
            let endpoint = match &cam.usb {
                Some(usb) => format!("usb:{}", usb.device),
                None => cam
                    .main
                    .resolve_with(|_| None)
                    .map(|t| t.redacted())
                    .unwrap_or_default(),
            };
            info!(camera = %id, %endpoint, "camera added from the web interface");
            self.0.publish(&state);
            Ok(())
        })
    }

    fn update(
        &self,
        raw_id: String,
        input: CameraInput,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let mut state = self.0.state.lock().await;
            let index = state
                .configs
                .iter()
                .position(|c| c.id.as_str() == raw_id)
                .ok_or_else(|| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
            let previous = state.configs[index].clone();
            if !previous.managed {
                return Err(ControlError::ReadOnly(format!(
                    "la caméra {raw_id:?} est déclarée dans config.toml : modifiez ce fichier puis redémarrez RustNVR"
                )));
            }
            let cam = config_from_input(
                previous.id.clone(),
                input,
                Some(&previous),
                &state.storage,
                &self.0.general().default_destination,
            )?;
            state.configs[index] = cam.clone();
            if let Err(e) = self.0.save_cameras(&state.configs) {
                state.configs[index] = previous;
                return Err(e);
            }
            if let Some(old) = state.running.remove(&cam.id) {
                old.stop().await;
            }
            if state.runs(&cam) {
                let rt = self
                    .0
                    .engine
                    .start(&cam, &state.storage, state.motion.get(&cam.id))
                    .map_err(internal)?;
                state.running.insert(cam.id.clone(), rt);
            }
            info!(camera = %cam.id, "camera updated from the web interface");
            self.0.publish(&state);
            Ok(())
        })
    }

    fn delete(&self, raw_id: String) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let mut state = self.0.state.lock().await;
            let index = state
                .configs
                .iter()
                .position(|c| c.id.as_str() == raw_id)
                .ok_or_else(|| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
            if !state.configs[index].managed {
                return Err(ControlError::ReadOnly(format!(
                    "la caméra {raw_id:?} est déclarée dans config.toml : retirez-la de ce fichier puis redémarrez RustNVR"
                )));
            }
            let removed = state.configs.remove(index);
            if let Err(e) = self.0.save_cameras(&state.configs) {
                state.configs.insert(index, removed);
                return Err(e);
            }
            if let Some(rt) = state.running.remove(&removed.id) {
                rt.stop().await;
            }
            self.0
                .engine
                .triggers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&removed.id);
            if state.motion.remove(&removed.id).is_some()
                && let Some(db) = &self.0.database
                && let Err(e) = db.delete_motion_settings(&removed.id).await
            {
                warn!(camera = %removed.id, error = %e, "cannot delete motion detection settings");
            }
            info!(camera = %removed.id, "camera removed from the web interface (recordings kept)");
            self.0.publish(&state);
            Ok(())
        })
    }

    fn target(&self, raw_id: String) -> BoxFuture<'_, Option<(RtspTarget, SessionOptions)>> {
        Box::pin(async move {
            let state = self.0.state.lock().await;
            let cam = state.configs.iter().find(|c| c.id.as_str() == raw_id)?;
            // Nothing to probe in RTSP on a webcam.
            if cam.usb.is_some() {
                return None;
            }
            let target = cam.main.resolve().ok()?;
            Some((target, SessionOptions::from_config(&cam.main)))
        })
    }

    fn create_destination(
        &self,
        input: DestinationInput,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let id = input
                .id
                .clone()
                .map(|i| i.trim().to_owned())
                .filter(|i| !i.is_empty())
                .ok_or_else(|| ControlError::Invalid("l'identifiant est obligatoire".into()))?;
            let mut state = self.0.state.lock().await;
            if id == LOCAL_DESTINATION || state.storage.destinations.iter().any(|d| d.id == id) {
                return Err(ControlError::Conflict(format!(
                    "l'identifiant {id:?} est déjà utilisé"
                )));
            }
            let data_dir = self.0.data_dir.clone();
            let dest = tokio::task::spawn_blocking(move || {
                destination_from_input(id, input, None, &data_dir)
            })
            .await
            .map_err(|e| ControlError::Internal(e.to_string()))??;
            let mut storage = state.storage.clone();
            storage.destinations.push(dest.clone());
            self.0.save_storage(&storage)?;
            state.storage = storage;
            info!(destination = %dest.id, path = %dest.path.display(), "storage destination added from the web interface");
            self.0.publish(&state);
            drop(state);
            detection::restart_uploaders(&self.0).await;
            Ok(())
        })
    }

    fn update_destination(
        &self,
        id: String,
        input: DestinationInput,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let mut state = self.0.state.lock().await;
            let mut storage = state.storage.clone();
            let retention = RetentionConfig {
                retention_days: input.retention_days,
                max_size_gb: input.max_size_gb,
                min_free_gb: input.min_free_gb,
            };
            let problems = retention.problems("rétention");
            if !problems.is_empty() {
                return Err(ControlError::Invalid(problems.join("; ")));
            }
            if id == LOCAL_DESTINATION {
                // The local directory comes from config.toml; only its retention is editable.
                storage.local_retention = retention;
                self.0.save_storage(&storage)?;
                state.storage = storage;
                info!(?retention, "local retention changed from the web interface");
                self.0.publish(&state);
                return Ok(());
            }
            let index = storage
                .destinations
                .iter()
                .position(|d| d.id == id)
                .ok_or_else(|| ControlError::NotFound(format!("aucun point de stockage {id:?}")))?;
            if !storage.destinations[index].managed {
                return Err(ControlError::ReadOnly(format!(
                    "le point de stockage {id:?} est déclaré dans config.toml : modifiez ce fichier puis redémarrez RustNVR"
                )));
            }
            let previous = storage.destinations[index].clone();
            let id_for_check = id.clone();
            let data_dir = self.0.data_dir.clone();
            let before = previous.clone();
            let dest = tokio::task::spawn_blocking(move || {
                destination_from_input(id_for_check, input, Some(&before), &data_dir)
            })
            .await
            .map_err(|e| ControlError::Internal(e.to_string()))??;
            let location_changed =
                dest.path != previous.path || dest.require_mount != previous.require_mount;
            storage.destinations[index] = dest;
            self.0.save_storage(&storage)?;
            state.storage = storage;
            if location_changed {
                self.restart_users(&mut state, &id).await?;
            }
            info!(destination = %id, "storage destination updated from the web interface");
            self.0.publish(&state);
            drop(state);
            detection::restart_uploaders(&self.0).await;
            Ok(())
        })
    }

    fn delete_destination(&self, id: String) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            if id == LOCAL_DESTINATION {
                return Err(ControlError::ReadOnly(
                    "le disque local ne peut pas être supprimé".into(),
                ));
            }
            let mut state = self.0.state.lock().await;
            let index = state
                .storage
                .destinations
                .iter()
                .position(|d| d.id == id)
                .ok_or_else(|| ControlError::NotFound(format!("aucun point de stockage {id:?}")))?;
            if !state.storage.destinations[index].managed {
                return Err(ControlError::ReadOnly(format!(
                    "le point de stockage {id:?} est déclaré dans config.toml : retirez-le de ce fichier puis redémarrez RustNVR"
                )));
            }
            let users: Vec<String> = state
                .configs
                .iter()
                .filter(|c| c.record.destinations.contains(&id))
                .map(|c| c.name.clone().unwrap_or_else(|| c.id.to_string()))
                .collect();
            if !users.is_empty() {
                return Err(ControlError::Conflict(format!(
                    "utilisé par : {} — retirez-le d'abord de ces caméras",
                    users.join(", ")
                )));
            }
            let mut storage = state.storage.clone();
            storage.destinations.remove(index);
            self.0.save_storage(&storage)?;
            state.storage = storage;
            info!(destination = %id, "storage destination removed from the web interface (files kept)");
            self.0.publish(&state);
            drop(state);
            detection::restart_uploaders(&self.0).await;
            Ok(())
        })
    }

    fn motion(&self, raw_id: String) -> BoxFuture<'_, Result<MotionSettings, ControlError>> {
        Box::pin(async move {
            let state = self.0.state.lock().await;
            let cam = state
                .configs
                .iter()
                .find(|c| c.id.as_str() == raw_id)
                .ok_or_else(|| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
            Ok(state.motion.get(&cam.id).cloned().unwrap_or_default())
        })
    }

    fn set_motion(
        &self,
        raw_id: String,
        settings: MotionSettings,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move { detection::set_motion(&self.0, &raw_id, settings).await })
    }

    fn set_audio(&self, raw_id: String, enabled: bool) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let mut state = self.0.state.lock().await;
            let index = state
                .configs
                .iter()
                .position(|c| c.id.as_str() == raw_id)
                .ok_or_else(|| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
            let previous = state.configs[index].clone();
            if previous.record.audio == enabled {
                return Ok(());
            }
            if !previous.managed {
                return Err(ControlError::ReadOnly(format!(
                    "la caméra {raw_id:?} est déclarée dans config.toml : réglez « record = {{ audio = … }} » dans ce fichier puis redémarrez RustNVR"
                )));
            }
            state.configs[index].record.audio = enabled;
            if let Err(e) = self.0.save_cameras(&state.configs) {
                state.configs[index] = previous;
                return Err(e);
            }
            let id = previous.id.clone();
            // Off: the recorders stop writing the sound at once. On: they add
            // it from the next key frame, unless the stream was started
            // without its sound, which needs a reconnection.
            let reconnect = match state.running.get(&id) {
                Some(rt) => {
                    rt.record_audio.store(enabled, Ordering::Relaxed);
                    enabled && !rt.audio_requested
                }
                None => false,
            };
            if reconnect {
                if let Some(old) = state.running.remove(&id) {
                    old.stop().await;
                }
                let cam = state.configs[index].clone();
                let rt = self
                    .0
                    .engine
                    .start(&cam, &state.storage, state.motion.get(&id))
                    .map_err(internal)?;
                state.running.insert(id.clone(), rt);
            }
            info!(camera = %id, enabled, reconnect, "sound recording switched");
            self.0.publish(&state);
            Ok(())
        })
    }

    fn set_camera_paused(
        &self,
        raw_id: String,
        paused: bool,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let mut state = self.0.state.lock().await;
            let cam = state
                .configs
                .iter()
                .find(|c| c.id.as_str() == raw_id)
                .cloned()
                .ok_or_else(|| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
            if state.paused.contains(&cam.id) == paused {
                return Ok(());
            }
            if paused {
                state.paused.insert(cam.id.clone());
                if let Some(old) = state.running.remove(&cam.id) {
                    old.stop().await;
                }
            } else {
                state.paused.remove(&cam.id);
                if state.runs(&cam) && !state.running.contains_key(&cam.id) {
                    let rt = self
                        .0
                        .engine
                        .start(&cam, &state.storage, state.motion.get(&cam.id))
                        .map_err(internal)?;
                    state.running.insert(cam.id.clone(), rt);
                }
            }
            info!(camera = %cam.id, paused, "camera paused until restart, or resumed");
            self.0.publish(&state);
            Ok(())
        })
    }

    fn set_detection_paused(&self, paused: bool) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move { detection::set_detection_paused(&self.0, paused).await })
    }

    fn mqtt(&self) -> BoxFuture<'_, MqttView> {
        Box::pin(async move { detection::mqtt_view(&self.0).await })
    }

    fn set_mqtt(&self, input: MqttInput) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move { detection::set_mqtt(&self.0, input).await })
    }

    fn settings(&self) -> BoxFuture<'_, SettingsView> {
        Box::pin(async move { detection::settings_view(&self.0) })
    }

    fn set_settings(&self, input: SettingsInput) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move { detection::set_settings(&self.0, input).await })
    }

    fn ai(&self) -> BoxFuture<'_, rustnvr_api::AiView> {
        Box::pin(async move { detection::ai_view(&self.0) })
    }

    fn set_ai(
        &self,
        settings: rustnvr_core::config::AiConfig,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move { detection::set_ai(&self.0, settings).await })
    }

    fn ai_test(
        &self,
        camera: String,
    ) -> BoxFuture<'_, Result<rustnvr_api::AiTestResult, ControlError>> {
        Box::pin(async move { detection::ai_test(&self.0, &camera).await })
    }

    fn ai_probe(
        &self,
        input: rustnvr_api::AiProbeInput,
    ) -> BoxFuture<'_, rustnvr_api::AiProbeResult> {
        Box::pin(detection::ai_probe(input))
    }

    fn ai_download_model(
        &self,
        id: String,
    ) -> BoxFuture<'_, Result<rustnvr_api::AiModelView, ControlError>> {
        Box::pin(async move { detection::ai_download_model(&self.0, &id).await })
    }

    fn check_updates(&self) -> BoxFuture<'_, rustnvr_api::dto::UpdateStatus> {
        Box::pin(async move { updates::check(&self.0).await })
    }

    fn set_updates(
        &self,
        check: bool,
    ) -> BoxFuture<'_, Result<rustnvr_api::dto::UpdateStatus, ControlError>> {
        Box::pin(async move { updates::set(&self.0, check).await })
    }

    fn captures(&self) -> BoxFuture<'_, Result<Vec<rustnvr_api::CaptureDay>, ControlError>> {
        Box::pin(async move { replay::days(&self.0).await })
    }

    fn capture_pictures(
        &self,
        camera: String,
        day: String,
    ) -> BoxFuture<'_, Result<Vec<rustnvr_api::CapturePicture>, ControlError>> {
        Box::pin(async move { replay::pictures(&self.0, camera, day).await })
    }

    fn capture_picture(
        &self,
        camera: String,
        day: String,
        file: String,
    ) -> BoxFuture<'_, Result<Vec<u8>, ControlError>> {
        Box::pin(async move { replay::picture(&self.0, camera, day, file).await })
    }

    fn delete_captures(
        &self,
        camera: String,
        day: String,
    ) -> BoxFuture<'_, Result<usize, ControlError>> {
        Box::pin(async move { replay::delete(&self.0, camera, day).await })
    }

    fn replay(
        &self,
        camera: String,
        day: String,
        file: Option<String>,
    ) -> BoxFuture<'_, Result<rustnvr_api::ReplayResult, ControlError>> {
        Box::pin(async move { replay::replay(&self.0, camera, day, file).await })
    }

    fn presence(&self) -> BoxFuture<'_, Vec<(String, String, u64)>> {
        Box::pin(async move {
            detection::presence_counts(&self.0)
                .await
                .into_iter()
                .map(|(c, l, n)| (c.to_string(), l, n))
                .collect()
        })
    }

    fn reset_presence(
        &self,
        camera: String,
        line: String,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let id = CameraId::new(&camera)
                .map_err(|_| ControlError::NotFound(format!("caméra inconnue : {camera}")))?;
            detection::reset_presence(&self.0, &id, &line).await
        })
    }

    fn backup(&self, passphrase: String) -> BoxFuture<'_, Result<Vec<u8>, ControlError>> {
        Box::pin(async move { backups::make(&self.0, passphrase).await })
    }

    fn backup_inspect(
        &self,
        data: Vec<u8>,
        passphrase: String,
    ) -> BoxFuture<'_, Result<rustnvr_api::BackupPreview, ControlError>> {
        Box::pin(async move { backups::inspect(&self.0, data, passphrase).await })
    }

    fn backup_restore(
        &self,
        data: Vec<u8>,
        passphrase: String,
        files: Vec<String>,
        motion: bool,
    ) -> BoxFuture<'_, Result<rustnvr_api::BackupRestored, ControlError>> {
        Box::pin(async move { backups::restore(&self.0, data, passphrase, files, motion).await })
    }

    fn ptz_presets(
        &self,
        camera: String,
    ) -> BoxFuture<'_, Result<Vec<rustnvr_api::control::PtzPreset>, ControlError>> {
        Box::pin(async move { ptz::presets(&self.0, &camera).await })
    }

    fn camera_device(
        &self,
        camera: String,
        fresh: bool,
    ) -> BoxFuture<'_, Result<rustnvr_api::control::CameraDevice, ControlError>> {
        Box::pin(async move { ptz::device(&self.0, &camera, fresh).await })
    }

    fn reboot_camera(&self, camera: String) -> BoxFuture<'_, Result<String, ControlError>> {
        Box::pin(async move { ptz::reboot(&self.0, &camera).await })
    }

    fn ptz_command(
        &self,
        camera: String,
        command: rustnvr_api::control::PtzCommand,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move { ptz::command(&self.0, &camera, command).await })
    }

    fn webhooks(&self) -> BoxFuture<'_, Vec<rustnvr_core::config::WebhookConfig>> {
        Box::pin(async move {
            // The form written out: the interface shows the type of each.
            self.0
                .webhooks
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned()
                .map(rustnvr_core::config::WebhookConfig::with_kind)
                .collect()
        })
    }

    fn set_webhooks(
        &self,
        hooks: Vec<rustnvr_core::config::WebhookConfig>,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move { webhooks::set(&self.0, hooks) })
    }

    fn test_webhook(
        &self,
        hook: rustnvr_core::config::WebhookConfig,
    ) -> BoxFuture<'_, rustnvr_api::WebhookTestResult> {
        Box::pin(async move { webhooks::test(&hook).await })
    }
}

async fn start_web(
    config: &Config,
    shared: &Arc<Shared>,
    events: EventBus,
) -> anyhow::Result<ApiServer> {
    let web = &config.web;
    let addr = web.listen_addr().map_err(anyhow::Error::msg)?;
    let auth = web
        .resolve_auth(|n| std::env::var(n).ok())
        .map_err(anyhow::Error::msg)?;
    if auth.is_none() {
        warn!(
            listen = %addr,
            "web interface has no web.username: until an administrator account is created in the interface (Settings > Users), anyone who can reach this address can view recordings and manage cameras"
        );
    }
    if !web.static_dir.join("index.html").is_file() {
        warn!(dir = %web.static_dir.display(), "web interface not built: only the API is served");
    }
    let (sources, destinations) = {
        let state = shared.state.lock().await;
        (state.sources(), shared.destination_infos(&state))
    };
    let config_auth = auth.clone();
    let state = ApiState::new(
        sources,
        config.storage.recordings_dir.clone(),
        events,
        auth,
        LiveSettings::from(&config.live),
    );
    state.set_destinations(destinations);
    state.set_detection_paused(shared.engine.detection_paused.load(Ordering::Relaxed));
    if let Some(db) = &shared.database {
        state.set_database(db.clone());
        bootstrap_accounts(db, config_auth.as_ref(), &state).await;
    }
    state.set_ffmpeg_available(shared.ffmpeg_ok);
    state.set_trusted_proxies(web.proxies().map_err(anyhow::Error::msg)?);
    // Test conversion on the graphics card (live.hwaccel), in the background.
    tokio::spawn(
        state
            .clone()
            .configure_hwaccel(shared.general().live_hwaccel),
    );
    if let Some(logs) = crate::logging::buffer() {
        state.set_logs(logs);
    }
    state.set_playback_cache_dir(shared.data_dir.join("h264-cache"));
    state.set_ai_provider(shared.ai().map(|a| a.provider().to_owned()));
    state.set_low_space_percent(shared.general().low_space_percent);
    state.set_default_destination(shared.general().default_destination.clone());
    state.set_control(Arc::new(Controller(shared.clone())));
    let _ = shared.api.set(state.clone());
    // Shown before the first check (a few seconds after startup).
    state.set_update_status(rustnvr_api::dto::UpdateStatus {
        current: rustnvr_core::release_version().to_owned(),
        repository: shared
            .updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .repository
            .clone(),
        install: updates::install_kind().to_owned(),
        ..Default::default()
    });
    let tls = https_config(config, addr).await?;
    let scheme = if tls.is_some() { "https" } else { "http" };
    if tls.is_none() && !addr.ip().is_loopback() {
        warn!(listen = %addr, "web interface reachable from the network over plain HTTP: passwords and video travel unencrypted; set web.tls_self_signed = true or web.tls_cert / web.tls_key");
    }
    let server = ApiServer::start_with_tls(addr, state, web.static_dir.clone(), tls)
        .await
        .map_err(|e| {
            // Ports below 1024 are refused to an unprivileged account.
            if e.kind() == std::io::ErrorKind::PermissionDenied && addr.port() < 1024 {
                anyhow::anyhow!(
                    "cannot listen on {addr}: {e}; a port below 1024 needs the capability                      CAP_NET_BIND_SERVICE (AmbientCapabilities=CAP_NET_BIND_SERVICE in the                      systemd unit, which rustnvr-update installs), or use a port from 1024 up"
                )
            } else {
                anyhow::Error::new(e).context(format!("cannot listen on {addr}"))
            }
        })?;
    info!(url = %format!("{scheme}://{}", server.local_addr()), "web interface listening");
    Ok(server)
}

/// This machine's address on the default route (no packet is sent).
fn network_address() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // TEST-NET-1: only selects the route.
    socket
        .connect((std::net::Ipv4Addr::new(192, 0, 2, 1), 9))
        .ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// HTTPS configuration from `web.tls_cert` / `web.tls_key`, or a self-signed
/// certificate kept in `<data_dir>/tls` (`web.tls_self_signed`).
async fn https_config(
    config: &Config,
    addr: std::net::SocketAddr,
) -> anyhow::Result<Option<Arc<rustls::ServerConfig>>> {
    let web = &config.web;
    let (cert, key) = match (&web.tls_cert, &web.tls_key) {
        (Some(c), Some(k)) => (c.clone(), k.clone()),
        _ if web.tls_self_signed => {
            let dir = config.storage.data_dir().join("tls");
            let mut names = vec![
                "localhost".to_owned(),
                "127.0.0.1".to_owned(),
                "::1".to_owned(),
            ];
            if !addr.ip().is_unspecified() && !addr.ip().is_loopback() {
                names.push(addr.ip().to_string());
            } else if addr.ip().is_unspecified()
                && let Some(ip) = network_address()
            {
                // Listening everywhere (Docker): the address browsers use.
                names.push(ip.to_string());
            }
            names.extend(web.tls_names.iter().cloned());
            if let Ok(host) = std::fs::read_to_string("/etc/hostname") {
                let host = host.trim();
                if !host.is_empty() {
                    names.push(host.to_owned());
                }
            }
            tokio::task::spawn_blocking(move || rustnvr_api::tls::self_signed(&dir, names))
                .await?
                .map_err(anyhow::Error::msg)?
        }
        _ => return Ok(None),
    };
    let (tls, fingerprint) =
        rustnvr_api::tls::server_config(&cert, &key).map_err(anyhow::Error::msg)?;
    info!(cert = %cert.display(), sha256 = %fingerprint, "HTTPS enabled (compare this fingerprint with the one shown by the browser)");
    Ok(Some(tls))
}

async fn log_events(mut rx: tokio::sync::broadcast::Receiver<NvrEvent>, cancel: CancellationToken) {
    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => return,
            e = rx.recv() => e,
        };
        match event {
            Ok(NvrEvent::SegmentFinished(s)) => info!(
                camera = %s.camera,
                file = %s.path.display(),
                duration_ms = s.duration.as_millis() as u64,
                frames = s.frames,
                bytes = s.bytes,
                "segment finished"
            ),
            Ok(NvrEvent::RecorderError {
                camera,
                message,
                salvaged,
            }) => {
                warn!(%camera, %message, salvaged = salvaged.is_some(), "recording error")
            }
            // Logged where they happen.
            Ok(
                NvrEvent::StreamState { .. }
                | NvrEvent::CamerasChanged
                | NvrEvent::Detection(_)
                | NvrEvent::Snapshot { .. }
                | NvrEvent::ObjectSnapshot { .. }
                | NvrEvent::Crossing { .. }
                | NvrEvent::StorageState { .. }
                | NvrEvent::Presence { .. }
                | NvrEvent::MotionSettingsChanged { .. },
            ) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "event logger lagging");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Logs per-camera and process metrics; follows cameras added or removed at run time.
async fn log_metrics(shared: Arc<Shared>, interval: Duration, cancel: CancellationToken) {
    let mut previous: HashMap<CameraId, StreamMetricsSnapshot> = HashMap::new();
    let mut previous_proc = ProcSample::read();
    let mut previous_group = GroupSample::read();
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // the first tick is immediate
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let secs = interval.as_secs_f64();
        let mut online = 0;
        let mut seen = HashMap::new();
        let state = shared.state.lock().await;
        for (id, rt) in &state.running {
            let now = rt.stream.metrics.snapshot();
            let prev = previous.get(id).copied().unwrap_or_default();
            let streaming = rt.stream.state.borrow().is_streaming();
            online += usize::from(streaming);
            let (segments, write_errors) = rt.recorders.iter().fold((0, 0), |(s, w), r| {
                let m = r.metrics();
                (
                    s + m.segments.load(Ordering::Relaxed),
                    w + m.write_errors.load(Ordering::Relaxed),
                )
            });
            info!(
                camera = %id,
                streaming,
                fps = format!("{:.1}", now.frames.saturating_sub(prev.frames) as f64 / secs),
                kbps = (now.bytes.saturating_sub(prev.bytes) as f64 * 8.0 / secs / 1000.0) as u64,
                lost_packets = now.lost_packets.saturating_sub(prev.lost_packets),
                dropped_frames = now.dropped_frames.saturating_sub(prev.dropped_frames),
                reconnects = now.failures.saturating_sub(prev.failures),
                segments_total = segments,
                write_errors_total = write_errors,
                "camera metrics"
            );
            seen.insert(id.clone(), now);
        }
        let total = state.running.len();
        drop(state);
        previous = seen;
        let current = ProcSample::read();
        let group = GroupSample::read();
        if let (Some(now), Some(before)) = (current, previous_proc) {
            // With FFmpeg: the basis of `docker stats` in a container.
            let with_ffmpeg = group.zip(previous_group);
            info!(
                cameras_online = online,
                cameras = total,
                cpu_percent = format!("{:.1}", now.cpu_percent_since(&before)),
                rss_mib = format!("{:.1}", now.rss_bytes as f64 / 1_048_576.0),
                with_ffmpeg_cpu_percent = with_ffmpeg
                    .and_then(|(n, b)| n.cpu_percent_since(&b))
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_default(),
                with_ffmpeg_mib = group
                    .map(|g| format!("{:.1}", g.memory_bytes as f64 / 1_048_576.0))
                    .unwrap_or_default(),
                scope = group.map(|g| g.scope.as_str()).unwrap_or_default(),
                "process metrics"
            );
        }
        previous_proc = current;
        previous_group = group;
    }
}

/// Longest wait for the check of one destination (a hung network share).
const DESTINATION_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// First check after the start, once the webhooks and MQTT listen.
const DISK_FIRST_CHECK: Duration = Duration::from_secs(5);

/// What is wrong with each destination (by identifier), checked in parallel
/// off the runtime. A check still running from the previous round (a hung
/// share) is waited for again instead of being started twice.
async fn destination_problems(
    targets: &[(Destination, RetentionConfig)],
    pending: &mut HashMap<String, tokio::task::JoinHandle<Option<String>>>,
) -> HashMap<String, String> {
    let mut checks: Vec<(
        String,
        std::path::PathBuf,
        tokio::task::JoinHandle<Option<String>>,
    )> = targets
        .iter()
        .map(|(d, _)| {
            let handle = pending.remove(&d.id).unwrap_or_else(|| {
                let d = d.clone();
                tokio::task::spawn_blocking(move || d.problem())
            });
            (d.id.clone(), d.root.clone(), handle)
        })
        .collect();
    pending.clear();
    let deadline = tokio::time::Instant::now() + DESTINATION_CHECK_TIMEOUT;
    let mut out = HashMap::new();
    for (id, root, handle) in &mut checks {
        match tokio::time::timeout_at(deadline, &mut *handle).await {
            Ok(Ok(Some(problem))) => {
                out.insert(id.clone(), problem);
            }
            Ok(Ok(None) | Err(_)) => {}
            Err(_) => {
                out.insert(
                    id.clone(),
                    format!("{} ne répond pas (partage réseau bloqué ?)", root.display()),
                );
            }
        }
    }
    for (id, _, handle) in checks {
        if !handle.is_finished() {
            pending.insert(id, handle);
        }
    }
    out
}

/// Checks every destination each minute: reports those that cannot be used
/// (share not mounted, not writable, not answering) when that changes, frees
/// the reserve (`min_free_gb`) by deleting the oldest recordings, and
/// reports the destinations short of space.
async fn run_disk_monitor(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + DISK_FIRST_CHECK,
        DISK_CHECK_INTERVAL,
    );
    let mut previous: Vec<String> = Vec::new();
    let mut previous_problems: HashMap<String, String> = HashMap::new();
    let mut pending = HashMap::new();
    let mut events = shared.engine.events.subscribe();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = ticker.tick() => {}
            // A write error (typically a full disk): check now rather than
            // at the next minute, so that the reserve is freed at once.
            event = events.recv() => {
                if matches!(event, Ok(NvrEvent::RecorderError { .. })) {
                    ticker.reset_immediately();
                }
                continue;
            }
        }
        let targets = shared.state.lock().await.storage.all();
        let problems = destination_problems(&targets, &mut pending).await;
        for (dest, _) in &targets {
            let now = problems.get(&dest.id);
            if now == previous_problems.get(&dest.id) {
                continue;
            }
            match now {
                Some(problem) => warn!(destination = %dest.id, %problem, "destination unusable"),
                None => info!(destination = %dest.id, "destination usable again"),
            }
            let _ = shared.engine.events.send(NvrEvent::StorageState {
                destination: dest.id.clone(),
                name: dest.name.clone(),
                problem: now.cloned(),
            });
        }
        let report: Vec<(String, String)> = targets
            .iter()
            .filter_map(|(d, _)| Some((d.name.clone(), problems.get(&d.id)?.clone())))
            .collect();
        if let Some(api) = shared.api.get() {
            api.set_storage_problems(report.clone());
        }
        *shared
            .storage_problems
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = report;
        previous_problems = problems.clone();
        // Kept pictures: where (the default destination if it answers), and
        // pruned with the retention days of that destination.
        let default = shared.general().default_destination;
        let (root, days) = shared
            .state
            .lock()
            .await
            .storage
            .captures(&default, |id| !problems.contains_key(id));
        *shared
            .captures_root
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((root.clone(), days));
        if let Some(days) = days {
            let now = unix_now_ms();
            if let Ok(removed) =
                tokio::task::spawn_blocking(move || crate::captures::prune(&root, days, now)).await
                && removed > 0
            {
                info!(
                    days_removed = removed,
                    keep_days = days,
                    "old analysed pictures deleted"
                );
            }
        }
        // Space: only on the destinations that answer.
        let targets: Vec<_> = targets
            .into_iter()
            .filter(|(d, _)| !problems.contains_key(&d.id))
            .collect();
        let percent = shared.general().low_space_percent;
        let protected = protected_ranges(&shared).await;
        let checked = tokio::task::spawn_blocking(move || {
            let mut low = Vec::new();
            for (dest, retention) in targets {
                if !dest.is_available() {
                    continue;
                }
                let Some(mut disk) = rustnvr_api::storage::disk_usage(&dest.root) else {
                    continue;
                };
                if let Some(min_gb) = retention.min_free_gb {
                    let reserve = u64::from(min_gb) * 1_000_000_000;
                    if disk.free_bytes < reserve {
                        let report = free_space(&dest.root, reserve - disk.free_bytes, &protected);
                        warn!(destination = %dest.id, deleted = report.deleted, freed_mb = report.freed_bytes / 1_000_000, reserve_gb = min_gb, "free space below the reserve: oldest recordings deleted");
                        for e in report.errors.iter().take(5) {
                            warn!(destination = %dest.id, error = %e, "cannot delete");
                        }
                        disk = rustnvr_api::storage::disk_usage(&dest.root).unwrap_or(disk);
                    }
                }
                if rustnvr_api::storage::is_low_space(&disk, percent, retention.min_free_gb) {
                    low.push(dest.name.clone());
                }
            }
            low
        })
        .await;
        let Ok(low) = checked else { continue };
        if low != previous {
            if low.is_empty() {
                info!("storage space back to normal");
            } else {
                warn!(destinations = ?low, threshold_percent = percent, "storage short of space");
            }
            if let Some(api) = shared.api.get() {
                api.set_low_space(low.clone());
            }
            *shared.low_space.lock().unwrap_or_else(|e| e.into_inner()) = low.clone();
            previous = low;
        }
    }
}

/// Creates the administrator of `web.username` / `web.password` when the
/// database has no account yet (installations from before accounts keep their
/// credentials), and tells the API whether accounts exist.
async fn bootstrap_accounts(
    db: &Database,
    config_auth: Option<&(String, rustnvr_core::Secret)>,
    api: &ApiState,
) {
    let mut has_users = match db.users().await {
        Ok(users) => !users.is_empty(),
        Err(e) => {
            warn!(error = %e, "cannot read accounts");
            false
        }
    };
    if !has_users && let Some((user, password)) = config_auth {
        let password = password.expose().to_owned();
        match tokio::task::spawn_blocking(move || rustnvr_api::auth::hash_password(&password)).await
        {
            Ok(Ok(hash)) => match db.create_user(user, &hash, "admin", unix_now_ms()).await {
                Ok(_) => {
                    info!(user = %user, "administrator account created from web.username");
                    has_users = true;
                }
                Err(e) => warn!(error = %e, "cannot create the administrator account"),
            },
            _ => warn!("cannot hash the administrator password"),
        }
    }
    api.set_has_users(has_users);
}

fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
