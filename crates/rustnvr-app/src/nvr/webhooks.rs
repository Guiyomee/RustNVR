//! Webhooks: an HTTP POST with the event as JSON (and its picture) to the
//! configured addresses, on motion, recognized objects and end of an event.
//! Discord webhooks, which refuse that JSON, get a Discord message instead
//! (an embed, the picture attached).
//!
//! Each webhook has its own bounded queue, served by its own task: a slow or
//! unreachable address delays neither the NVR nor the other webhooks.
//! Addresses may carry tokens: only their host is logged.

use super::Shared;
use base64::Engine as _;
use bytes::Bytes;
use rustnvr_api::{ControlError, WebhookTestResult};
use rustnvr_core::config::{MANAGED_WEBHOOKS_FILE, WebhookConfig, WebhookEvent};
use rustnvr_core::{DetectionEvent, NvrEvent};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Calls waiting to be sent to one webhook, at most (more are dropped, with
/// a warning).
const QUEUE: usize = 50;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A failed call is tried once more after…
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// Events followed at the same time, at most (older ones forgotten).
const FOLLOWED: usize = 500;

/// Name of the picture attached to Discord messages.
const DISCORD_PICTURE: &str = "evenement.jpg";
/// Characters of an error answer quoted in the error, at most.
const REASON_CHARS: usize = 160;

/// What is posted to a webhook.
enum Body {
    /// RustNVR's JSON (n8n, Node-RED, Home Assistant, scripts…).
    Json(Value),
    /// A Discord message (`payload_json`) and its picture, attached.
    Discord { payload: Value, jpeg: Option<Bytes> },
}

/// One call to make.
struct Call {
    hook: WebhookConfig,
    body: Body,
}

/// Why a call failed.
#[derive(Debug)]
struct Failure {
    /// HTTP status, if the address answered.
    status: Option<u16>,
    /// In French, for the interface and the log; never the address.
    message: String,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("RustNVR/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// Posts `body` to `hook`; the HTTP status, or why it failed.
async fn post(client: &reqwest::Client, hook: &WebhookConfig, body: &Body) -> Result<u16, Failure> {
    let request = client.post(&hook.url);
    let request = match body {
        Body::Json(value) => request.json(value),
        Body::Discord {
            payload,
            jpeg: None,
        } => request.json(payload),
        Body::Discord {
            payload,
            jpeg: Some(jpeg),
        } => {
            let picture = reqwest::multipart::Part::bytes(jpeg.to_vec())
                .file_name(DISCORD_PICTURE)
                .mime_str("image/jpeg")
                .map_err(|_| Failure {
                    status: None,
                    message: "échec de l'envoi".to_owned(),
                })?;
            let form = reqwest::multipart::Form::new()
                .text("payload_json", payload.to_string())
                .part("files[0]", picture);
            request.multipart(form)
        }
    };
    let response = request.send().await.map_err(|e| Failure {
        status: None,
        // reqwest errors may quote the address: keep only what failed.
        message: if e.is_timeout() {
            "pas de réponse (délai dépassé)".to_owned()
        } else if e.is_connect() {
            "connexion impossible".to_owned()
        } else {
            "échec de l'envoi".to_owned()
        },
    })?;
    let status = response.status();
    if status.is_success() {
        return Ok(status.as_u16());
    }
    let text = response.text().await.unwrap_or_default();
    Err(Failure {
        status: Some(status.as_u16()),
        message: match reason(&text) {
            Some(why) => format!("réponse {} : {why}", status.as_u16()),
            None => format!("réponse {}", status.as_u16()),
        },
    })
}

/// Short reason quoted from an error answer: the `message` of a JSON answer
/// (Discord, most APIs), else the start of a plain text answer; nothing for
/// an empty answer or an HTML page.
fn reason(text: &str) -> Option<String> {
    let text = text.trim();
    let message = match serde_json::from_str::<Value>(text) {
        Ok(v) => v.get("message").and_then(Value::as_str)?.to_owned(),
        Err(_) if text.is_empty() || text.starts_with('<') => return None,
        Err(_) => text.to_owned(),
    };
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    let mut short: String = line.chars().take(REASON_CHARS).collect();
    if short.len() < line.len() {
        short.push('…');
    }
    Some(short)
}

/// Sends the queued calls, one at a time.
async fn send_calls(mut rx: mpsc::Receiver<Call>, cancel: CancellationToken) {
    let client = client();
    loop {
        let call = tokio::select! {
            _ = cancel.cancelled() => return,
            call = rx.recv() => match call { Some(c) => c, None => return },
        };
        let mut result = post(&client, &call.hook, &call.body).await;
        if result.is_err() {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(RETRY_AFTER) => {}
            }
            result = post(&client, &call.hook, &call.body).await;
        }
        match result {
            Ok(status) => {
                debug!(webhook = %call.hook.name, host = call.hook.host(), status, "webhook called")
            }
            Err(e) => {
                warn!(webhook = %call.hook.name, host = call.hook.host(), error = %e.message, "webhook call failed")
            }
        }
    }
}

/// What was already sent for an event.
#[derive(Default)]
struct Sent {
    motion: bool,
    objects: bool,
}

/// Whether `hook` wants the `kind` call for `event`.
fn wanted(hook: &WebhookConfig, kind: WebhookEvent, event: &DetectionEvent) -> bool {
    if !hook.enabled || !hook.events.contains(&kind) {
        return false;
    }
    if !hook.cameras.is_empty() && !hook.cameras.iter().any(|c| c == event.camera.as_str()) {
        return false;
    }
    if hook.objects.is_empty() {
        return true;
    }
    // With an object filter: only calls about one of these objects.
    kind != WebhookEvent::Motion
        && event
            .objects
            .iter()
            .any(|o| hook.objects.contains(&o.label))
}

fn kind_name(kind: WebhookEvent) -> &'static str {
    match kind {
        WebhookEvent::Motion => "motion",
        WebhookEvent::Objects => "objects",
        WebhookEvent::End => "end",
    }
}

/// JSON sent for `event`.
fn body(kind: &str, event: &DetectionEvent, camera_name: &str, image: Option<&[u8]>) -> Value {
    json!({
        "type": kind,
        "camera": event.camera.as_str(),
        "camera_name": camera_name,
        "event": {
            "id": event.id,
            "start_ms": event.start_ms,
            "end_ms": event.end_ms,
            "score": (f64::from(event.score) * 1000.0).round() / 1000.0,
            "zones": event.zones,
            "objects": event.objects,
        },
        "image": image.map(|jpeg| format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(jpeg)
        )),
        "rustnvr": env!("CARGO_PKG_VERSION"),
    })
}

/// French name of the usual object labels (the others stay in English).
fn object_name(label: &str) -> &str {
    match label {
        "person" => "Personne",
        "car" => "Voiture",
        "truck" => "Camion",
        "bus" => "Bus",
        "motorcycle" => "Moto",
        "bicycle" => "Vélo",
        "boat" => "Bateau",
        "train" => "Train",
        "airplane" => "Avion",
        "dog" => "Chien",
        "cat" => "Chat",
        "bird" => "Oiseau",
        "horse" => "Cheval",
        "sheep" => "Mouton",
        "cow" => "Vache",
        "bear" => "Ours",
        "backpack" => "Sac à dos",
        "handbag" => "Sac à main",
        "suitcase" => "Valise",
        "umbrella" => "Parapluie",
        other => other,
    }
}

/// « 45 s », « 2 min 05 s », « 1 h 02 min ».
fn duration_text(ms: i64) -> String {
    let s = (ms.max(0) + 500) / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {:02} s", s / 60, s % 60),
        _ => format!("{} h {:02} min", s / 3600, s % 3600 / 60),
    }
}

/// Discord message (`payload_json`) for `event`; the embed shows the
/// attached picture when `with_picture`.
fn discord_payload(
    kind: &str,
    event: &DetectionEvent,
    camera_name: &str,
    with_picture: bool,
) -> Value {
    let objects = event
        .objects
        .iter()
        .map(|o| {
            format!(
                "{} {:.0} %",
                object_name(&o.label),
                f64::from(o.score) * 100.0
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let (title, color, at_ms) = match kind {
        "motion" => (
            format!("Mouvement · {camera_name}"),
            0xF5_9E_0B,
            event.start_ms,
        ),
        "objects" => {
            let first = event
                .objects
                .first()
                .map_or("Objet", |o| object_name(&o.label));
            (
                format!("{first} · {camera_name}"),
                0x3B_82_F6,
                event.start_ms,
            )
        }
        "end" => (
            format!("Fin de l'événement · {camera_name}"),
            0x6B_72_80,
            event.end_ms.unwrap_or(event.start_ms),
        ),
        _ => ("Essai RustNVR".to_owned(), 0x22_C5_5E, event.start_ms),
    };
    let mut lines = Vec::new();
    if kind == "test" {
        lines.push("Le webhook fonctionne : les événements arriveront ici.".to_owned());
    }
    if !objects.is_empty() {
        lines.push(objects);
    }
    if !event.zones.is_empty() {
        lines.push(format!("Zones : {}", event.zones.join(", ")));
    }
    if let (Some(end), "end") = (event.end_ms, kind) {
        lines.push(format!("Durée : {}", duration_text(end - event.start_ms)));
    }
    let footer = if kind == "test" {
        format!("RustNVR {}", env!("CARGO_PKG_VERSION"))
    } else {
        format!(
            "RustNVR {} · événement {}",
            env!("CARGO_PKG_VERSION"),
            event.id
        )
    };
    let mut embed = json!({
        "title": title,
        "color": color,
        "footer": { "text": footer },
    });
    if !lines.is_empty() {
        embed["description"] = lines.join("\n").into();
    }
    if let Ok(at) = jiff::Timestamp::from_millisecond(at_ms) {
        embed["timestamp"] = at.to_string().into();
    }
    let mut payload = json!({ "embeds": [embed] });
    if with_picture {
        payload["embeds"][0]["image"] = json!({ "url": format!("attachment://{DISCORD_PICTURE}") });
        payload["attachments"] = json!([{ "id": 0, "filename": DISCORD_PICTURE }]);
    }
    payload
}

/// What to post to `hook` for `event`: a Discord message for a Discord
/// address, RustNVR's JSON otherwise.
fn request(
    hook: &WebhookConfig,
    kind: &str,
    event: &DetectionEvent,
    camera_name: &str,
    jpeg: Option<&Bytes>,
) -> Body {
    if hook.is_discord() {
        Body::Discord {
            payload: discord_payload(kind, event, camera_name, jpeg.is_some()),
            jpeg: jpeg.cloned(),
        }
    } else {
        Body::Json(body(kind, event, camera_name, jpeg.map(|b| &b[..])))
    }
}

/// Picture of the event (with the boxes of the objects for `objects`).
async fn picture(shared: &Shared, kind: WebhookEvent, event: &DetectionEvent) -> Option<Vec<u8>> {
    let db = shared.database.as_ref()?;
    let path = db.thumbnail_path(event.id).await.ok().flatten()?;
    let jpeg = tokio::fs::read(path).await.ok()?;
    if kind != WebhookEvent::Objects {
        return Some(jpeg);
    }
    let boxes: Vec<[f32; 4]> = event.objects.iter().filter_map(|o| o.bbox).collect();
    match rustnvr_motion::boxed_jpeg(&shared.ffmpeg, jpeg.clone().into(), &boxes).await {
        Ok(boxed) => Some(boxed.to_vec()),
        Err(_) => Some(jpeg),
    }
}

/// Follows the detection events and queues the calls.
pub(super) async fn run(shared: Arc<Shared>, cancel: CancellationToken) {
    // One queue and one sending task per webhook (name and address).
    let key = |h: &WebhookConfig| format!("{}\n{}", h.name, h.url);
    let mut queues: HashMap<String, mpsc::Sender<Call>> = HashMap::new();
    let mut events = shared.engine.events.subscribe();
    let mut sent: HashMap<i64, Sent> = HashMap::new();
    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => return,
            e = events.recv() => match e {
                Ok(NvrEvent::Detection(e)) => e,
                Ok(_) => continue,
                Err(RecvError::Lagged(n)) => {
                    warn!(skipped = n, "webhooks lagging behind the events");
                    continue;
                }
                Err(RecvError::Closed) => return,
            },
        };
        let hooks = shared
            .webhooks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        // Webhooks removed or changed: their task ends with their queue.
        queues.retain(|k, _| hooks.iter().any(|h| key(h) == *k));
        if hooks.iter().all(|h| !h.enabled) {
            continue;
        }
        if sent.len() > FOLLOWED {
            sent.clear();
        }
        let state = sent.entry(event.id).or_default();
        let mut kinds = Vec::new();
        if event.has_thumbnail && !state.motion {
            state.motion = true;
            kinds.push(WebhookEvent::Motion);
        }
        if !event.objects.is_empty() && !state.objects {
            state.objects = true;
            kinds.push(WebhookEvent::Objects);
        }
        if event.end_ms.is_some() {
            sent.remove(&event.id);
            kinds.push(WebhookEvent::End);
        }
        if kinds.is_empty() {
            continue;
        }
        let camera_name = shared
            .state
            .lock()
            .await
            .configs
            .iter()
            .find(|c| c.id == event.camera)
            .and_then(|c| c.name.clone())
            .unwrap_or_else(|| event.camera.as_str().to_owned());
        for kind in kinds {
            let targets: Vec<&WebhookConfig> =
                hooks.iter().filter(|h| wanted(h, kind, &event)).collect();
            if targets.is_empty() {
                continue;
            }
            let image = if kind != WebhookEvent::End && targets.iter().any(|h| h.image) {
                picture(&shared, kind, &event).await.map(Bytes::from)
            } else {
                None
            };
            for hook in targets {
                let jpeg = image.as_ref().filter(|_| hook.image);
                let call = Call {
                    hook: hook.clone(),
                    body: request(hook, kind_name(kind), &event, &camera_name, jpeg),
                };
                let tx = queues.entry(key(hook)).or_insert_with(|| {
                    let (tx, rx) = mpsc::channel(QUEUE);
                    tokio::spawn(send_calls(rx, cancel.child_token()));
                    tx
                });
                if tx.try_send(call).is_err() {
                    warn!(webhook = %hook.name, "webhook queue full: call dropped");
                }
            }
        }
    }
}

/// Calls `hook` once with a sample event, for the « Essayer » button.
pub(super) async fn test(hook: &WebhookConfig) -> WebhookTestResult {
    let sample = DetectionEvent {
        id: 0,
        camera: rustnvr_core::CameraId::new("test").expect("valid identifier"),
        kind: rustnvr_core::EventKind::Motion,
        start_ms: jiff::Timestamp::now().as_millisecond(),
        end_ms: None,
        score: 0.5,
        zones: Vec::new(),
        has_thumbnail: false,
        objects: Vec::new(),
    };
    let started = Instant::now();
    let result = post(
        &client(),
        hook,
        &request(hook, "test", &sample, "Essai RustNVR", None),
    )
    .await;
    let ms = started.elapsed().as_millis() as u64;
    info!(webhook = %hook.name, host = hook.host(), ok = result.is_ok(), ms, "webhook tried");
    match result {
        Ok(status) => WebhookTestResult {
            status: Some(status),
            ms,
            error: None,
        },
        Err(e) => WebhookTestResult {
            status: e.status,
            ms,
            error: Some(e.message),
        },
    }
}

/// Stores and applies the webhooks.
pub(super) fn set(shared: &Shared, hooks: Vec<WebhookConfig>) -> Result<(), ControlError> {
    let path = shared.managed_webhooks_file.clone().ok_or_else(|| {
        ControlError::Internal("no configuration file to store settings next to".into())
    })?;
    save(&path, &hooks)
        .map_err(|e| ControlError::Internal(format!("cannot write {}: {e}", path.display())))?;
    info!(
        webhooks = hooks.len(),
        enabled = hooks.iter().filter(|h| h.enabled).count(),
        "webhooks changed"
    );
    *shared.webhooks.write().unwrap_or_else(|e| e.into_inner()) = hooks;
    Ok(())
}

const HEADER: &str = "# Webhooks saisis dans l'interface web de RustNVR (fichier géré\n\
# automatiquement ; les adresses peuvent contenir un jeton, gardez-le privé).\n\
# Ils remplacent les sections [[webhooks]] de config.toml.\n\n";

#[derive(Serialize)]
struct StoredFile<'a> {
    webhooks: &'a [WebhookConfig],
}

/// Writes `hooks` to `path`, atomically, readable by its owner only.
fn save(path: &Path, hooks: &[WebhookConfig]) -> io::Result<()> {
    let body = toml::to_string(&StoredFile { webhooks: hooks }).map_err(io::Error::other)?;
    let tmp = path.with_file_name(format!("{MANAGED_WEBHOOKS_FILE}.tmp"));
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(HEADER.as_bytes())?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::config::Config;
    use rustnvr_core::{CameraId, DetectedObject, EventKind};

    fn hook() -> WebhookConfig {
        WebhookConfig {
            name: "n8n".into(),
            url: "http://127.0.0.1:9/hook".into(),
            enabled: true,
            cameras: Vec::new(),
            objects: Vec::new(),
            events: vec![
                WebhookEvent::Motion,
                WebhookEvent::Objects,
                WebhookEvent::End,
            ],
            image: true,
        }
    }

    fn event(camera: &str, labels: &[&str]) -> DetectionEvent {
        DetectionEvent {
            id: 1,
            camera: CameraId::new(camera).expect("id"),
            kind: EventKind::Motion,
            start_ms: 1,
            end_ms: None,
            score: 0.3,
            zones: Vec::new(),
            has_thumbnail: true,
            objects: labels
                .iter()
                .map(|l| DetectedObject {
                    label: (*l).into(),
                    score: 0.9,
                    bbox: None,
                    zone: None,
                })
                .collect(),
        }
    }

    #[test]
    fn filters_by_camera_object_and_kind() {
        let all = hook();
        assert!(wanted(&all, WebhookEvent::Motion, &event("jardin", &[])));
        let jardin = WebhookConfig {
            cameras: vec!["jardin".into()],
            ..hook()
        };
        assert!(!wanted(
            &jardin,
            WebhookEvent::Motion,
            &event("entree", &[])
        ));
        let persons = WebhookConfig {
            objects: vec!["person".into()],
            ..hook()
        };
        // An object filter skips plain motion, keeps the matching objects.
        assert!(!wanted(
            &persons,
            WebhookEvent::Motion,
            &event("jardin", &["person"])
        ));
        assert!(wanted(
            &persons,
            WebhookEvent::Objects,
            &event("jardin", &["person"])
        ));
        assert!(!wanted(
            &persons,
            WebhookEvent::Objects,
            &event("jardin", &["car"])
        ));
        let off = WebhookConfig {
            enabled: false,
            ..hook()
        };
        assert!(!wanted(&off, WebhookEvent::Motion, &event("jardin", &[])));
        let b = body(
            "objects",
            &event("jardin", &["person"]),
            "Jardin",
            Some(b"\xff\xd8"),
        );
        assert_eq!(b["camera_name"], "Jardin");
        assert_eq!(b["event"]["objects"][0]["label"], "person");
        assert_eq!(b["image"], "data:image/jpeg;base64,/9g=");
    }

    #[test]
    fn discord_messages() {
        let discord = WebhookConfig {
            url: "https://discord.com/api/webhooks/1/token".into(),
            ..hook()
        };
        let mut e = event("jardin", &["person", "car"]);
        e.zones = vec!["Allée".into()];
        let jpeg = Bytes::from_static(b"\xff\xd8");
        let Body::Discord {
            payload,
            jpeg: sent,
        } = request(&discord, "objects", &e, "Jardin", Some(&jpeg))
        else {
            panic!("Discord address: Discord message expected");
        };
        assert_eq!(sent.as_deref(), Some(&b"\xff\xd8"[..]));
        let embed = &payload["embeds"][0];
        assert_eq!(embed["title"], "Personne · Jardin");
        assert_eq!(
            embed["description"],
            "Personne 90 % · Voiture 90 %\nZones : Allée"
        );
        assert_eq!(embed["image"]["url"], "attachment://evenement.jpg");
        assert_eq!(payload["attachments"][0]["filename"], "evenement.jpg");
        assert_eq!(embed["timestamp"], "1970-01-01T00:00:00.001Z");
        // Without picture: no attachment announced.
        let Body::Discord {
            payload,
            jpeg: None,
        } = request(&discord, "motion", &e, "Jardin", None)
        else {
            panic!("Discord message without picture expected");
        };
        assert!(payload.get("attachments").is_none());
        assert_eq!(payload["embeds"][0]["title"], "Mouvement · Jardin");
        e.end_ms = Some(125_001);
        let end = discord_payload("end", &e, "Jardin", false);
        assert!(
            end["embeds"][0]["description"]
                .as_str()
                .is_some_and(|d| d.ends_with("Durée : 2 min 05 s"))
        );
        // Any other address keeps RustNVR's JSON.
        assert!(matches!(
            request(&hook(), "motion", &e, "Jardin", None),
            Body::Json(_)
        ));
    }

    #[test]
    fn quotes_the_reason_of_error_answers() {
        assert_eq!(
            reason(r#"{"message": "Cannot send an empty message", "code": 50006}"#).as_deref(),
            Some("Cannot send an empty message")
        );
        assert_eq!(reason("  bad\n token ").as_deref(), Some("bad token"));
        assert_eq!(reason(""), None);
        assert_eq!(reason("<html><body>Bad Request</body></html>"), None);
        assert_eq!(reason(r#"{"error": "x"}"#), None);
        let long = reason(&"é".repeat(500)).expect("reason");
        assert_eq!(long.chars().count(), REASON_CHARS + 1);
        assert_eq!(duration_text(44_600), "45 s");
        assert_eq!(duration_text(3_725_000), "1 h 02 min");
    }

    /// Serves one request on a local port, answers `answer`; the request.
    async fn one_request(status: &str, json: &str) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let answer = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
            json.len()
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!(
            "http://{}/api/webhooks/1/token",
            listener.local_addr().expect("addr")
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.expect("read");
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            socket.write_all(answer.as_bytes()).await.expect("write");
            request
        });
        (url, server)
    }

    #[tokio::test]
    async fn posts_discord_messages_with_the_picture_attached() {
        let (url, server) = one_request(
            "400 Bad Request",
            r#"{"message": "Cannot send an empty message", "code": 50006}"#,
        )
        .await;
        let hook = WebhookConfig { url, ..hook() };
        let body = Body::Discord {
            payload: discord_payload("motion", &event("jardin", &[]), "Jardin", true),
            jpeg: Some(Bytes::from_static(b"\xff\xd8JPEG")),
        };
        let failure = post(&client(), &hook, &body)
            .await
            .expect_err("400 answered");
        assert_eq!(failure.status, Some(400));
        assert_eq!(
            failure.message,
            "réponse 400 : Cannot send an empty message"
        );
        let request = String::from_utf8_lossy(&server.await.expect("server")).into_owned();
        assert!(request.contains("multipart/form-data; boundary="));
        assert!(request.contains("name=\"payload_json\""));
        assert!(request.contains("\"title\":\"Mouvement · Jardin\""));
        assert!(request.contains("name=\"files[0]\"; filename=\"evenement.jpg\""));
        assert!(request.contains("Content-Type: image/jpeg"));
        assert!(request.contains("JPEG"));
    }

    #[test]
    fn saved_webhooks_load_back() {
        let dir = tempfile::tempdir().expect("tmp");
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").expect("config");
        let hooks = vec![hook()];
        save(&dir.path().join(MANAGED_WEBHOOKS_FILE), &hooks).expect("save");
        let loaded = Config::load_with_env(&config_path, |_| None).expect("load");
        assert_eq!(loaded.webhooks, hooks);
    }
}
