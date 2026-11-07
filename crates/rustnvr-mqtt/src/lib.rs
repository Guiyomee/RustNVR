//! MQTT client of RustNVR, with Home Assistant MQTT discovery.
//!
//! Topics (`<p>` is `mqtt.topic_prefix`, `rustnvr` by default):
//!
//! | Topic | Payload | Retained |
//! |---|---|---|
//! | `<p>/availability` | `online` / `offline` (last will) | yes |
//! | `<p>/<camera>/status` | `online` / `offline` (camera connected) | yes |
//! | `<p>/<camera>/motion` | `ON` / `OFF` (event ongoing) | yes |
//! | `<p>/<camera>/motion_detection` | `ON` / `OFF` (detection enabled) | yes |
//! | `<p>/<camera>/motion_detection/set` | `ON` / `OFF` (command) | — |
//! | `<p>/<camera>/recording` | `ON` / `OFF` (continuous recording enabled) | yes |
//! | `<p>/<camera>/audio` | `ON` / `OFF` (sound recorded) | yes |
//! | `<p>/<camera>/audio/set` | `ON` / `OFF` (command) | — |
//! | `<p>/<camera>/person` | `ON` / `OFF` (person recognized, optional AI) | yes |
//! | `<p>/<camera>/snapshot` | JPEG of the last event | yes |
//! | `<p>/<camera>/<object>/snapshot` | JPEG of the last recognized object of that kind (`person`, `car`…), boxes drawn | yes |
//! | `<p>/<camera>/event` | JSON of each event change | no |
//! | `<p>/<camera>/crossing` | JSON of each crossing of a line (`line`, `direction`, `at_ms`, `today`, `object`) | no |
//! | `<p>/<camera>/line/<line>/today` | crossings of the line since midnight | yes |
//! | `<p>/<camera>/line/<line>/present` | who is inside (lines counting the presence) | yes |
//! | `<p>/<camera>/line/<line>/presence/reset` | any payload (command): count from zero again | — |
//! | `<p>/<camera>/ptz/preset/set` | name of a PTZ preset (command): the motorised camera goes there | — |
//! | `<p>/cameras_online` | number | yes |
//! | `<p>/disk_used_percent` | number (local recordings disk) | yes |
//! | `<p>/disk_low` | `ON` / `OFF` (a destination is short of space) | yes |
//! | `<p>/storage_problem` | `ON` / `OFF` (a destination cannot be used: share not mounted, not writable…) | yes |
//!
//! Messages are queued without ever blocking the NVR: when the broker is
//! unreachable the client reconnects with an increasing delay, and the whole
//! state is published again once connected.

mod discovery;

use rumqttc::{
    AsyncClient, Event, EventLoop, LastWill, MqttOptions, Packet, QoS, TlsConfiguration, Transport,
};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject as _;
use rustls::{ClientConfig, RootCertStore};
use rustnvr_core::config::MqttConfig;
use rustnvr_core::{CameraId, DetectionEvent, EventBus, NvrEvent, Secret, StreamRole};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Queued requests between the NVR and the network task.
const REQUEST_CAPACITY: usize = 1024;
/// Largest message sent (snapshots).
const MAX_OUTGOING: usize = 8 << 20;
/// Largest message accepted (commands are tiny).
const MAX_INCOMING: usize = 64 << 10;
const KEEP_ALIVE: Duration = Duration::from_secs(30);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// Period of the NVR-wide sensors.
const SYSTEM_PERIOD: Duration = Duration::from_secs(60);

/// Future returned by [`NvrView`] methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Current state of a camera, as published.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraView {
    /// Identifier.
    pub id: CameraId,
    /// Display name.
    pub name: String,
    /// Main stream connected.
    pub online: bool,
    /// Continuous recording enabled.
    pub recording: bool,
    /// Motion detection enabled.
    pub motion_enabled: bool,
    /// Motion event ongoing.
    pub motion_active: bool,
    /// Sound recorded with the video.
    pub audio_enabled: bool,
    /// Object recognition is configured: a « person » sensor is announced.
    pub person_detection: bool,
    /// Lines counting the presence: name, who is inside.
    pub presence: Vec<(String, u64)>,
    /// Names of the PTZ presets of a motorised camera (empty: no selector).
    pub ptz_presets: Vec<String>,
}

/// NVR-wide figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemView {
    /// Connected cameras.
    pub cameras_online: usize,
    /// Used fraction of the local recordings disk, in percent.
    pub disk_used_percent: Option<f32>,
    /// A storage destination is short of space.
    pub low_space: bool,
    /// A storage destination cannot be used (share not mounted, not
    /// writable, not answering).
    pub storage_problem: bool,
}

/// What the MQTT client reads from, and asks to, the NVR.
pub trait NvrView: Send + Sync + 'static {
    /// Every configured camera.
    fn cameras(&self) -> BoxFuture<'_, Vec<CameraView>>;
    /// NVR-wide figures.
    fn system(&self) -> BoxFuture<'_, SystemView>;
    /// Switches motion detection of a camera (command from MQTT).
    fn set_motion_enabled(
        &self,
        camera: CameraId,
        enabled: bool,
    ) -> BoxFuture<'_, Result<(), String>>;
    /// Switches the recording of the sound of a camera (command from MQTT).
    fn set_audio_enabled(
        &self,
        camera: CameraId,
        enabled: bool,
    ) -> BoxFuture<'_, Result<(), String>>;
    /// Starts the presence count of the line of `camera` whose topic level
    /// is `label` again from zero (command from MQTT).
    fn reset_presence(&self, camera: CameraId, label: String) -> BoxFuture<'_, Result<(), String>>;
    /// Sends a motorised camera to its preset named `name` (command from MQTT).
    fn ptz_goto(&self, camera: CameraId, name: String) -> BoxFuture<'_, Result<(), String>>;
}

/// Resolved connection settings.
#[derive(Clone, Debug)]
pub struct MqttSettings {
    /// Broker host.
    pub host: String,
    /// Broker port.
    pub port: u16,
    /// User name and password.
    pub credentials: Option<(String, Secret)>,
    /// Client identifier.
    pub client_id: String,
    /// Root of RustNVR topics.
    pub topic_prefix: String,
    /// Home Assistant discovery prefix, if discovery is enabled.
    pub discovery_prefix: Option<String>,
    /// Encrypt the connection.
    pub tls: bool,
    /// Certificate authority of the broker (PEM); system authorities otherwise.
    pub ca_file: Option<PathBuf>,
}

impl MqttSettings {
    /// Settings of an enabled `[mqtt]` section, resolving the password.
    pub fn from_config(
        c: &MqttConfig,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let password = c.resolve_password(env)?;
        let credentials = match (&c.username, password) {
            (Some(u), p) if !u.is_empty() => Some((u.clone(), p.unwrap_or_default())),
            _ => None,
        };
        Ok(Self {
            host: c.host.trim().to_owned(),
            port: c.port,
            credentials,
            client_id: c.client_id.clone(),
            topic_prefix: c.topic_prefix.clone(),
            discovery_prefix: c.discovery.then(|| c.discovery_prefix.clone()),
            tls: c.tls,
            ca_file: c.ca_file.clone(),
        })
    }
}

/// Connection state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MqttStatus {
    /// Connected to the broker.
    pub connected: bool,
    /// Last error, while not connected.
    pub error: Option<String>,
}

/// A running client.
pub struct MqttHandle {
    task: JoinHandle<()>,
    cancel: CancellationToken,
    /// Connection state.
    pub status: watch::Receiver<MqttStatus>,
}

impl MqttHandle {
    /// Publishes `offline`, disconnects and waits for the client to stop.
    pub async fn stop(self) {
        self.cancel.cancel();
        if tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .is_err()
        {
            tracing::warn!("MQTT client did not stop in time");
        }
    }
}

/// Starts the client.
pub fn spawn_mqtt(
    settings: MqttSettings,
    view: Arc<dyn NvrView>,
    events: &EventBus,
    cancel: CancellationToken,
) -> MqttHandle {
    let topics = Topics {
        prefix: settings.topic_prefix.clone(),
    };
    let mut options = MqttOptions::new(&settings.client_id, &settings.host, settings.port);
    options
        .set_keep_alive(KEEP_ALIVE)
        .set_max_packet_size(MAX_INCOMING, MAX_OUTGOING)
        .set_request_channel_capacity(REQUEST_CAPACITY)
        .set_last_will(LastWill::new(
            topics.availability(),
            "offline",
            QoS::AtLeastOnce,
            true,
        ));
    if let Some((user, password)) = &settings.credentials {
        options.set_credentials(user, password.expose());
    }
    let tls_error = if settings.tls {
        match tls_config(settings.ca_file.as_deref()) {
            Ok(config) => {
                options.set_transport(Transport::tls_with_config(TlsConfiguration::Rustls(
                    Arc::new(config),
                )));
                None
            }
            Err(e) => Some(e),
        }
    } else {
        None
    };
    let (client, eventloop) = AsyncClient::new(options, REQUEST_CAPACITY);
    let (status_tx, status) = watch::channel(MqttStatus::default());
    if let Some(error) = tls_error {
        tracing::warn!(%error, "MQTT not started: TLS configuration");
        status_tx.send_replace(MqttStatus {
            connected: false,
            error: Some(error),
        });
        let wait = cancel.clone();
        return MqttHandle {
            task: tokio::spawn(async move {
                wait.cancelled().await;
                drop(status_tx);
            }),
            cancel,
            status,
        };
    }
    let client = Client {
        client,
        topics,
        settings,
        view,
        status: status_tx,
        announced: HashSet::new(),
        presence_labels: HashMap::new(),
        ptz_announced: HashSet::new(),
        persons: HashSet::new(),
        dropped: 0,
    };
    MqttHandle {
        task: tokio::spawn(client.run(eventloop, events.subscribe(), cancel.clone())),
        cancel,
        status,
    }
}

/// TLS client configuration: the broker certificate is checked against
/// `ca_file` (PEM) if given, else against the system certificate authorities.
fn tls_config(ca_file: Option<&Path>) -> Result<ClientConfig, String> {
    let mut roots = RootCertStore::empty();
    match ca_file {
        Some(path) => {
            let certs = CertificateDer::pem_file_iter(path)
                .map_err(|e| format!("mqtt.ca_file {}: {e}", path.display()))?;
            for cert in certs {
                let cert = cert.map_err(|e| format!("mqtt.ca_file {}: {e}", path.display()))?;
                roots
                    .add(cert)
                    .map_err(|e| format!("mqtt.ca_file {}: {e}", path.display()))?;
            }
        }
        None => {
            let native = rustls_native_certs::load_native_certs();
            for cert in native.certs {
                // Unusable system certificates are skipped, as browsers do.
                let _ = roots.add(cert);
            }
        }
    }
    if roots.is_empty() {
        return Err("TLS : aucun certificat d'autorité utilisable".to_owned());
    }
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())
        .map(|b| b.with_root_certificates(roots).with_no_client_auth())
}

/// Topic names.
#[derive(Clone)]
pub(crate) struct Topics {
    prefix: String,
}

impl Topics {
    pub(crate) fn availability(&self) -> String {
        format!("{}/availability", self.prefix)
    }
    pub(crate) fn camera(&self, cam: &CameraId, what: &str) -> String {
        format!("{}/{cam}/{what}", self.prefix)
    }
    pub(crate) fn system(&self, what: &str) -> String {
        format!("{}/{what}", self.prefix)
    }
    /// Camera and line (topic level) of a
    /// `<p>/<camera>/line/<line>/presence/reset` command.
    fn presence_reset(&self, topic: &str) -> Option<(CameraId, String)> {
        let rest = topic.strip_prefix(&self.prefix)?.strip_prefix('/')?;
        let rest = rest.strip_suffix("/presence/reset")?;
        let (cam, label) = rest.split_once("/line/")?;
        if label.is_empty() || label.contains('/') {
            return None;
        }
        Some((CameraId::new(cam).ok()?, label.to_owned()))
    }

    /// Camera of a `<p>/<camera>/ptz/preset/set` command.
    fn ptz_preset(&self, topic: &str) -> Option<CameraId> {
        let rest = topic.strip_prefix(&self.prefix)?.strip_prefix('/')?;
        let cam = rest.strip_suffix("/ptz/preset/set")?;
        if cam.contains('/') {
            return None;
        }
        CameraId::new(cam).ok()
    }

    /// Camera and switch of a `<p>/<camera>/motion_detection/set` or
    /// `<p>/<camera>/audio/set` topic.
    fn command(&self, topic: &str) -> Option<(CameraId, Switch)> {
        let rest = topic.strip_prefix(&self.prefix)?.strip_prefix('/')?;
        let (cam, switch) = if let Some(cam) = rest.strip_suffix("/motion_detection/set") {
            (cam, Switch::Motion)
        } else {
            (rest.strip_suffix("/audio/set")?, Switch::Audio)
        };
        Some((CameraId::new(cam).ok()?, switch))
    }
}

/// Camera setting switched by an MQTT command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Switch {
    Motion,
    Audio,
}

fn on_off(v: bool) -> &'static str {
    if v { "ON" } else { "OFF" }
}

/// A score with 3 decimals (an `f32` would print as 0.8700000047683716).
fn round3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

/// Object kind as a topic level (`traffic light` → `traffic_light`): never
/// `/`, `+` nor `#`.
pub fn topic_label(label: &str) -> String {
    let level: String = label
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if level.is_empty() {
        "object".to_owned()
    } else {
        level
    }
}

fn event_json(e: &DetectionEvent) -> String {
    let objects: Vec<serde_json::Value> = e
        .objects
        .iter()
        .map(|o| {
            let mut v = serde_json::json!({ "label": o.label, "score": round3(o.score) });
            if let Some(b) = o.bbox {
                v["bbox"] = b.map(round3).to_vec().into();
            }
            if let Some(z) = &o.zone {
                v["zone"] = z.as_str().into();
            }
            v
        })
        .collect();
    serde_json::json!({
        "id": e.id,
        "camera": e.camera.as_str(),
        "kind": e.kind.as_str(),
        "start_ms": e.start_ms,
        "end_ms": e.end_ms,
        "score": round3(e.score),
        "zones": e.zones,
        "objects": objects,
        "ongoing": e.end_ms.is_none(),
    })
    .to_string()
}

struct Client {
    client: AsyncClient,
    topics: Topics,
    settings: MqttSettings,
    view: Arc<dyn NvrView>,
    status: watch::Sender<MqttStatus>,
    /// Cameras whose discovery and state are published.
    announced: HashSet<CameraId>,
    /// Cameras whose « person » sensor is ON.
    persons: HashSet<CameraId>,
    /// Topic levels of the presence lines announced, per camera.
    presence_labels: HashMap<CameraId, Vec<String>>,
    /// Cameras whose PTZ preset selector is announced.
    ptz_announced: HashSet<CameraId>,
    /// Messages dropped because the queue was full (logged once per burst).
    dropped: u64,
}

impl Client {
    /// Queues a message; never waits (the queue is drained by the network task).
    fn publish(&mut self, topic: String, retain: bool, payload: impl Into<Vec<u8>>) {
        if self
            .client
            .try_publish(topic, QoS::AtLeastOnce, retain, payload)
            .is_err()
        {
            if self.dropped == 0 {
                tracing::warn!("MQTT queue full: messages dropped until the broker catches up");
            }
            self.dropped += 1;
        } else {
            self.dropped = 0;
        }
    }

    fn publish_camera(&mut self, c: &CameraView) {
        if let Some(prefix) = self.settings.discovery_prefix.clone() {
            for (topic, payload) in
                discovery::camera_configs(&prefix, &self.settings.client_id, &self.topics, c)
            {
                self.publish(topic, true, payload);
            }
        }
        let t = &self.topics;
        let msgs = [
            (
                t.camera(&c.id, "status"),
                if c.online { "online" } else { "offline" },
            ),
            (t.camera(&c.id, "motion"), on_off(c.motion_active)),
            (
                t.camera(&c.id, "motion_detection"),
                on_off(c.motion_enabled),
            ),
            (t.camera(&c.id, "recording"), on_off(c.recording)),
            (t.camera(&c.id, "audio"), on_off(c.audio_enabled)),
        ];
        for (topic, payload) in msgs {
            self.publish(topic, true, payload);
        }
        if c.person_detection {
            let on = self.persons.contains(&c.id);
            let topic = self.topics.camera(&c.id, "person");
            self.publish(topic, true, on_off(on));
        }
        self.publish_presence(c);
        self.publish_ptz(c);
    }

    /// Announces the PTZ preset selector of `c` (its presets as options), or
    /// removes it when the camera has none.
    fn publish_ptz(&mut self, c: &CameraView) {
        let Some(prefix) = self.settings.discovery_prefix.clone() else {
            return;
        };
        if c.ptz_presets.is_empty() {
            if self.ptz_announced.remove(&c.id) {
                let topic = discovery::ptz_config_topic(&prefix, &self.settings.client_id, &c.id);
                self.publish(topic, true, Vec::new());
            }
            return;
        }
        let (topic, payload) =
            discovery::ptz_config(&prefix, &self.settings.client_id, &self.topics, c);
        self.publish(topic, true, payload);
        self.ptz_announced.insert(c.id.clone());
    }

    /// Announces the presence lines of `c` with their counts, and removes
    /// those that no longer count.
    fn publish_presence(&mut self, c: &CameraView) {
        let labels: Vec<String> = c.presence.iter().map(|(l, _)| topic_label(l)).collect();
        let before = self.presence_labels.remove(&c.id).unwrap_or_default();
        let prefix = self.settings.discovery_prefix.clone();
        for label in before.iter().filter(|l| !labels.contains(l)) {
            if let Some(prefix) = &prefix {
                for topic in discovery::presence_config_topics(
                    prefix,
                    &self.settings.client_id,
                    &c.id,
                    label,
                ) {
                    self.publish(topic, true, Vec::new());
                }
            }
            let topic = self.topics.camera(&c.id, &format!("line/{label}/present"));
            self.publish(topic, true, Vec::new());
        }
        for (line, present) in &c.presence {
            if let Some(prefix) = &prefix {
                for (topic, payload) in discovery::presence_configs(
                    prefix,
                    &self.settings.client_id,
                    &self.topics,
                    c,
                    line,
                ) {
                    self.publish(topic, true, payload);
                }
            }
            let topic = self
                .topics
                .camera(&c.id, &format!("line/{}/present", topic_label(line)));
            self.publish(topic, true, present.to_string());
        }
        if !labels.is_empty() {
            self.presence_labels.insert(c.id.clone(), labels);
        }
    }

    /// Removes a deleted camera: empty retained messages clear the broker.
    fn forget_camera(&mut self, id: &CameraId) {
        if self.ptz_announced.remove(id)
            && let Some(prefix) = self.settings.discovery_prefix.clone()
        {
            let topic = discovery::ptz_config_topic(&prefix, &self.settings.client_id, id);
            self.publish(topic, true, Vec::new());
        }
        for label in self.presence_labels.remove(id).unwrap_or_default() {
            if let Some(prefix) = self.settings.discovery_prefix.clone() {
                for topic in
                    discovery::presence_config_topics(&prefix, &self.settings.client_id, id, &label)
                {
                    self.publish(topic, true, Vec::new());
                }
            }
            let topic = self.topics.camera(id, &format!("line/{label}/present"));
            self.publish(topic, true, Vec::new());
        }
        if let Some(prefix) = self.settings.discovery_prefix.clone() {
            for topic in discovery::camera_config_topics(&prefix, &self.settings.client_id, id) {
                self.publish(topic, true, Vec::new());
            }
        }
        for what in [
            "status",
            "motion",
            "motion_detection",
            "recording",
            "snapshot",
            "audio",
            "person",
        ] {
            let topic = self.topics.camera(id, what);
            self.publish(topic, true, Vec::new());
        }
    }

    /// Publishes cameras added since the last call and removes deleted ones.
    /// « Person » sensor: ON while an event with a recognized person is
    /// ongoing; a person recognized after the end of its event gives a
    /// short ON then OFF (enough to trigger automations).
    fn update_person(&mut self, e: &DetectionEvent) {
        let person = e.objects.iter().any(|o| o.label == "person");
        let on = self.persons.contains(&e.camera);
        let topic = self.topics.camera(&e.camera, "person");
        match (e.end_ms.is_none(), person, on) {
            (true, true, false) => {
                self.persons.insert(e.camera.clone());
                self.publish(topic, true, "ON");
            }
            (false, _, true) => {
                self.persons.remove(&e.camera);
                self.publish(topic, true, "OFF");
            }
            (false, true, false) => {
                self.publish(topic.clone(), true, "ON");
                self.publish(topic, true, "OFF");
            }
            _ => {}
        }
    }

    async fn sync_cameras(&mut self, full: bool) {
        let cameras = self.view.cameras().await;
        let current: HashSet<CameraId> = cameras.iter().map(|c| c.id.clone()).collect();
        let gone: Vec<CameraId> = self.announced.difference(&current).cloned().collect();
        for id in gone {
            self.forget_camera(&id);
        }
        for c in &cameras {
            if full || !self.announced.contains(&c.id) {
                self.publish_camera(c);
            } else {
                // Names or recording may have changed.
                if let Some(prefix) = self.settings.discovery_prefix.clone() {
                    for (topic, payload) in discovery::camera_configs(
                        &prefix,
                        &self.settings.client_id,
                        &self.topics,
                        c,
                    ) {
                        self.publish(topic, true, payload);
                    }
                }
                let topic = self.topics.camera(&c.id, "recording");
                self.publish(topic, true, on_off(c.recording));
                let topic = self.topics.camera(&c.id, "audio");
                self.publish(topic, true, on_off(c.audio_enabled));
                self.publish_presence(c);
                self.publish_ptz(c);
            }
        }
        self.announced = current;
    }

    async fn publish_system(&mut self) {
        let s = self.view.system().await;
        let topic = self.topics.system("cameras_online");
        self.publish(topic, true, s.cameras_online.to_string());
        if let Some(p) = s.disk_used_percent {
            let topic = self.topics.system("disk_used_percent");
            self.publish(topic, true, format!("{p:.1}"));
        }
        let topic = self.topics.system("disk_low");
        self.publish(topic, true, on_off(s.low_space));
        let topic = self.topics.system("storage_problem");
        self.publish(topic, true, on_off(s.storage_problem));
    }

    async fn on_connected(&mut self) {
        tracing::info!(broker = %format!("{}:{}", self.settings.host, self.settings.port), "MQTT connected");
        self.status.send_replace(MqttStatus {
            connected: true,
            error: None,
        });
        let topic = self.topics.availability();
        self.publish(topic, true, "online");
        if let Some(prefix) = self.settings.discovery_prefix.clone() {
            for (topic, payload) in
                discovery::nvr_configs(&prefix, &self.settings.client_id, &self.topics)
            {
                self.publish(topic, true, payload);
            }
        }
        self.sync_cameras(true).await;
        self.publish_system().await;
        for command in [
            "motion_detection/set",
            "audio/set",
            "line/+/presence/reset",
            "ptz/preset/set",
        ] {
            let topic = format!("{}/+/{command}", self.topics.prefix);
            if self.client.try_subscribe(topic, QoS::AtLeastOnce).is_err() {
                tracing::warn!("MQTT: cannot subscribe to commands");
            }
        }
    }

    fn on_command(&self, topic: &str, payload: &[u8]) {
        if let Some(camera) = self.topics.ptz_preset(topic) {
            let name = String::from_utf8_lossy(payload).trim().to_owned();
            let view = self.view.clone();
            tokio::spawn(async move {
                if let Err(e) = view.ptz_goto(camera.clone(), name.clone()).await {
                    tracing::warn!(%camera, preset = %name, error = %e, "MQTT: PTZ preset failed");
                }
            });
            return;
        }
        if let Some((camera, label)) = self.topics.presence_reset(topic) {
            let view = self.view.clone();
            tokio::spawn(async move {
                if let Err(e) = view.reset_presence(camera.clone(), label.clone()).await {
                    tracing::warn!(%camera, line = %label, error = %e, "MQTT: presence reset failed");
                }
            });
            return;
        }
        let Some((camera, switch)) = self.topics.command(topic) else {
            return;
        };
        let enabled = match payload {
            b"ON" | b"on" | b"true" | b"1" => true,
            b"OFF" | b"off" | b"false" | b"0" => false,
            _ => {
                tracing::warn!(%camera, "MQTT: ignored command (expected ON or OFF)");
                return;
            }
        };
        let view = self.view.clone();
        // Applying it may take a moment: never block the network task.
        tokio::spawn(async move {
            let result = match switch {
                Switch::Motion => view.set_motion_enabled(camera.clone(), enabled).await,
                Switch::Audio => view.set_audio_enabled(camera.clone(), enabled).await,
            };
            if let Err(e) = result {
                tracing::warn!(%camera, ?switch, error = %e, "MQTT: command failed");
            }
        });
    }

    async fn on_event(&mut self, event: NvrEvent) {
        match event {
            NvrEvent::StreamState {
                camera,
                role: StreamRole::Main,
                state,
            } => {
                let topic = self.topics.camera(&camera, "status");
                let online = if state.is_streaming() {
                    "online"
                } else {
                    "offline"
                };
                self.publish(topic, true, online);
            }
            NvrEvent::Detection(e) => {
                let topic = self.topics.camera(&e.camera, "motion");
                self.publish(topic, true, on_off(e.end_ms.is_none()));
                self.update_person(&e);
                let topic = self.topics.camera(&e.camera, "event");
                self.publish(topic, false, event_json(&e));
            }
            NvrEvent::Snapshot { camera, jpeg } => {
                let topic = self.topics.camera(&camera, "snapshot");
                self.publish(topic, true, jpeg.to_vec());
            }
            NvrEvent::Crossing {
                camera,
                line,
                forward,
                at_ms,
                today,
                object,
                ..
            } => {
                let level = topic_label(&line);
                let topic = self.topics.camera(&camera, "crossing");
                let body = serde_json::json!({
                    "line": line,
                    "direction": if forward { "forward" } else { "backward" },
                    "at_ms": at_ms,
                    "today": today,
                    "object": object,
                });
                self.publish(topic, false, body.to_string());
                let topic = self.topics.camera(&camera, &format!("line/{level}/today"));
                self.publish(topic, true, today.to_string());
            }
            NvrEvent::ObjectSnapshot {
                camera,
                label,
                jpeg,
            } => {
                let topic = self
                    .topics
                    .camera(&camera, &format!("{}/snapshot", topic_label(&label)));
                self.publish(topic, true, jpeg.to_vec());
            }
            NvrEvent::MotionSettingsChanged { camera, enabled } => {
                let topic = self.topics.camera(&camera, "motion_detection");
                self.publish(topic, true, on_off(enabled));
                // Lines counting the presence may have changed.
                self.sync_cameras(false).await;
            }
            NvrEvent::CamerasChanged => self.sync_cameras(false).await,
            NvrEvent::Presence {
                camera,
                line,
                present,
            } => {
                let topic = self
                    .topics
                    .camera(&camera, &format!("line/{}/present", topic_label(&line)));
                self.publish(topic, true, present.to_string());
            }
            NvrEvent::StorageState { .. } => self.publish_system().await,
            NvrEvent::StreamState { .. }
            | NvrEvent::SegmentFinished(_)
            | NvrEvent::RecorderError { .. } => {}
        }
    }

    async fn run(
        mut self,
        mut eventloop: EventLoop,
        mut events: tokio::sync::broadcast::Receiver<NvrEvent>,
        cancel: CancellationToken,
    ) {
        let mut retry = FIRST_RETRY;
        let mut ticker = tokio::time::interval(SYSTEM_PERIOD);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                polled = eventloop.poll() => match polled {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        retry = FIRST_RETRY;
                        self.on_connected().await;
                    }
                    Ok(Event::Incoming(Packet::Publish(p))) => self.on_command(&p.topic, &p.payload),
                    Ok(_) => {}
                    Err(e) => {
                        let message = e.to_string();
                        if self.status.borrow().error.as_deref() != Some(message.as_str()) {
                            tracing::warn!(broker = %format!("{}:{}", self.settings.host, self.settings.port), error = %message, retry_in_s = retry.as_secs(), "MQTT connection failed");
                        }
                        self.status.send_replace(MqttStatus { connected: false, error: Some(message) });
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(retry) => {}
                        }
                        retry = (retry * 2).min(MAX_RETRY);
                    }
                },
                event = events.recv() => match event {
                    Ok(e) => {
                        if self.status.borrow().connected {
                            self.on_event(e).await;
                        }
                    }
                    // Missed events: publish the whole state again.
                    Err(RecvError::Lagged(_)) => {
                        if self.status.borrow().connected {
                            self.sync_cameras(true).await;
                        }
                    }
                    Err(RecvError::Closed) => break,
                },
                _ = ticker.tick() => {
                    if self.status.borrow().connected {
                        self.publish_system().await;
                    }
                }
            }
        }
        // Clean shutdown: say goodbye (the last will covers crashes).
        if self.status.borrow().connected {
            let topic = self.topics.availability();
            self.publish(topic, true, "offline");
            let _ = self.client.try_disconnect();
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                while eventloop.poll().await.is_ok() {}
            })
            .await;
        }
        self.status.send_replace(MqttStatus::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_kinds_make_safe_topic_levels() {
        assert_eq!(topic_label("person"), "person");
        assert_eq!(topic_label("Traffic Light"), "traffic_light");
        assert_eq!(topic_label("a/b+#"), "a_b__");
        assert_eq!(topic_label(" "), "object");
    }

    #[test]
    fn event_json_has_rounded_scores_and_objects() {
        let e = DetectionEvent {
            id: 1,
            camera: CameraId::new("jardin").expect("id"),
            kind: rustnvr_core::EventKind::Motion,
            start_ms: 10,
            end_ms: None,
            score: 0.060_104_17,
            zones: vec![],
            has_thumbnail: true,
            objects: vec![rustnvr_core::DetectedObject {
                label: "person".into(),
                score: 0.87,
                bbox: Some([0.25, 0.25, 0.5, 0.75]),
                zone: Some("Allée".into()),
            }],
        };
        assert_eq!(
            event_json(&e),
            r#"{"camera":"jardin","end_ms":null,"id":1,"kind":"motion","objects":[{"bbox":[0.25,0.25,0.5,0.75],"label":"person","score":0.87,"zone":"Allée"}],"ongoing":true,"score":0.06,"start_ms":10,"zones":[]}"#
        );
    }

    #[test]
    fn parses_command_topics() {
        let t = Topics {
            prefix: "home/nvr".into(),
        };
        assert_eq!(
            t.presence_reset("home/nvr/garage/line/porte/presence/reset"),
            Some((CameraId::new("garage").expect("id"), "porte".to_owned()))
        );
        assert!(
            t.presence_reset("home/nvr/garage/line//presence/reset")
                .is_none()
        );
        assert!(
            t.presence_reset("home/nvr/garage/motion_detection/set")
                .is_none()
        );
        assert_eq!(
            t.ptz_preset("home/nvr/garage/ptz/preset/set")
                .map(|c| c.to_string()),
            Some("garage".to_owned())
        );
        assert!(t.ptz_preset("home/nvr/a/b/ptz/preset/set").is_none());
        assert!(t.ptz_preset("home/nvr/garage/audio/set").is_none());
        let parse = |topic: &str| t.command(topic).map(|(c, s)| (c.to_string(), s));
        assert_eq!(
            parse("home/nvr/jardin/motion_detection/set"),
            Some(("jardin".into(), Switch::Motion))
        );
        assert_eq!(
            parse("home/nvr/jardin/audio/set"),
            Some(("jardin".into(), Switch::Audio))
        );
        assert!(parse("home/nvr/jardin/motion").is_none());
        assert!(parse("home/nvr/jardin/audio").is_none());
        assert!(parse("other/jardin/motion_detection/set").is_none());
        assert!(parse("home/nvr/a/b/motion_detection/set").is_none());
    }

    #[test]
    fn resolves_credentials() {
        let c = MqttConfig {
            enabled: true,
            username: Some("ha".into()),
            password_env: Some("PW".into()),
            ..MqttConfig::default()
        };
        let s = MqttSettings::from_config(&c, |_| Some("secret".into())).expect("settings");
        let (u, p) = s.credentials.expect("credentials");
        assert_eq!((u.as_str(), p.expose()), ("ha", "secret"));
        assert_eq!(s.discovery_prefix.as_deref(), Some("homeassistant"));
        assert!(MqttSettings::from_config(&c, |_| None).is_err());
    }
}
