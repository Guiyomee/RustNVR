//! Motion detection events (database, thumbnails, notifications), detection
//! settings and the MQTT client.

use super::{Controller, Shared, State};
use crate::captures::Capture;
use crate::{ai_store, mqtt_store};
use bytes::Bytes;
use rustnvr_api::{
    AiModelView, AiProbeInput, AiProbeResult, AiTestObject, AiTestResult, AiView, ControlError,
    LiveSettings, MqttInput, MqttView, NvrControl as _, SettingsInput, SettingsView,
};
use rustnvr_core::config::{
    AiConfig, Config, HwAccelMode, MANAGED_SETTINGS_FILE, ManagedSettings, MotionConfig,
    MqttConfig, RecordMode, RecordingConfig, RemoteConfig, RestreamConfig,
};
use rustnvr_core::{
    CameraId, DetectedObject, DetectionEvent, EventKind, MotionSettings, NvrEvent, Secret,
};
use rustnvr_motion::MotionOptions;
use rustnvr_motion::{MotionUpdate, boxed_jpeg, snapshot_jpeg};
use rustnvr_mqtt::{
    BoxFuture, CameraView, MqttHandle, MqttSettings, NvrView, SystemView, spawn_mqtt,
};
use rustnvr_remote::{RemoteTarget, S3Target, SftpTarget, spawn_uploader};
use rustnvr_storage::Database;
use rustnvr_stream::LiveTap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Delay between the start of an event and its thumbnail, so that the
/// moving object is in the picture rather than just entering it.
const THUMBNAIL_DELAY: Duration = Duration::from_secs(1);
/// Width of thumbnails (also sent over MQTT).
const THUMBNAIL_WIDTH: u32 = 640;

/// MQTT settings in effect and the running client.
pub(super) struct MqttRuntime {
    pub(super) config: MqttConfig,
    /// Settings come from `mqtt.ui.toml`.
    pub(super) managed: bool,
    pub(super) handle: Option<MqttHandle>,
}

/// Whether `ffmpeg -version` runs.
pub(super) async fn ffmpeg_works(ffmpeg: &str) -> bool {
    let run = tokio::process::Command::new(ffmpeg)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    matches!(
        tokio::time::timeout(Duration::from_secs(10), run).await,
        Ok(Ok(s)) if s.success()
    )
}

/// File of the data directory present while motion detection is paused.
pub(super) const PAUSED_FILE: &str = "detection-paused";

/// Width of the pictures taken on command (Home Assistant button).
const CAPTURE_WIDTH: u32 = 1920;

/// A manual event asked for (Home Assistant button, MQTT command).
pub(super) struct Manual {
    pub(super) camera: CameraId,
    pub(super) seconds: u32,
    /// Its identifier, or why it was not created.
    pub(super) reply: tokio::sync::oneshot::Sender<Result<i64, String>>,
}

/// Background work finished on an event (announced by [`run_events`]).
enum EventUpdate {
    /// The thumbnail is stored.
    Thumbnail { camera: CameraId, id: i64 },
    /// Objects were recognized in the thumbnail (and stored).
    Objects {
        camera: CameraId,
        id: i64,
        objects: Vec<DetectedObject>,
    },
}

/// Writes detections to the database and announces them, until every
/// detection has stopped (the channel closes).
pub(super) async fn run_events(
    shared: Arc<Shared>,
    db: Database,
    mut updates: mpsc::Receiver<MotionUpdate>,
    mut manual: mpsc::Receiver<Manual>,
) {
    let (thumb_tx, mut thumbs) = mpsc::channel::<EventUpdate>(16);
    let mut ongoing: HashMap<CameraId, DetectionEvent> = HashMap::new();
    // Manual events: their timers end them here.
    let (end_tx, mut ends) = mpsc::channel::<(CameraId, i64)>(16);
    let mut open_manual: HashMap<i64, CameraId> = HashMap::new();
    // Until when each camera records for its manual events (milliseconds).
    let mut manual_until: HashMap<CameraId, i64> = HashMap::new();
    let post_roll_ms = |shared: &Shared| {
        i64::try_from(shared.engine.recording().post_roll().as_millis()).unwrap_or(0)
    };
    let announce = |e: &DetectionEvent| {
        let _ = shared.engine.events.send(NvrEvent::Detection(e.clone()));
    };
    loop {
        tokio::select! {
            update = updates.recv() => {
                let Some(update) = update else { break };
                match update {
                    MotionUpdate::Started { camera, start_ms, score, zones } => {
                        // Recording first: the database must not delay it.
                        if let Some(t) = shared.engine.trigger(&camera) {
                            t.hold();
                        }
                        match db.start_event(&camera, EventKind::Motion, start_ms, score, &zones).await {
                            Ok(e) => {
                                info!(%camera, id = e.id, zones = ?e.zones, "motion started");
                                announce(&e);
                                tokio::spawn(take_thumbnail(shared.clone(), db.clone(), e.clone(), thumb_tx.clone()));
                                ongoing.insert(camera, e);
                            }
                            Err(err) => warn!(%camera, error = %err, "cannot record motion event"),
                        }
                    }
                    MotionUpdate::Zones { camera, zones } => {
                        if let Some(e) = ongoing.get_mut(&camera) {
                            e.zones = zones;
                            if let Err(err) = db.update_event(e).await {
                                warn!(%camera, error = %err, "cannot update motion event");
                            }
                            announce(e);
                        }
                    }
                    MotionUpdate::Ended { camera, end_ms, score, zones } => {
                        if let Some(t) = shared.engine.trigger(&camera) {
                            let post = post_roll_ms(&shared);
                            t.release_at(end_ms.saturating_add(post));
                            // A manual event still running keeps recording.
                            if let Some(until) = manual_until.get(&camera) {
                                t.extend_to(until.saturating_add(post));
                            }
                        }
                        if let Some(mut e) = ongoing.remove(&camera) {
                            e.end_ms = Some(end_ms.max(e.start_ms));
                            e.score = score;
                            e.zones = zones;
                            if let Err(err) = db.update_event(&e).await {
                                warn!(%camera, error = %err, "cannot update motion event");
                            }
                            info!(%camera, id = e.id, duration_s = (end_ms - e.start_ms) / 1000, score = format!("{:.3}", e.score), "motion ended");
                            announce(&e);
                        }
                    }
                    MotionUpdate::Crossing { camera, line, forward, at_ms } => {
                        // A picture may be analysed first: off this loop.
                        tokio::spawn(crossing(shared.clone(), db.clone(), camera, line, forward, at_ms));
                    }
                }
            }
            Some(Manual { camera, seconds, reply }) = manual.recv() => {
                let start_ms = jiff::Timestamp::now().as_millisecond();
                let until = start_ms.saturating_add(i64::from(seconds) * 1000);
                if let Some(t) = shared.engine.trigger(&camera) {
                    t.extend_to(until.saturating_add(post_roll_ms(&shared)));
                }
                let latest = manual_until.entry(camera.clone()).or_insert(until);
                *latest = (*latest).max(until);
                match db.start_event(&camera, EventKind::Manual, start_ms, 0.0, &[]).await {
                    Ok(e) => {
                        info!(%camera, id = e.id, seconds, "manual event started");
                        announce(&e);
                        tokio::spawn(take_thumbnail(shared.clone(), db.clone(), e.clone(), thumb_tx.clone()));
                        open_manual.insert(e.id, camera.clone());
                        let end_tx = end_tx.clone();
                        let id = e.id;
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(seconds.into())).await;
                            let _ = end_tx.send((camera, id)).await;
                        });
                        let _ = reply.send(Ok(id));
                    }
                    Err(err) => {
                        warn!(%camera, error = %err, "cannot record manual event");
                        let _ = reply.send(Err(err.to_string()));
                    }
                }
            }
            Some((camera, id)) = ends.recv() => {
                let now = jiff::Timestamp::now().as_millisecond();
                if manual_until.get(&camera).is_some_and(|until| *until <= now) {
                    manual_until.remove(&camera);
                }
                if open_manual.remove(&id).is_some() {
                    end_manual(&db, id, now, &announce).await;
                }
            }
            Some(update) = thumbs.recv() => {
                // Announced from here so that it never overtakes the end of the event.
                let (camera, id, objects) = match update {
                    EventUpdate::Thumbnail { camera, id } => (camera, id, None),
                    EventUpdate::Objects { camera, id, objects } => (camera, id, Some(objects)),
                };
                let current = match ongoing.get_mut(&camera).filter(|e| e.id == id) {
                    Some(e) => {
                        e.has_thumbnail = true;
                        if let Some(objects) = objects {
                            e.objects = objects;
                        }
                        Some(e.clone())
                    }
                    None => db.event(id).await.ok().flatten(),
                };
                if let Some(e) = current {
                    announce(&e);
                }
            }
        }
    }
    // Stopping: the manual events end now.
    let now = jiff::Timestamp::now().as_millisecond();
    for id in open_manual.into_keys() {
        end_manual(&db, id, now, &announce).await;
    }
}

/// Ends manual event `id` at `end_ms` (read back: its thumbnail and objects
/// may have come meanwhile).
async fn end_manual(db: &Database, id: i64, end_ms: i64, announce: &impl Fn(&DetectionEvent)) {
    match db.event(id).await {
        Ok(Some(mut e)) => {
            e.end_ms = Some(end_ms.max(e.start_ms));
            if let Err(err) = db.update_event(&e).await {
                warn!(camera = %e.camera, error = %err, "cannot end manual event");
            }
            info!(camera = %e.camera, id, "manual event ended");
            announce(&e);
        }
        Ok(None) => {}
        Err(err) => warn!(id, error = %err, "cannot end manual event"),
    }
}

/// Pauses motion detection on every camera, or resumes it; kept across
/// restarts (a file in the data directory). The settings of each camera do
/// not change.
pub(super) async fn set_detection_paused(
    shared: &Arc<Shared>,
    paused: bool,
) -> Result<(), ControlError> {
    let path = shared.data_dir.join(PAUSED_FILE);
    let stored = if paused {
        tokio::fs::write(&path, b"paused\n").await
    } else {
        match tokio::fs::remove_file(&path).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    };
    stored.map_err(|e| ControlError::Internal(format!("{}: {e}", path.display())))?;
    let mut guard = shared.state.lock().await;
    let state: &mut State = &mut guard;
    if shared
        .engine
        .detection_paused
        .swap(paused, std::sync::atomic::Ordering::Relaxed)
        == paused
    {
        return Ok(());
    }
    // Stopping a detection ends its ongoing event.
    let ids: Vec<CameraId> = state.running.keys().cloned().collect();
    for id in ids {
        let settings = state.motion.get(&id).cloned();
        if let Some(rt) = state.running.get_mut(&id) {
            if let Some(old) = rt.motion.take() {
                old.stop().await;
            }
            shared.engine.start_motion(rt, settings.as_ref());
        }
    }
    if paused {
        warn!("motion detection paused on every camera");
    } else {
        info!("motion detection resumed");
    }
    shared.publish(state);
    Ok(())
}

/// Distance (fraction of the picture) from a line within which a recognized
/// object is taken as the one crossing it.
const AT_LINE: f32 = 0.25;

/// Distance from `p` to the segment `a`–`b` (fractions of the picture).
fn distance_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// Whether `o` is at `line`: the bottom middle (feet) or the centre of its
/// box is near the line. An object without a box counts.
pub(super) fn at_line(o: &DetectedObject, line: &rustnvr_core::CrossingLine) -> bool {
    let Some([x1, y1, x2, y2]) = o.bbox else {
        return true;
    };
    let x = (x1 + x2) / 2.0;
    [[x, y2], [x, (y1 + y2) / 2.0]]
        .into_iter()
        .any(|p| distance_to_segment(p, line.a, line.b) <= AT_LINE)
}

/// Latest picture of `camera` (privacy masks applied), if it is running.
async fn camera_picture(
    shared: &Shared,
    camera: &CameraId,
    settings: &MotionSettings,
) -> Option<Bytes> {
    let tap = {
        let state = shared.state.lock().await;
        state.running.get(camera).map(|rt| rt.detection_tap())
    }?;
    match snapshot_jpeg(
        &shared.ffmpeg,
        &tap,
        THUMBNAIL_WIDTH,
        &settings.mask_rects(),
    )
    .await
    {
        Ok(jpeg) => Some(jpeg),
        Err(e) => {
            debug!(%camera, error = %e, "no picture of the crossing");
            None
        }
    }
}

/// Checks, records and announces a crossing of `line`. For a line limited
/// to objects (with object recognition running), a picture is analysed
/// first: the crossing counts only when one of the objects is at the line.
async fn crossing(
    shared: Arc<Shared>,
    db: Database,
    camera: CameraId,
    line: String,
    forward: bool,
    at_ms: i64,
) {
    let settings = camera_settings(&shared, &camera).await;
    let Some(geometry) = settings.lines.iter().find(|l| l.name == line).cloned() else {
        return;
    };
    let ai = if geometry.objects.is_empty() {
        None
    } else {
        shared.ai()
    };
    let mut picture = None;
    if ai.is_some() || super::webhooks::want_crossing_picture(&shared, &camera) {
        picture = camera_picture(&shared, &camera, &settings).await;
    }
    let mut object = None;
    if let Some(ai) = ai {
        let Some(jpeg) = picture.clone() else {
            debug!(%camera, line, "crossing not checked: no picture");
            return;
        };
        let started = std::time::Instant::now();
        match ai.analyze(&jpeg).await {
            Ok(objects) => {
                let seen: Vec<DetectedObject> = objects
                    .iter()
                    .filter(|o| geometry.objects.contains(&o.label) && at_line(o, &geometry))
                    .cloned()
                    .collect();
                keep_picture(
                    &shared,
                    &camera,
                    &jpeg,
                    Capture {
                        at_ms: jiff::Timestamp::now().as_millisecond(),
                        event: None,
                        picture: None,
                        line: Some(line.clone()),
                        provider: ai.provider().to_owned(),
                        ms: started.elapsed().as_millis() as u64,
                        found: objects,
                        kept: seen.iter().map(|o| o.label.clone()).collect(),
                    },
                );
                let Some(best) = seen.iter().max_by(|a, b| a.score.total_cmp(&b.score)) else {
                    debug!(%camera, line, "crossing ignored: none of the line's objects at the line");
                    return;
                };
                let label = best.label.clone();
                let boxes: Vec<[f32; 4]> = seen
                    .iter()
                    .filter(|o| o.label == label)
                    .filter_map(|o| o.bbox)
                    .collect();
                if let Ok(boxed) = boxed_jpeg(&shared.ffmpeg, jpeg, &boxes).await {
                    picture = Some(boxed);
                }
                object = Some(label);
            }
            // Counted all the same, without object (webhooks limited to
            // objects skip it).
            Err(e) => {
                warn!(%camera, line, error = %e, "cannot check the crossing: object recognition failed")
            }
        }
    }
    if let Err(err) = db
        .add_crossing(camera.as_str(), &line, forward, at_ms)
        .await
    {
        warn!(%camera, error = %err, "cannot record a crossing");
        return;
    }
    // Today's count of the line, for MQTT.
    let midnight = jiff::Zoned::now()
        .start_of_day()
        .map_or(at_ms, |z| z.timestamp().as_millisecond());
    let today = db.crossings(midnight, at_ms + 1).await.map_or(0, |list| {
        list.iter()
            .filter(|(c, l, _, _)| c == camera.as_str() && *l == line)
            .count() as u64
    });
    let present = if geometry.presence {
        Some(line_presence(&db, &camera, &line).await)
    } else {
        None
    };
    info!(%camera, line, forward, today, object = object.as_deref().unwrap_or(""), present = present.map(|p| p.to_string()).unwrap_or_default(), "line crossed");
    let _ = shared.engine.events.send(NvrEvent::Crossing {
        camera: camera.clone(),
        line: line.clone(),
        forward,
        at_ms,
        today,
        object,
        picture,
    });
    if let Some(present) = present {
        let _ = shared.engine.events.send(NvrEvent::Presence {
            camera,
            line,
            present,
        });
    }
}

/// Keeps an analysed picture and what was found in it, when
/// `ai.keep_pictures` is on, for the replays. Written off the caller.
fn keep_picture(shared: &Shared, camera: &CameraId, jpeg: &Bytes, capture: Capture) {
    let keep = shared
        .ai_settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0
        .keep_pictures;
    let root = shared
        .captures_root
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let (true, Some((root, _))) = (keep, root) else {
        return;
    };
    let (camera, jpeg) = (camera.clone(), jpeg.clone());
    tokio::task::spawn_blocking(move || {
        if let Err(e) = crate::captures::save(&root, &camera, &jpeg, &capture) {
            warn!(%camera, error = %e, "cannot keep the analysed picture");
        }
    });
}

/// Local midnight of today, in milliseconds since the Unix epoch.
fn local_midnight_ms() -> i64 {
    let now = jiff::Zoned::now();
    now.start_of_day()
        .map_or(now.timestamp().as_millisecond(), |z| {
            z.timestamp().as_millisecond()
        })
}

/// Who is inside according to `line` of `camera` (a line with `presence`):
/// entries minus exits since local midnight or its last reset, whichever is
/// later, never below zero.
pub(super) async fn line_presence(db: &Database, camera: &CameraId, line: &str) -> u64 {
    let reset = db
        .presence_reset(camera.as_str(), line)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let from = local_midnight_ms().max(reset);
    match db.line_crossings_since(camera.as_str(), line, from).await {
        Ok(crossings) => rustnvr_core::presence(crossings),
        Err(e) => {
            warn!(%camera, line, error = %e, "cannot count the presence");
            0
        }
    }
}

/// The lines counting the presence, for each camera.
async fn presence_lines(shared: &Shared) -> Vec<(CameraId, String)> {
    let state = shared.state.lock().await;
    let mut out: Vec<(CameraId, String)> = state
        .motion
        .iter()
        .flat_map(|(camera, s)| {
            s.lines
                .iter()
                .filter(|l| l.presence)
                .map(|l| (camera.clone(), l.name.clone()))
        })
        .collect();
    out.sort();
    out
}

/// Who is inside according to every line counting the presence.
pub(super) async fn presence_counts(shared: &Shared) -> Vec<(CameraId, String, u64)> {
    let Some(db) = shared.database.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (camera, line) in presence_lines(shared).await {
        let present = line_presence(db, &camera, &line).await;
        out.push((camera, line, present));
    }
    out
}

/// Starts the presence count of `line` of `camera` again from zero.
pub(super) async fn reset_presence(
    shared: &Shared,
    camera: &CameraId,
    line: &str,
) -> Result<(), ControlError> {
    let db = shared.database.as_ref().ok_or_else(|| {
        ControlError::Invalid("pas de base de données : aucun comptage".to_owned())
    })?;
    if !presence_lines(shared)
        .await
        .iter()
        .any(|(c, l)| c == camera && l == line)
    {
        return Err(ControlError::NotFound(format!(
            "la caméra {camera} n'a pas de ligne {line:?} qui compte les présences"
        )));
    }
    let now = jiff::Timestamp::now().as_millisecond();
    db.set_presence_reset(camera.as_str(), line, now)
        .await
        .map_err(|e| ControlError::Internal(e.to_string()))?;
    info!(%camera, line, "presence count reset");
    let _ = shared.engine.events.send(NvrEvent::Presence {
        camera: camera.clone(),
        line: line.to_owned(),
        present: 0,
    });
    Ok(())
}

/// At each local midnight, announces that the presence counts start again
/// from zero.
pub(super) async fn run_presence_midnight(shared: Arc<Shared>, cancel: CancellationToken) {
    loop {
        let wait = jiff::Zoned::now()
            .tomorrow()
            .and_then(|d| d.start_of_day())
            .ok()
            .and_then(|midnight| {
                let ms =
                    midnight.timestamp().as_millisecond() - jiff::Timestamp::now().as_millisecond();
                u64::try_from(ms).ok()
            })
            .map_or(Duration::from_secs(3600), Duration::from_millis);
        tokio::select! {
            _ = cancel.cancelled() => return,
            // A little after midnight, so that « today » is the new day.
            _ = tokio::time::sleep(wait + Duration::from_secs(1)) => {}
        }
        for (camera, line, present) in presence_counts(&shared).await {
            let _ = shared.engine.events.send(NvrEvent::Presence {
                camera,
                line,
                present,
            });
        }
    }
}

/// Takes, stores and announces the thumbnail of an event.
async fn take_thumbnail(
    shared: Arc<Shared>,
    db: Database,
    event: DetectionEvent,
    done: mpsc::Sender<EventUpdate>,
) {
    tokio::time::sleep(THUMBNAIL_DELAY).await;
    let tap = {
        let state = shared.state.lock().await;
        state
            .running
            .get(&event.camera)
            .map(|rt| rt.detection_tap())
    };
    let Some(tap) = tap else { return };
    let masks = camera_settings(&shared, &event.camera).await.mask_rects();
    match snapshot_jpeg(&shared.ffmpeg, &tap, THUMBNAIL_WIDTH, &masks).await {
        Ok(jpeg) => {
            if let Err(e) = db.set_thumbnail(&event, &jpeg).await {
                warn!(camera = %event.camera, error = %e, "cannot store event thumbnail");
                return;
            }
            let _ = shared.engine.events.send(NvrEvent::Snapshot {
                camera: event.camera.clone(),
                jpeg: jpeg.clone(),
            });
            let _ = done
                .send(EventUpdate::Thumbnail {
                    camera: event.camera.clone(),
                    id: event.id,
                })
                .await;
            if let Some(ai) = shared.ai() {
                recognize(&shared, &ai, &db, &event, &tap, jpeg, &done).await;
            }
        }
        Err(e) => debug!(camera = %event.camera, error = %e, "no thumbnail for event"),
    }
}

/// Recognizes objects in the pictures of `event`: the thumbnail first, then
/// a new picture every few seconds while the event lasts and nothing is
/// recognized (a person entering the picture after the start of the motion),
/// up to the configured number of pictures. The picture where objects are
/// found becomes the thumbnail of the event.
async fn recognize(
    shared: &Shared,
    ai: &rustnvr_ai::Analyzer,
    db: &Database,
    event: &DetectionEvent,
    tap: &LiveTap,
    thumbnail: Bytes,
    done: &mpsc::Sender<EventUpdate>,
) {
    let (max_pictures, interval) = ai.schedule();
    // Objects chosen for this camera (empty: all those of the settings).
    let wanted = camera_settings(shared, &event.camera).await;
    let mut jpeg = thumbnail;
    for picture in 1..=max_pictures {
        let started = std::time::Instant::now();
        let found = match ai.analyze(&jpeg).await {
            Ok(found) => found,
            Err(e) => {
                // Do not insist with a provider that does not answer.
                warn!(camera = %event.camera, error = %e, "object recognition failed");
                return;
            }
        };
        let objects = place(found.clone(), &wanted);
        let ms = started.elapsed().as_millis() as u64;
        keep_picture(
            shared,
            &event.camera,
            &jpeg,
            Capture {
                at_ms: jiff::Timestamp::now().as_millisecond(),
                event: Some(event.id),
                picture: Some(picture),
                line: None,
                provider: ai.provider().to_owned(),
                ms,
                found,
                kept: objects.iter().map(|o| o.label.clone()).collect(),
            },
        );
        if !objects.is_empty() {
            let labels: Vec<&str> = objects.iter().map(|o| o.label.as_str()).collect();
            info!(camera = %event.camera, id = event.id, picture, objects = ?labels, ms, "objects recognized");
            if picture > 1 {
                // The thumbnail shows what was recognized (boxes match it).
                if let Err(e) = db.set_thumbnail(event, &jpeg).await {
                    warn!(camera = %event.camera, error = %e, "cannot store event thumbnail");
                }
                let _ = shared.engine.events.send(NvrEvent::Snapshot {
                    camera: event.camera.clone(),
                    jpeg: jpeg.clone(),
                });
            }
            if let Err(e) = db.set_objects(event.id, &objects).await {
                warn!(camera = %event.camera, error = %e, "cannot store recognized objects");
                return;
            }
            // Before the « Personne » sensor turns on: an automation started
            // by it finds the picture already there.
            object_pictures(shared, &event.camera, &jpeg, &objects).await;
            let _ = done
                .send(EventUpdate::Objects {
                    camera: event.camera.clone(),
                    id: event.id,
                    objects,
                })
                .await;
            return;
        }
        debug!(camera = %event.camera, id = event.id, picture, ms, "nothing recognized");
        if picture == max_pictures {
            return;
        }
        tokio::time::sleep(interval).await;
        // Stop with the event.
        match db.event(event.id).await {
            Ok(Some(e)) if e.end_ms.is_none() => {}
            _ => return,
        }
        jpeg = match snapshot_jpeg(&shared.ffmpeg, tap, THUMBNAIL_WIDTH, &wanted.mask_rects()).await
        {
            Ok(j) => j,
            Err(e) => {
                debug!(camera = %event.camera, error = %e, "no picture to analyse");
                return;
            }
        };
    }
}

/// Announces, for each kind of recognized object, the picture with the boxes
/// of the objects of that kind (MQTT `<camera>/<label>/snapshot`). Nothing is
/// drawn while MQTT is off.
async fn object_pictures(
    shared: &Shared,
    camera: &CameraId,
    jpeg: &Bytes,
    objects: &[DetectedObject],
) {
    if shared.mqtt.lock().await.handle.is_none() {
        return;
    }
    let mut labels: Vec<&str> = objects.iter().map(|o| o.label.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    for label in labels {
        let boxes: Vec<[f32; 4]> = objects
            .iter()
            .filter(|o| o.label == label)
            .filter_map(|o| o.bbox)
            .collect();
        match boxed_jpeg(&shared.ffmpeg, jpeg.clone(), &boxes).await {
            Ok(picture) => {
                let _ = shared.engine.events.send(NvrEvent::ObjectSnapshot {
                    camera: camera.clone(),
                    label: label.to_owned(),
                    jpeg: picture,
                });
            }
            Err(e) => warn!(%camera, label, error = %e, "cannot draw the recognized objects"),
        }
    }
}

/// Stores the detection settings of a camera and restarts its detection.
pub(super) async fn set_motion(
    shared: &Arc<Shared>,
    raw_id: &str,
    settings: MotionSettings,
) -> Result<(), ControlError> {
    let problems = settings.problems();
    if !problems.is_empty() {
        return Err(ControlError::Invalid(problems.join("; ")));
    }
    let db = shared.database.as_ref().ok_or_else(|| {
        ControlError::Internal(
            "base de données indisponible : la détection ne peut pas être configurée".into(),
        )
    })?;
    let mut guard = shared.state.lock().await;
    let state: &mut State = &mut guard;
    let id = state
        .configs
        .iter()
        .find(|c| c.id.as_str() == raw_id)
        .map(|c| c.id.clone())
        .ok_or_else(|| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
    db.set_motion_settings(&id, &settings)
        .await
        .map_err(|e| ControlError::Internal(e.to_string()))?;
    let enabled = settings.enabled;
    state.motion.insert(id.clone(), settings);
    if let Some(rt) = state.running.get_mut(&id) {
        if let Some(old) = rt.motion.take() {
            old.stop().await;
        }
        shared.engine.start_motion(rt, state.motion.get(&id));
    }
    info!(camera = %id, enabled, "motion detection settings changed");
    shared.publish(state);
    let _ = shared.engine.events.send(NvrEvent::MotionSettingsChanged {
        camera: id,
        enabled,
    });
    Ok(())
}

/// What the interface shows of the MQTT settings and connection.
pub(super) async fn mqtt_view(shared: &Arc<Shared>) -> MqttView {
    let rt = shared.mqtt.lock().await;
    let c = &rt.config;
    let status = rt
        .handle
        .as_ref()
        .map(|h| h.status.borrow().clone())
        .unwrap_or_default();
    MqttView {
        enabled: c.enabled,
        host: c.host.clone(),
        port: c.port,
        tls: c.tls,
        ca_file: c.ca_file.as_ref().map(|p| p.display().to_string()),
        username: c.username.clone(),
        has_password: c.password.as_ref().is_some_and(|p| !p.is_empty())
            || c.password_env.is_some(),
        client_id: c.client_id.clone(),
        topic_prefix: c.topic_prefix.clone(),
        discovery: c.discovery,
        discovery_prefix: c.discovery_prefix.clone(),
        managed: rt.managed,
        connected: status.connected,
        error: if c.enabled {
            status.error.or_else(|| {
                rt.handle
                    .is_none()
                    .then(|| "client MQTT non démarré (réglages invalides ?)".to_owned())
            })
        } else {
            None
        },
    }
}

/// Validates, stores and applies MQTT settings from the interface.
pub(super) async fn set_mqtt(shared: &Arc<Shared>, input: MqttInput) -> Result<(), ControlError> {
    let path = shared.managed_mqtt_file.clone().ok_or_else(|| {
        ControlError::Internal("no configuration file to store settings next to".into())
    })?;
    let mut rt = shared.mqtt.lock().await;
    let previous = rt.config.clone();
    let username = input
        .username
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty());
    // No password in the input keeps the stored one (never sent back).
    let (password, password_env) = match input.password {
        Some(p) if p.is_empty() => (None, None),
        Some(p) => (Some(p), None),
        None => (previous.password.clone(), previous.password_env.clone()),
    };
    let config = MqttConfig {
        enabled: input.enabled,
        host: input.host.trim().to_owned(),
        port: input.port,
        tls: input.tls,
        ca_file: input
            .ca_file
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from),
        password: username.as_ref().and(password),
        password_env: username.as_ref().and(password_env),
        username,
        client_id: input.client_id.trim().to_owned(),
        topic_prefix: input.topic_prefix.trim().to_owned(),
        discovery: input.discovery,
        discovery_prefix: input.discovery_prefix.trim().to_owned(),
    };
    let problems = config.problems(|n| std::env::var(n).ok());
    if !problems.is_empty() {
        return Err(ControlError::Invalid(problems.join("; ")));
    }
    mqtt_store::save(&path, &config).map_err(|e| {
        warn!(path = %path.display(), error = %e, "cannot save MQTT settings");
        ControlError::Internal(format!("cannot write {}: {e}", path.display()))
    })?;
    rt.config = config;
    rt.managed = true;
    if let Some(old) = rt.handle.take() {
        old.stop().await;
    }
    rt.handle = start_mqtt(shared, &rt.config);
    info!(enabled = rt.config.enabled, broker = %format!("{}:{}", rt.config.host, rt.config.port), "MQTT settings changed from the web interface");
    Ok(())
}

/// (Re)starts the MQTT client with the current settings.
pub(super) async fn restart_mqtt(shared: &Arc<Shared>) {
    let mut rt = shared.mqtt.lock().await;
    if let Some(old) = rt.handle.take() {
        old.stop().await;
    }
    rt.handle = start_mqtt(shared, &rt.config);
}

fn start_mqtt(shared: &Arc<Shared>, config: &MqttConfig) -> Option<MqttHandle> {
    if !config.enabled {
        return None;
    }
    match MqttSettings::from_config(config, |n| std::env::var(n).ok()) {
        Ok(settings) => {
            info!(broker = %format!("{}:{}", settings.host, settings.port), discovery = settings.discovery_prefix.is_some(), "starting MQTT client");
            Some(spawn_mqtt(
                settings,
                Arc::new(MqttBridge(Arc::downgrade(shared))),
                &shared.engine.events,
                CancellationToken::new(),
            ))
        }
        Err(e) => {
            warn!(error = %e, "MQTT not started");
            None
        }
    }
}

/// The NVR as seen by the MQTT client. Weak: the client must not keep the
/// NVR alive.
struct MqttBridge(Weak<Shared>);

impl NvrView for MqttBridge {
    fn cameras(&self) -> BoxFuture<'_, Vec<CameraView>> {
        Box::pin(async move {
            let Some(shared) = self.0.upgrade() else {
                return Vec::new();
            };
            // Before locking the state (the counts read it too).
            let presence = presence_counts(&shared).await;
            let state = shared.state.lock().await;
            state
                .configs
                .iter()
                .map(|c| {
                    let rt = state.running.get(&c.id);
                    CameraView {
                        id: c.id.clone(),
                        name: c.name.clone().unwrap_or_else(|| c.id.to_string()),
                        online: rt.is_some_and(|r| r.stream.state.borrow().is_streaming()),
                        recording: c.enabled && c.record.enabled,
                        record_mode: c.record.mode,
                        motion_enabled: state.motion.get(&c.id).is_some_and(|m| m.enabled),
                        motion_active: rt
                            .and_then(|r| r.motion.as_ref())
                            .is_some_and(|m| m.status.borrow().active),
                        audio_enabled: c.record.audio,
                        person_detection: shared.ai().is_some(),
                        presence: presence
                            .iter()
                            .filter(|(cam, _, _)| *cam == c.id)
                            .map(|(_, line, n)| (line.clone(), *n))
                            .collect(),
                        ptz_presets: if c.onvif.is_some() {
                            shared.ptz.preset_names(c.id.as_str())
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect()
        })
    }

    fn system(&self) -> BoxFuture<'_, SystemView> {
        Box::pin(async move {
            let Some(shared) = self.0.upgrade() else {
                return SystemView::default();
            };
            let (online, dir) = {
                let state = shared.state.lock().await;
                let online = state
                    .running
                    .values()
                    .filter(|r| r.stream.state.borrow().is_streaming())
                    .count();
                (online, state.storage.local_dir.clone())
            };
            let disk = tokio::task::spawn_blocking(move || rustnvr_api::storage::disk_usage(&dir))
                .await
                .ok()
                .flatten();
            SystemView {
                cameras_online: online,
                disk_used_percent: disk
                    .filter(|d| d.total_bytes > 0)
                    .map(|d| (d.used_bytes as f64 * 100.0 / d.total_bytes as f64) as f32),
                low_space: !shared
                    .low_space
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty(),
                storage_problem: !shared
                    .storage_problems
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty(),
                detection_paused: shared
                    .engine
                    .detection_paused
                    .load(std::sync::atomic::Ordering::Relaxed),
            }
        })
    }

    fn set_motion_enabled(
        &self,
        camera: CameraId,
        enabled: bool,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            let controller = Controller(shared.clone());
            let mut settings = controller
                .motion(camera.to_string())
                .await
                .map_err(|e| e.to_string())?;
            if settings.enabled == enabled {
                return Ok(());
            }
            if enabled && !shared.ffmpeg_ok {
                return Err("FFmpeg indisponible".to_owned());
            }
            settings.enabled = enabled;
            set_motion(&shared, camera.as_str(), settings)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn set_audio_enabled(
        &self,
        camera: CameraId,
        enabled: bool,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            Controller(shared)
                .set_audio(camera.to_string(), enabled)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn reset_presence(&self, camera: CameraId, label: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            // The topic level of a line is its name made topic-safe.
            let line = presence_lines(&shared)
                .await
                .into_iter()
                .find(|(c, l)| *c == camera && rustnvr_mqtt::topic_label(l) == label)
                .map(|(_, l)| l)
                .ok_or_else(|| format!("aucune ligne de présence {label:?}"))?;
            reset_presence(&shared, &camera, &line)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn ptz_goto(&self, camera: CameraId, name: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            super::ptz::goto_name(&shared, camera.as_str(), &name)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn set_record_mode(
        &self,
        camera: CameraId,
        mode: Option<RecordMode>,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            super::set_record_mode(&shared, camera.as_str(), mode)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn manual_event(&self, camera: CameraId, seconds: u32) -> BoxFuture<'_, Result<i64, String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            let manual = shared
                .manual
                .clone()
                .ok_or("base de données indisponible : pas d'événements")?;
            {
                let state = shared.state.lock().await;
                let rt = state
                    .running
                    .get(&camera)
                    .ok_or_else(|| format!("caméra {camera} inconnue ou désactivée"))?;
                if !rt.stream.state.borrow().is_streaming() {
                    return Err(format!("caméra {camera} hors ligne"));
                }
            }
            let (reply, answer) = tokio::sync::oneshot::channel();
            manual
                .send(Manual {
                    camera,
                    seconds,
                    reply,
                })
                .await
                .map_err(|_| "NVR arrêté")?;
            answer.await.map_err(|_| "NVR arrêté")?
        })
    }

    fn capture(&self, camera: CameraId) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            if !shared.ffmpeg_ok {
                return Err("FFmpeg indisponible".to_owned());
            }
            let tap = {
                let state = shared.state.lock().await;
                state.running.get(&camera).map(|rt| rt.live.clone())
            }
            .ok_or_else(|| format!("caméra {camera} inconnue ou désactivée"))?;
            let masks = camera_settings(&shared, &camera).await.mask_rects();
            let jpeg = snapshot_jpeg(&shared.ffmpeg, &tap, CAPTURE_WIDTH, &masks).await?;
            info!(%camera, bytes = jpeg.len(), "picture taken on command");
            Ok(jpeg.to_vec())
        })
    }

    fn set_detection_paused(&self, paused: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let shared = self.0.upgrade().ok_or("NVR arrêté")?;
            set_detection_paused(&shared, paused)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

/// Remote target of a destination, with its secrets resolved.
fn remote_target(remote: &RemoteConfig) -> Result<RemoteTarget, String> {
    let env = |name: &str| {
        std::env::var(name).map_err(|_| format!("variable d'environnement {name:?} absente"))
    };
    Ok(match remote {
        RemoteConfig::S3(s) => RemoteTarget::S3(S3Target {
            endpoint: s.endpoint.clone(),
            region: s.region.clone(),
            bucket: s.bucket.clone(),
            prefix: s.prefix.clone(),
            access_key_id: s.access_key_id.clone(),
            secret_access_key: match (&s.secret_access_key, &s.secret_access_key_env) {
                (Some(k), _) => k.clone(),
                (None, Some(var)) => Secret::new(env(var)?),
                (None, None) => return Err("clé secrète S3 manquante".to_owned()),
            },
        }),
        RemoteConfig::Sftp(s) => RemoteTarget::Sftp(SftpTarget {
            host: s.host.clone(),
            port: s.port,
            username: s.username.clone(),
            password: match (&s.password, &s.password_env) {
                (Some(p), _) => Some(p.clone()),
                (None, Some(var)) => Some(Secret::new(env(var)?)),
                (None, None) => None,
            },
            private_key: s.private_key.clone(),
            remote_dir: s.remote_dir.clone(),
            host_key: s.host_key.clone(),
        }),
    })
}

/// (Re)starts one uploader per remote destination.
pub(super) async fn restart_uploaders(shared: &Arc<Shared>) {
    let destinations = shared.state.lock().await.storage.destinations.clone();
    let mut uploaders = shared.uploaders.lock().await;
    for (_, old) in uploaders.drain() {
        old.stop().await;
    }
    let mut status = HashMap::new();
    for d in destinations {
        let Some(remote) = &d.remote else { continue };
        match remote_target(remote) {
            Ok(target) => {
                info!(destination = %d.id, kind = remote.kind(), outbox = %d.path.display(), "starting uploader");
                let handle = spawn_uploader(
                    d.id.clone(),
                    target,
                    d.path.clone(),
                    CancellationToken::new(),
                );
                status.insert(d.id.clone(), handle.status.clone());
                uploaders.insert(d.id.clone(), handle);
            }
            Err(e) => warn!(destination = %d.id, error = %e, "uploader not started"),
        }
    }
    drop(uploaders);
    *shared
        .upload_status
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = status;
    // The API shows the upload state of each destination.
    let state = shared.state.lock().await;
    shared.publish(&state);
}

/// General settings in effect.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GeneralSettings {
    pub(super) recording: RecordingConfig,
    pub(super) motion: MotionConfig,
    pub(super) live_transcode: bool,
    pub(super) live_max_bitrate_kbps: u32,
    pub(super) live_hwaccel: HwAccelMode,
    pub(super) low_space_percent: u8,
    pub(super) default_destination: String,
    pub(super) restream: RestreamConfig,
    /// Changed from the interface (`settings.ui.toml` exists).
    pub(super) managed: bool,
}

impl GeneralSettings {
    pub(super) fn from_config(c: &Config) -> Self {
        Self {
            recording: c.recording.clone(),
            motion: c.motion.clone(),
            live_transcode: c.live.transcode,
            live_max_bitrate_kbps: c.live.max_bitrate_kbps,
            live_hwaccel: c.live.hwaccel,
            low_space_percent: c.storage.low_space_percent,
            default_destination: c.storage.default_destination.clone(),
            restream: c.restream.clone(),
            managed: c
                .managed_settings_file
                .as_ref()
                .is_some_and(|p| p.is_file()),
        }
    }
}

pub(super) fn settings_view(shared: &Arc<Shared>) -> SettingsView {
    let g = shared.general();
    SettingsView {
        recording: g.recording,
        motion: g.motion,
        live_transcode: g.live_transcode,
        live_max_bitrate_kbps: g.live_max_bitrate_kbps,
        live_hwaccel: g.live_hwaccel,
        low_space_percent: g.low_space_percent,
        default_destination: g.default_destination,
        restream: rustnvr_api::RestreamView {
            enabled: g.restream.enabled,
            listen: g.restream.listen.clone(),
            max_clients: g.restream.max_clients,
            max_clients_per_address: g.restream.max_clients_per_address,
            allow_unauthenticated_remote: g.restream.allow_unauthenticated_remote,
            username: g.restream.username.clone(),
            has_password: g.restream.password.as_ref().is_some_and(|p| !p.is_empty())
                || g.restream.password_env.is_some(),
        },
        restream_error: shared
            .restream_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        ffmpeg: shared.ffmpeg.clone(),
        managed: g.managed,
    }
}

/// Writes `settings.ui.toml` atomically.
fn save_settings(path: &std::path::Path, s: &ManagedSettings) -> std::io::Result<()> {
    let body = toml::to_string(s).map_err(std::io::Error::other)?;
    let tmp = path.with_file_name(format!("{MANAGED_SETTINGS_FILE}.tmp"));
    // May hold the password of the RTSP republication: its owner only.
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    std::io::Write::write_all(
        &mut f,
        format!(
            "# Réglages généraux modifiés depuis l'interface web de RustNVR (fichier géré\n\
             # automatiquement, à garder privé). Ils remplacent ceux de config.toml.\n\n{body}"
        )
        .as_bytes(),
    )?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Stores and applies general settings from the interface.
pub(super) async fn set_settings(
    shared: &Arc<Shared>,
    input: SettingsInput,
) -> Result<(), ControlError> {
    let path = shared.managed_settings_file.clone().ok_or_else(|| {
        ControlError::Internal("no configuration file to store settings next to".into())
    })?;
    let previous = shared.general();
    let default_destination = match input.default_destination {
        Some(id) => {
            if shared.state.lock().await.storage.destination(&id).is_none() {
                return Err(ControlError::Invalid(format!(
                    "point de stockage inconnu : {id:?}"
                )));
            }
            id
        }
        None => previous.default_destination.clone(),
    };
    let restream = match input.restream {
        None => previous.restream.clone(),
        Some(r) => {
            let username = r
                .username
                .map(|u| u.trim().to_owned())
                .filter(|u| !u.is_empty());
            // No password in the input keeps the stored one (never sent
            // back); without a user name there is none.
            let (password, password_env) = match (&username, r.password) {
                (None, _) => (None, None),
                (Some(_), Some(p)) => ((!p.is_empty()).then_some(p), None),
                (Some(_), None) => (
                    previous.restream.password.clone(),
                    previous.restream.password_env.clone(),
                ),
            };
            let next = RestreamConfig {
                enabled: r.enabled,
                listen: r.listen.trim().to_owned(),
                max_clients: r.max_clients,
                max_clients_per_address: r
                    .max_clients_per_address
                    .unwrap_or(previous.restream.max_clients_per_address)
                    .min(r.max_clients.max(1)),
                allow_unauthenticated_remote: r.allow_unauthenticated_remote,
                username,
                password,
                password_env,
            };
            let problems = next.problems(|n| std::env::var(n).ok());
            if !problems.is_empty() {
                return Err(ControlError::Invalid(problems.join("; ")));
            }
            next
        }
    };
    let next = GeneralSettings {
        recording: input.recording,
        motion: input.motion,
        live_transcode: input.live_transcode,
        live_max_bitrate_kbps: input.live_max_bitrate_kbps,
        live_hwaccel: input.live_hwaccel.unwrap_or(previous.live_hwaccel),
        low_space_percent: input.low_space_percent,
        default_destination,
        restream,
        managed: true,
    };
    save_settings(
        &path,
        &ManagedSettings {
            recording: Some(next.recording.clone()),
            motion: Some(next.motion.clone()),
            live_transcode: Some(next.live_transcode),
            live_max_bitrate_kbps: Some(next.live_max_bitrate_kbps),
            live_hwaccel: Some(next.live_hwaccel),
            low_space_percent: Some(next.low_space_percent),
            default_destination: Some(next.default_destination.clone()),
            restream: Some(next.restream.clone()),
        },
    )
    .map_err(|e| ControlError::Internal(format!("cannot write {}: {e}", path.display())))?;
    *shared.general.lock().unwrap_or_else(|e| e.into_inner()) = next.clone();

    if let Some(api) = shared.api.get() {
        api.set_live_settings(LiveSettings {
            transcode: next.live_transcode,
            ffmpeg: shared.ffmpeg.clone(),
            max_bitrate_kbps: next.live_max_bitrate_kbps,
            hw: None,
        });
        api.set_low_space_percent(next.low_space_percent);
        api.set_default_destination(next.default_destination.clone());
        if next.live_hwaccel != previous.live_hwaccel {
            // A test conversion: in the background.
            tokio::spawn(api.clone().configure_hwaccel(next.live_hwaccel));
        }
    }
    if next.restream != previous.restream {
        super::restream::restart(shared).await;
    }
    let recording_changed = next.recording != previous.recording;
    let motion_changed = next.motion != previous.motion;
    *shared
        .engine
        .recording
        .write()
        .unwrap_or_else(|e| e.into_inner()) = next.recording.clone();
    *shared
        .engine
        .motion
        .write()
        .unwrap_or_else(|e| e.into_inner()) = MotionOptions::new(&next.motion, &shared.ffmpeg);

    let mut guard = shared.state.lock().await;
    let state: &mut State = &mut guard;
    if recording_changed {
        // Recorders take their settings when they start.
        let cameras: Vec<_> = state
            .configs
            .iter()
            .filter(|c| state.running.contains_key(&c.id))
            .cloned()
            .collect();
        for cam in cameras {
            if let Some(old) = state.running.remove(&cam.id) {
                old.stop().await;
            }
            match shared
                .engine
                .start(&cam, &state.storage, state.motion.get(&cam.id))
            {
                Ok(rt) => {
                    state.running.insert(cam.id.clone(), rt);
                }
                Err(e) => warn!(camera = %cam.id, error = %e, "cannot restart camera"),
            }
        }
    } else if motion_changed {
        let ids: Vec<CameraId> = state.running.keys().cloned().collect();
        for id in ids {
            let settings = state.motion.get(&id).cloned();
            if let Some(rt) = state.running.get_mut(&id) {
                if let Some(old) = rt.motion.take() {
                    old.stop().await;
                }
                shared.engine.start_motion(rt, settings.as_ref());
            }
        }
    }
    info!(
        recording_changed,
        motion_changed, "general settings changed from the web interface"
    );
    shared.publish(state);
    Ok(())
}

/// Detection settings of `camera` (objects and zones that count).
pub(super) async fn camera_settings(shared: &Shared, camera: &CameraId) -> MotionSettings {
    shared
        .state
        .lock()
        .await
        .motion
        .get(camera)
        .cloned()
        .unwrap_or_default()
}

/// The objects that count for the camera, each with the zone it is in.
pub(super) fn place(
    objects: Vec<DetectedObject>,
    settings: &MotionSettings,
) -> Vec<DetectedObject> {
    objects
        .into_iter()
        .filter_map(|mut o| {
            o.zone = settings.locate(&o)?;
            Some(o)
        })
        .collect()
}

/// Starts object recognition from `[ai]`; returns it, or why it is not running.
pub(super) fn start_ai(
    settings: &AiConfig,
    detection_ready: bool,
) -> (Option<rustnvr_ai::Analyzer>, Option<String>) {
    match rustnvr_ai::Analyzer::from_config(settings, |n| std::env::var(n).ok()) {
        Ok(Some(a)) if detection_ready => {
            info!(
                provider = a.provider(),
                "object recognition enabled on event pictures"
            );
            (Some(a), None)
        }
        Ok(Some(_)) => {
            let why = "la reconnaissance d'objets nécessite la détection de mouvement (base de données et FFmpeg)";
            warn!("object recognition needs motion detection (database and FFmpeg): disabled");
            (None, Some(why.to_owned()))
        }
        Ok(None) => (None, None),
        Err(e) => {
            warn!(error = %e, "object recognition disabled");
            (None, Some(e))
        }
    }
}

/// Object recognition settings and state, for the interface.
pub(super) fn ai_view(shared: &Arc<Shared>) -> AiView {
    let (settings, error) = shared
        .ai_settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    AiView {
        settings,
        active: shared.ai().map(|a| a.provider().to_owned()),
        device: shared.ai().and_then(|a| a.device()),
        error,
        onnx_available: cfg!(feature = "onnx"),
        detection_ready: shared.database.is_some() && shared.ffmpeg_ok,
        managed: shared.managed_ai_file.as_ref().is_some_and(|p| p.is_file()),
        models: rustnvr_ai::models::MODELS
            .iter()
            .map(|m| model_view(shared, m, None))
            .collect(),
    }
}

/// Directory of the downloaded models.
fn models_dir(shared: &Shared) -> PathBuf {
    shared.data_dir.join("models")
}

/// What the interface shows of a model; `downloaded` checked from the
/// file size when not given.
fn model_view(
    shared: &Shared,
    m: &rustnvr_ai::models::ModelInfo,
    downloaded: Option<bool>,
) -> AiModelView {
    let path = models_dir(shared).join(m.file);
    AiModelView {
        id: m.id.to_owned(),
        name: m.name.to_owned(),
        bytes: m.bytes,
        license: m.license.to_owned(),
        hint: m.hint.to_owned(),
        downloaded: downloaded
            .unwrap_or_else(|| std::fs::metadata(&path).is_ok_and(|f| f.len() == m.bytes)),
        path: path.display().to_string(),
    }
}

/// Downloads a model of the built-in recognition (one download at a time).
pub(super) async fn ai_download_model(
    shared: &Arc<Shared>,
    id: &str,
) -> Result<AiModelView, ControlError> {
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let model = rustnvr_ai::models::find(id)
        .ok_or_else(|| ControlError::NotFound(format!("aucun modèle {id:?}")))?;
    let _guard = ONE_AT_A_TIME.lock().await;
    let started = std::time::Instant::now();
    match rustnvr_ai::models::download(model, &models_dir(shared)).await {
        Ok(path) => {
            info!(model = model.id, path = %path.display(), ms = started.elapsed().as_millis() as u64, "AI model ready");
            Ok(model_view(shared, model, Some(true)))
        }
        Err(e) => {
            warn!(model = model.id, error = %e, "AI model download failed");
            Err(ControlError::Internal(e))
        }
    }
}

/// Starts the provider of `settings` (refused if it cannot start), stores the
/// settings in `ai.ui.toml` and uses them from the next event on.
pub(super) async fn set_ai(shared: &Arc<Shared>, settings: AiConfig) -> Result<(), ControlError> {
    let path = shared.managed_ai_file.clone().ok_or_else(|| {
        ControlError::Internal("no configuration file to store settings next to".into())
    })?;
    let ready = shared.database.is_some() && shared.ffmpeg_ok;
    // Loading a model takes a moment: off the async runtime.
    let candidate = settings.clone();
    let (analyzer, error) = tokio::task::spawn_blocking(move || start_ai(&candidate, ready))
        .await
        .map_err(|e| ControlError::Internal(e.to_string()))?;
    if settings.provider.is_some()
        && let Some(e) = &error
    {
        return Err(ControlError::Invalid(e.clone()));
    }
    ai_store::save(&path, &settings)
        .map_err(|e| ControlError::Internal(format!("cannot write {}: {e}", path.display())))?;
    *shared.ai.write().unwrap_or_else(|e| e.into_inner()) = analyzer;
    *shared.ai_settings.lock().unwrap_or_else(|e| e.into_inner()) = (settings, error);
    if let Some(api) = shared.api.get() {
        api.set_ai_provider(shared.ai().map(|a| a.provider().to_owned()));
    }
    info!(provider = ?shared.ai().map(|a| a.provider().to_owned()), "object recognition settings changed from the web interface");
    // MQTT announces or removes the « person » sensors.
    let _ = shared.engine.events.send(NvrEvent::CamerasChanged);
    Ok(())
}

/// A server may load its model on the first call: longer than analyses.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Checks a recognition server (not stored) with a test pattern.
pub(super) async fn ai_probe(input: AiProbeInput) -> AiProbeResult {
    let failed = |error: String| AiProbeResult {
        ok: false,
        ms: 0,
        device: None,
        error: Some(error),
    };
    let api_key = match input.api_key_env.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(var) => match std::env::var(var) {
            Ok(key) => Some(rustnvr_core::Secret::new(key)),
            Err(_) => {
                return failed(format!(
                    "la variable d'environnement {var} (clé d'API) n'existe pas dans le conteneur"
                ));
            }
        },
    };
    let started = std::time::Instant::now();
    let result = rustnvr_ai::probe(input.url.trim(), api_key, PROBE_TIMEOUT).await;
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(report) => AiProbeResult {
            ok: true,
            ms: report.ms,
            device: report.device,
            error: None,
        },
        Err(e) => AiProbeResult {
            ms,
            ..failed(probe_error(&e))
        },
    }
}

/// Why a probe failed, with what to check.
fn probe_error(e: &rustnvr_ai::AiError) -> String {
    use rustnvr_ai::AiError;
    match e {
        AiError::Unreachable(detail) if detail.contains("404") => {
            "aucune détection d'objets à cette adresse (404) : est-ce bien CodeProject.AI, avec un module « Object Detection » démarré ?".to_owned()
        }
        // Something answered, with an HTTP error: not a recognition server.
        AiError::Unreachable(detail) if detail.contains("HTTP status") => format!(
            "un serveur répond, mais pas comme CodeProject.AI ({detail}) : vérifiez le port (32168 par défaut)"
        ),
        AiError::Unreachable(detail) => format!(
            "serveur injoignable ({detail}) : vérifiez l'adresse, le port et que CodeProject.AI est démarré"
        ),
        AiError::Provider(msg) => format!(
            "le serveur répond, mais la détection échoue : {msg} (module de détection installé et démarré ?)"
        ),
        AiError::Timeout(d) => format!(
            "pas de réponse en {} s : le premier appel peut charger le modèle, réessayez",
            d.as_secs()
        ),
    }
}

/// Analyses a current picture of `camera` with the provider in effect.
pub(super) async fn ai_test(
    shared: &Arc<Shared>,
    raw_id: &str,
) -> Result<AiTestResult, ControlError> {
    let ai = shared.ai().ok_or_else(|| {
        ControlError::Invalid("la reconnaissance d'objets n'est pas active".into())
    })?;
    let camera = CameraId::new(raw_id)
        .map_err(|_| ControlError::NotFound(format!("aucune caméra {raw_id:?}")))?;
    let tap = shared
        .state
        .lock()
        .await
        .running
        .get(&camera)
        .map(|rt| rt.detection_tap())
        .ok_or_else(|| ControlError::NotFound(format!("caméra {raw_id:?} non démarrée")))?;
    let wanted = camera_settings(shared, &camera).await;
    let jpeg = snapshot_jpeg(&shared.ffmpeg, &tap, THUMBNAIL_WIDTH, &wanted.mask_rects())
        .await
        .map_err(|e| ControlError::Invalid(format!("pas d'image de la caméra : {e}")))?;
    let started = std::time::Instant::now();
    let found = ai
        .detect_all(&jpeg)
        .await
        .map_err(|e| ControlError::Invalid(e.to_string()))?;
    let ms = started.elapsed().as_millis() as u64;
    use base64::Engine as _;
    Ok(AiTestResult {
        ms,
        device: ai.device(),
        objects: found
            .into_iter()
            .filter(|o| o.score >= 0.2)
            .map(|mut o| {
                let located = wanted.locate(&o);
                let kept = ai.keeps(&o) && located.is_some();
                o.zone = located.flatten();
                AiTestObject { kept, object: o }
            })
            .collect(),
        image: format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&jpeg)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::{CrossingDirection, CrossingLine};

    #[test]
    fn finds_the_object_at_the_line() {
        // A gate across the bottom of the picture.
        let gate = CrossingLine {
            name: "portail".into(),
            a: [0.2, 0.7],
            b: [0.8, 0.7],
            direction: CrossingDirection::Both,
            objects: vec!["person".into()],
            presence: false,
        };
        let person = |bbox| DetectedObject {
            label: "person".into(),
            score: 0.9,
            bbox: Some(bbox),
            zone: None,
        };
        // Feet on the gate.
        assert!(at_line(&person([0.4, 0.3, 0.5, 0.72]), &gate));
        // Far away, in the top corner.
        assert!(!at_line(&person([0.0, 0.0, 0.1, 0.2]), &gate));
        // Beyond the end of the gate: measured to its end.
        assert!((distance_to_segment([1.0, 0.7], gate.a, gate.b) - 0.2).abs() < 1e-6);
        // No box: cannot tell, counts.
        let unboxed = DetectedObject {
            bbox: None,
            ..person([0.0; 4])
        };
        assert!(at_line(&unboxed, &gate));
    }
}
