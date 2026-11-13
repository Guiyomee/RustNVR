//! Events, snapshots, motion detection settings and MQTT settings.

use crate::control::{
    AiModelView, AiProbeInput, AiProbeResult, AiTestResult, AiView, MqttInput, MqttView,
};
use crate::dto::EventDto;
use crate::routes::{ApiError, control};
use crate::state::{ApiState, unix_ms};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use rustnvr_core::config::AiConfig;
use rustnvr_core::{CameraId, MotionSettings};
use rustnvr_storage::{Database, EventFilter, MAX_EVENTS_PER_QUERY};
use serde::Deserialize;
use std::sync::Arc;
use std::time::SystemTime;

/// Default period of `GET /events` without `from`.
const DEFAULT_PERIOD_MS: i64 = 24 * 3600 * 1000;
/// Widest period of one query.
const MAX_PERIOD_MS: i64 = 31 * 24 * 3600 * 1000;
/// Width of snapshots served by the API.
const SNAPSHOT_MAX_WIDTH: u32 = 1920;

fn database(state: &ApiState) -> Result<&Database, ApiError> {
    state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("events are not available on this server"))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %e, "database error");
    ApiError::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        e.to_string(),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsQuery {
    camera: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    limit: Option<u32>,
}

/// `GET /events?camera=&from=&to=&limit=`: events overlapping the period, newest first.
pub(crate) async fn events(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Vec<EventDto>>, ApiError> {
    let db = database(&state)?;
    let to = q.to.unwrap_or_else(|| unix_ms(SystemTime::now()));
    let from = q.from.unwrap_or(to - DEFAULT_PERIOD_MS);
    if to <= from || to - from > MAX_PERIOD_MS {
        return Err(ApiError::bad_request(
            "`to` must be after `from`, at most 31 days later",
        ));
    }
    let camera = q
        .camera
        .filter(|c| !c.is_empty())
        .map(|c| CameraId::new(c).map_err(|e| ApiError::bad_request(e.to_string())))
        .transpose()?;
    let list = db
        .events(&EventFilter {
            camera,
            cameras: crate::auth::scope(principal.as_ref()),
            from_ms: from,
            to_ms: to,
            limit: q.limit.unwrap_or(200).min(MAX_EVENTS_PER_QUERY),
        })
        .await
        .map_err(internal)?;
    Ok(Json(list.iter().map(EventDto::from).collect()))
}

fn event_id(id: &str) -> Result<i64, ApiError> {
    id.parse()
        .map_err(|_| ApiError::bad_request("invalid event identifier"))
}

/// `GET /events/{id}`.
pub(crate) async fn event(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Path(id): Path<String>,
) -> Result<Json<EventDto>, ApiError> {
    let db = database(&state)?;
    let e = db
        .event(event_id(&id)?)
        .await
        .map_err(internal)?
        // An event of a camera this account does not see does not exist for it.
        .filter(|e| crate::auth::sees(principal.as_ref(), e.camera.as_str()))
        .ok_or_else(|| ApiError::not_found(format!("no event {id}")))?;
    Ok(Json(EventDto::from(&e)))
}

fn jpeg(body: Vec<u8>, cache: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, cache),
        ],
        body,
    )
        .into_response()
}

/// `GET /events/{id}/thumbnail`: JPEG taken when the event started.
pub(crate) async fn thumbnail(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let db = database(&state)?;
    if crate::auth::scope(principal.as_ref()).is_some() {
        db.event(event_id(&id)?)
            .await
            .map_err(internal)?
            .filter(|e| crate::auth::sees(principal.as_ref(), e.camera.as_str()))
            .ok_or_else(|| ApiError::not_found(format!("no thumbnail for event {id}")))?;
    }
    let path = db
        .thumbnail_path(event_id(&id)?)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found(format!("no thumbnail for event {id}")))?;
    let body = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::not_found(format!("no thumbnail for event {id}")))?;
    // Thumbnails never change once written.
    Ok(jpeg(body, "private, max-age=86400, immutable"))
}

/// Other parameters are ignored: browsers add one to bypass their cache.
#[derive(Deserialize)]
pub(crate) struct SnapshotQuery {
    stream: Option<String>,
}

/// `GET /cameras/{id}/snapshot?stream=main|sub`: current picture as a JPEG.
pub(crate) async fn snapshot(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Query(q): Query<SnapshotQuery>,
) -> Result<Response, ApiError> {
    if !state.ffmpeg_ok() {
        return Err(ApiError::unavailable(
            "snapshots need FFmpeg, which could not be started (see live.ffmpeg)",
        ));
    }
    let c = state
        .camera(&id)
        .ok_or_else(|| ApiError::not_found(format!("no camera {id:?}")))?;
    let tap = match q.stream.as_deref() {
        Some("sub") => c.live_sub.or(c.live),
        Some("main") | None => c.live.or(c.live_sub),
        Some(other) => return Err(ApiError::bad_request(format!("unknown stream {other:?}"))),
    }
    .ok_or_else(|| ApiError::not_found(format!("camera {id:?} is not connected")))?;
    let picture = rustnvr_motion::snapshot_jpeg(
        &state.live_settings().ffmpeg,
        &tap,
        SNAPSHOT_MAX_WIDTH,
        &c.privacy_masks,
    )
    .await
    .map_err(ApiError::unavailable)?;
    Ok(jpeg(picture.to_vec(), "no-store"))
}

/// `GET /cameras/{id}/motion`.
pub(crate) async fn motion(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<MotionSettings>, ApiError> {
    Ok(Json(control(&state)?.motion(id).await?))
}

/// `PUT /cameras/{id}/motion`: stores the settings and restarts detection.
pub(crate) async fn set_motion(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(settings): Json<MotionSettings>,
) -> Result<Json<MotionSettings>, ApiError> {
    let problems = settings.problems();
    if !problems.is_empty() {
        return Err(ApiError::bad_request(problems.join("; ")));
    }
    if settings.enabled && !state.ffmpeg_ok() {
        return Err(ApiError::unavailable(
            "la détection de mouvement nécessite FFmpeg, qui n'a pas pu être lancé (voir live.ffmpeg)",
        ));
    }
    let control = control(&state)?;
    control.set_motion(id.clone(), settings).await?;
    Ok(Json(control.motion(id).await?))
}

/// Body of `PUT /cameras/{id}/audio`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioInput {
    enabled: bool,
}

/// `PUT /cameras/{id}/audio`: records the sound of the camera, or not.
pub(crate) async fn set_audio(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(input): Json<AudioInput>,
) -> Result<Json<crate::dto::CameraDto>, ApiError> {
    control(&state)?
        .set_audio(id.clone(), input.enabled)
        .await?;
    let camera = state
        .camera(&id)
        .ok_or_else(|| ApiError::not_found(format!("no camera {id:?}")))?;
    Ok(Json(state.camera_dto(&camera)))
}

/// Body of `PUT /cameras/{id}/pause`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CameraPauseInput {
    paused: bool,
}

/// `PUT /cameras/{id}/pause`: pauses a camera until RustNVR restarts, or
/// resumes it.
pub(crate) async fn set_camera_paused(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(input): Json<CameraPauseInput>,
) -> Result<Json<crate::dto::CameraDto>, ApiError> {
    control(&state)?
        .set_camera_paused(id.clone(), input.paused)
        .await?;
    let camera = state
        .camera(&id)
        .ok_or_else(|| ApiError::not_found(format!("no camera {id:?}")))?;
    Ok(Json(state.camera_dto(&camera)))
}

/// Body of `PUT /detection/pause`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PauseInput {
    paused: bool,
}

/// `PUT /detection/pause`: pauses motion detection on every camera, or
/// resumes it.
pub(crate) async fn set_detection_paused(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<PauseInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    control(&state)?.set_detection_paused(input.paused).await?;
    Ok(Json(serde_json::json!({ "paused": input.paused })))
}

/// `GET /mqtt`.
pub(crate) async fn mqtt(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
) -> Result<Json<MqttView>, ApiError> {
    let mut view = control(&state)?.mqtt().await;
    // A viewer sees the settings, not the broker's account.
    if crate::auth::is_viewer(principal.as_ref()) {
        view.username = None;
    }
    Ok(Json(view))
}

/// `PUT /mqtt`: stores the settings and reconnects.
pub(crate) async fn set_mqtt(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<MqttInput>,
) -> Result<Json<MqttView>, ApiError> {
    let control = control(&state)?;
    control.set_mqtt(input).await?;
    Ok(Json(control.mqtt().await))
}

/// `GET /settings`.
pub(crate) async fn settings(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<crate::control::SettingsView>, ApiError> {
    Ok(Json(control(&state)?.settings().await))
}

/// `PUT /settings`: validates, stores and applies the general settings.
pub(crate) async fn set_settings(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<crate::control::SettingsInput>,
) -> Result<Json<crate::control::SettingsView>, ApiError> {
    let mut problems = input.recording.problems();
    problems.extend(input.motion.problems());
    if !(100..=20_000).contains(&input.live_max_bitrate_kbps) {
        problems.push("live.max_bitrate_kbps must be between 100 and 20000".to_owned());
    }
    if !(50..=99).contains(&input.low_space_percent) {
        problems.push("storage.low_space_percent must be between 50 and 99".to_owned());
    }
    if !problems.is_empty() {
        return Err(ApiError::bad_request(problems.join("; ")));
    }
    let control = control(&state)?;
    control.set_settings(input).await?;
    Ok(Json(control.settings().await))
}

/// `GET /ai`: object recognition settings and state.
pub(crate) async fn ai(State(state): State<Arc<ApiState>>) -> Result<Json<AiView>, ApiError> {
    Ok(Json(control(&state)?.ai().await))
}

/// `PUT /ai`: checks, stores and applies the settings (no restart).
pub(crate) async fn set_ai(
    State(state): State<Arc<ApiState>>,
    Json(settings): Json<AiConfig>,
) -> Result<Json<AiView>, ApiError> {
    let problems = settings.problems(|n| std::env::var(n).ok());
    if !problems.is_empty() {
        return Err(ApiError::bad_request(problems.join("; ")));
    }
    let control = control(&state)?;
    control.set_ai(settings).await?;
    Ok(Json(control.ai().await))
}

/// Query of `POST /ai/test`.
#[derive(Deserialize)]
pub(crate) struct AiTestQuery {
    camera: String,
}

/// `POST /ai/test?camera=…`: analyses a current picture of the camera.
pub(crate) async fn ai_test(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<AiTestQuery>,
) -> Result<Json<AiTestResult>, ApiError> {
    Ok(Json(control(&state)?.ai_test(q.camera).await?))
}

/// `POST /ai/probe`: checks a recognition server (not stored) with a test
/// pattern.
pub(crate) async fn ai_probe(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<AiProbeInput>,
) -> Result<Json<AiProbeResult>, ApiError> {
    if !(input.url.starts_with("http://") || input.url.starts_with("https://")) {
        return Err(ApiError::bad_request(
            "adresse http://… ou https://… attendue",
        ));
    }
    Ok(Json(control(&state)?.ai_probe(input).await))
}

/// `POST /ai/models/{id}`: downloads a model of the built-in recognition.
pub(crate) async fn ai_download_model(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<AiModelView>, ApiError> {
    Ok(Json(control(&state)?.ai_download_model(id).await?))
}

/// Webhook addresses may carry tokens: administrators only, even to read.
fn admin_only(principal: Option<&axum::Extension<crate::auth::Principal>>) -> Result<(), ApiError> {
    if principal.is_some_and(|p| !p.admin()) {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "forbidden",
            "les webhooks sont réservés aux administrateurs",
        ));
    }
    Ok(())
}

/// `GET /webhooks`: webhooks in effect (administrators only).
pub(crate) async fn webhooks(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
) -> Result<Json<Vec<rustnvr_core::config::WebhookConfig>>, ApiError> {
    admin_only(principal.as_ref())?;
    Ok(Json(control(&state)?.webhooks().await))
}

/// `PUT /webhooks`: replaces every webhook.
pub(crate) async fn set_webhooks(
    State(state): State<Arc<ApiState>>,
    Json(hooks): Json<Vec<rustnvr_core::config::WebhookConfig>>,
) -> Result<Json<Vec<rustnvr_core::config::WebhookConfig>>, ApiError> {
    let problems: Vec<String> = hooks.iter().flat_map(|h| h.problems()).collect();
    if !problems.is_empty() {
        return Err(ApiError::bad_request(problems.join("; ")));
    }
    if hooks.len() > 20 {
        return Err(ApiError::bad_request("20 webhooks au plus"));
    }
    let control = control(&state)?;
    control.set_webhooks(hooks).await?;
    Ok(Json(control.webhooks().await))
}

/// `POST /webhooks/test`: calls one webhook with a sample event.
pub(crate) async fn test_webhook(
    State(state): State<Arc<ApiState>>,
    Json(hook): Json<rustnvr_core::config::WebhookConfig>,
) -> Result<Json<crate::control::WebhookTestResult>, ApiError> {
    let problems = hook.problems();
    if !problems.is_empty() {
        return Err(ApiError::bad_request(problems.join("; ")));
    }
    Ok(Json(control(&state)?.test_webhook(hook).await))
}

/// `GET /updates`: last check for new versions.
pub(crate) async fn updates(State(state): State<Arc<ApiState>>) -> Json<crate::dto::UpdateStatus> {
    Json(state.update_status())
}

/// `POST /updates/check`: checks the watched repository now.
pub(crate) async fn check_updates(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<crate::dto::UpdateStatus>, ApiError> {
    Ok(Json(control(&state)?.check_updates().await))
}

/// Body of `PUT /updates`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdatesInput {
    /// Check for new versions (the repository is the official one unless
    /// `config.toml` sets another).
    check: bool,
}

/// `PUT /updates`: turns the check on or off, then checks.
pub(crate) async fn set_updates(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<UpdatesInput>,
) -> Result<Json<crate::dto::UpdateStatus>, ApiError> {
    Ok(Json(control(&state)?.set_updates(input.check).await?))
}

/// Query of `GET /logs`.
#[derive(Deserialize)]
pub(crate) struct LogsQuery {
    /// Only messages after this `seq` (incremental reading).
    since: Option<u64>,
    /// Least severe level shown (`info` by default).
    level: Option<String>,
    camera: Option<String>,
    limit: Option<usize>,
}

/// Body of `GET /logs`.
#[derive(serde::Serialize)]
pub(crate) struct LogsDto {
    entries: Vec<crate::logs::LogEntry>,
    /// Last message written, to pass as `since` next time.
    last_seq: u64,
}

/// `GET /logs`: recent log messages (administrators only).
pub(crate) async fn logs(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<LogsDto>, ApiError> {
    if principal.is_some_and(|p| !p.admin()) {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "forbidden",
            "le journal est réservé aux administrateurs",
        ));
    }
    let buffer = state
        .logs
        .get()
        .ok_or_else(|| ApiError::unavailable("log capture is not enabled"))?;
    let level = match q.level.as_deref() {
        None => crate::logs::LogLevel::Info,
        Some(l) => crate::logs::LogLevel::parse(l)
            .ok_or_else(|| ApiError::bad_request(format!("unknown level {l:?}")))?,
    };
    let (entries, last_seq) = buffer.query(
        q.since.unwrap_or(0),
        level,
        q.camera.as_deref().filter(|c| !c.is_empty()),
        q.limit.unwrap_or(500).min(2000),
    );
    Ok(Json(LogsDto { entries, last_seq }))
}
