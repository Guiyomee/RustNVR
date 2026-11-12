//! Route table and REST handlers of `/api/v1`.

use crate::control::{CameraInput, ControlError, DestinationInput, NvrControl};
use crate::dto::{
    CameraDto, CapabilitiesDto, DestinationDto, ErrorBody, ErrorDetail, ProbeDto, ProbedStreamDto,
    ProbedVideoDto, RecordingDto, StorageDto, SystemDto,
};
use crate::recordings::{self, MAX_RANGE_MS, parse_segment_name, valid_day};
use crate::state::ApiState;
use crate::{auth, detection, live, storage, ws};
use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Router, middleware};
use rustnvr_core::config::RtspTransport;
use rustnvr_core::{CameraId, RtspTarget, Secret};
use rustnvr_stream::{SessionOptions, probe};
use serde::Deserialize;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt as _;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

const STORAGE_CACHE: Duration = Duration::from_secs(30);

/// An API error rendered as `{"error": {...}}`.
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }
    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

/// `GET /api/v1/health`: answers while the server runs (container
/// health check): `{"status":"ok"}`. Public and reveals nothing.
async fn health() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"ok"}"#,
    )
}

async fn capabilities(State(state): State<Arc<ApiState>>) -> Json<CapabilitiesDto> {
    Json(CapabilitiesDto {
        api_version: 1,
        live_video: true,
        live_transcode: state.live_settings().transcode,
        discovery: true,
        live_webrtc: true,
        restream_port: state.restream().map(|(port, _)| port),
        restream_local_only: state.restream().is_some_and(|(_, local)| local),
        snapshots: state.ffmpeg_ok(),
        events: state.database.get().is_some(),
        motion_detection: state.database.get().is_some()
            && state.control.get().is_some()
            && state.ffmpeg_ok(),
        camera_management: state.control.get().is_some(),
        camera_probe: true,
        recordings: true,
        export: state.ffmpeg_ok(),
        protect: state.database.get().is_some(),
        retention: state.control.get().is_some(),
        settings_write: state.control.get().is_some(),
        mqtt: state.control.get().is_some(),
        ai: state
            .ai_provider
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some(),
        users: state.database.get().is_some(),
        audit: state.database.get().is_some(),
        backup: state.control.get().is_some(),
        ptz: state.control.get().is_some(),
        usb_cameras: cfg!(target_os = "linux") && state.control.get().is_some(),
    })
}

async fn system(State(state): State<Arc<ApiState>>) -> Json<SystemDto> {
    Json(state.system_dto())
}

/// `GET /cameras`: the cameras this account sees.
async fn cameras(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<auth::Principal>>,
) -> Json<Vec<CameraDto>> {
    Json(
        state
            .cameras()
            .iter()
            .filter(|c| auth::sees(principal.as_ref(), c.id.as_str()))
            .map(|c| state.camera_dto(c))
            .collect(),
    )
}

pub(crate) fn control(state: &ApiState) -> Result<Arc<dyn NvrControl>, ApiError> {
    state
        .control
        .get()
        .cloned()
        .ok_or_else(|| ApiError::unavailable("camera management is not available on this server"))
}

impl From<ControlError> for ApiError {
    fn from(e: ControlError) -> Self {
        let (status, code) = match &e {
            ControlError::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ControlError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ControlError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            ControlError::ReadOnly(_) => (StatusCode::CONFLICT, "read_only"),
            ControlError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        ApiError::new(status, code, e.to_string())
    }
}

fn camera_dto_or_404(state: &ApiState, id: &str) -> Result<Json<CameraDto>, ApiError> {
    let c = state
        .camera(id)
        .ok_or_else(|| ApiError::not_found(format!("no camera {id:?}")))?;
    Ok(Json(state.camera_dto(&c)))
}

/// `POST /cameras`: JSON body only, so that a page from another site cannot
/// submit it without a CORS preflight (which this server never grants).
async fn create_camera(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<CameraInput>,
) -> Result<(StatusCode, Json<CameraDto>), ApiError> {
    let id = input.id.clone().unwrap_or_default();
    control(&state)?.create(input).await?;
    Ok((StatusCode::CREATED, camera_dto_or_404(&state, &id)?))
}

async fn update_camera(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(input): Json<CameraInput>,
) -> Result<Json<CameraDto>, ApiError> {
    control(&state)?.update(id.clone(), input).await?;
    camera_dto_or_404(&state, &id)
}

async fn delete_camera(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    control(&state)?.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Connection test parameters (`POST /cameras/probe`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeInput {
    url: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<Secret>,
    #[serde(default)]
    transport: RtspTransport,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

async fn run_probe(
    state: &ApiState,
    target: RtspTarget,
    mut opts: SessionOptions,
) -> Result<Json<ProbeDto>, ApiError> {
    opts.connect_timeout = PROBE_TIMEOUT;
    opts.stall_timeout = PROBE_TIMEOUT;
    // Video only for the preview (as for the recording when sound is off).
    let preview_target = (
        target.clone(),
        SessionOptions {
            audio: false,
            ..opts.clone()
        },
    );
    let report = probe(&target, &opts, true).await.map_err(|e| {
        let message = match e.status_code() {
            Some(401) => "la caméra refuse l'identifiant ou le mot de passe (401)".to_owned(),
            Some(404) => "chemin RTSP inconnu de la caméra (404)".to_owned(),
            _ => e.summary(),
        };
        ApiError::new(StatusCode::BAD_GATEWAY, "probe_failed", message)
    })?;
    let preview = (report.video.is_some() && state.ffmpeg_ok())
        .then(|| {
            state.previews.register(crate::preview::PreviewSource::Rtsp(
                preview_target.0,
                preview_target.1,
            ))
        })
        .flatten()
        .map(|token| format!("/api/v1/cameras/preview/{token}"));
    Ok(Json(ProbeDto {
        preview,
        streams: report
            .streams
            .into_iter()
            .map(|s| ProbedStreamDto {
                media: s.media,
                encoding: s.encoding,
                clock_rate: s.clock_rate,
            })
            .collect(),
        selected: report.selected,
        video: report.video.map(|v| ProbedVideoDto {
            codec: v.codec.to_string(),
            rfc6381_codec: v.rfc6381_codec,
            width: v.width,
            height: v.height,
            time_to_key_frame_ms: u64::try_from(v.time_to_key_frame.as_millis())
                .unwrap_or(u64::MAX),
        }),
    }))
}

async fn probe_url(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<ProbeInput>,
) -> Result<Json<ProbeDto>, ApiError> {
    let target = RtspTarget::parse(&input.url, input.username.as_deref(), input.password)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let opts = SessionOptions {
        transport: input.transport,
        ..SessionOptions::default()
    };
    run_probe(&state, target, opts).await
}

async fn probe_camera(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<ProbeDto>, ApiError> {
    let (target, opts) = control(&state)?
        .target(id.clone())
        .await
        .ok_or_else(|| ApiError::not_found(format!("no camera {id:?}")))?;
    run_probe(&state, target, opts).await
}

async fn camera(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<CameraDto>, ApiError> {
    camera_dto_or_404(&state, &id)
}

#[derive(Deserialize)]
pub(crate) struct RangeQuery {
    pub(crate) from: i64,
    pub(crate) to: i64,
}

pub(crate) async fn camera_recordings(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<Vec<RecordingDto>>, ApiError> {
    if state.camera(&id).is_none() {
        return Err(ApiError::not_found(format!("no camera {id:?}")));
    }
    if q.to <= q.from || q.to - q.from > MAX_RANGE_MS {
        return Err(ApiError::bad_request(
            "`to` must be after `from`, at most 7 days later",
        ));
    }
    if let Some(list) = recordings::list_indexed(&state, &id, q.from, q.to).await {
        return Ok(Json(list));
    }
    let list = tokio::task::spawn_blocking(move || recordings::list(&state, &id, q.from, q.to))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(list))
}

/// `GET /cameras/{id}/playlist.m3u8?from=&to=`: HLS playlist of the recordings
/// of the period, for gapless playback (and Safari / iPhone).
async fn camera_playlist(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> Result<Response, ApiError> {
    let Json(list) = camera_recordings(State(state.clone()), Path(id), Query(q)).await?;
    let roots: Vec<(String, std::path::PathBuf)> = state
        .destinations()
        .into_iter()
        .map(|d| (d.id, d.root))
        .collect();
    let segments = tokio::task::spawn_blocking(move || {
        list.into_iter()
            .filter_map(|r| {
                // `/api/v1/recordings/<dest>/<camera>/<day>/<file>`
                let rest = r.url.strip_prefix("/api/v1/recordings/")?;
                let (dest, path) = rest.split_once('/')?;
                let root = &roots.iter().find(|(d, _)| d == dest)?.1;
                let init = recordings::init_length(&root.join(path))?;
                Some((r, init))
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    if segments.is_empty() {
        return Err(ApiError::not_found("no recording in this period"));
    }
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        recordings::hls_playlist(&segments),
    )
        .into_response())
}

/// `GET /storage`. An account limited to some cameras gets the totals of
/// the disks, and the detail of its cameras only.
async fn storage(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<auth::Principal>>,
) -> Result<Json<StorageDto>, ApiError> {
    let limit = |mut dto: StorageDto| {
        if auth::scope(principal.as_ref()).is_some() {
            let sees = |camera: &String| auth::sees(principal.as_ref(), camera);
            dto.cameras.retain(|c| sees(&c.camera));
            for d in &mut dto.destinations {
                d.used_by.retain(sees);
            }
        }
        Json(dto)
    };
    let mut cache = state.storage_cache.lock().await;
    if let Some((at, dto)) = cache.as_ref()
        && at.elapsed() < STORAGE_CACHE
    {
        // The default may have changed since: not cached.
        let mut dto = dto.clone();
        dto.default_destination = state.default_destination();
        return Ok(limit(dto));
    }
    let s = state.clone();
    let dto = tokio::task::spawn_blocking(move || storage::compute(&s))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    *cache = Some((Instant::now(), dto.clone()));
    Ok(limit(dto))
}

/// `GET /storage/destinations`; a viewer gets no account of a remote
/// destination (S3 access key, SFTP user).
async fn list_destinations(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
) -> Result<Json<Vec<DestinationDto>>, ApiError> {
    let Json(mut list) = destinations_list(State(state)).await?;
    if crate::auth::is_viewer(principal.as_ref()) {
        for remote in list.iter_mut().filter_map(|d| d.remote.as_mut()) {
            if let Some(fields) = remote.as_object_mut() {
                fields.remove("access_key_id");
                fields.remove("username");
            }
        }
    }
    Ok(Json(list))
}

async fn destinations_list(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<DestinationDto>>, ApiError> {
    // Availability checks touch the filesystems (possibly network ones).
    let s = state.clone();
    let list = tokio::task::spawn_blocking(move || storage::destinations(&s))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(list))
}

async fn after_storage_change(
    state: &Arc<ApiState>,
) -> Result<Json<Vec<DestinationDto>>, ApiError> {
    *state.storage_cache.lock().await = None;
    destinations_list(State(state.clone())).await
}

async fn create_destination(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<DestinationInput>,
) -> Result<(StatusCode, Json<Vec<DestinationDto>>), ApiError> {
    control(&state)?.create_destination(input).await?;
    Ok((StatusCode::CREATED, after_storage_change(&state).await?))
}

async fn update_destination(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(input): Json<DestinationInput>,
) -> Result<Json<Vec<DestinationDto>>, ApiError> {
    control(&state)?.update_destination(id, input).await?;
    after_storage_change(&state).await
}

async fn delete_destination(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<DestinationDto>>, ApiError> {
    control(&state)?.delete_destination(id).await?;
    after_storage_change(&state).await
}

/// Serves a segment file, with Range support for seeking in `<video>`.
async fn recording_file(
    State(state): State<Arc<ApiState>>,
    Path((dest, camera, day, file)): Path<(String, String, String, String)>,
    req: Request,
) -> Result<Response, ApiError> {
    // Every component is validated, so the path cannot leave the recordings tree.
    if CameraId::new(dest.as_str()).is_err()
        || CameraId::new(camera.as_str()).is_err()
        || !valid_day(&day)
        || parse_segment_name(&file).is_none()
    {
        return Err(ApiError::bad_request("invalid recording path"));
    }
    if state.camera(&camera).is_none() {
        return Err(ApiError::not_found(format!("no camera {camera:?}")));
    }
    let root = state
        .destinations()
        .into_iter()
        .find(|d| d.id == dest)
        .map(|d| d.root)
        .ok_or_else(|| ApiError::not_found(format!("no storage destination {dest:?}")))?;
    let mut path = root.join(&camera).join(&day).join(&file);
    if !path.is_file() {
        return Err(ApiError::not_found("no such recording"));
    }
    // `?transcode=h264`: an H.264 copy, for browsers without H.265.
    let transcode = req
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == "transcode=h264"));
    if transcode {
        let key = format!("{dest}_{camera}_{}", file.trim_end_matches(".mp4"));
        path = state.playback.get(&state, &path, &key).await?;
    }
    // Serving a file never fails (`Infallible`): a missing file is a 404.
    let Ok(response) = ServeFile::new(path).oneshot(req).await;
    Ok(response.into_response())
}

async fn api_not_found(req: Request) -> ApiError {
    ApiError::not_found(format!("no API route {}", req.uri().path()))
}

async fn frontend_missing() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "RustNVR is running, but the web interface has not been built.\n\
         Build it with: cd web && npm ci && npm run build\n\
         The API is available under /api/v1/.\n",
    )
        .into_response()
}

/// Caching of the interface: built files (`/assets/`, names containing a
/// hash) never change; everything else (`index.html`) is revalidated, so a
/// rebuilt interface is used at once instead of pointing to deleted files.
async fn interface_cache(req: Request, next: middleware::Next) -> Response {
    let assets = req.uri().path().starts_with("/assets/");
    let mut res = next.run(req).await;
    let html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"text/html"));
    if assets && html {
        // A missing built file: a 404, not the interface page (which the
        // browser would fail to run as a script, and cache).
        return (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-cache")]).into_response();
    }
    let value = if assets && res.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    res
}

/// What the interface may load: only this server (a script injected some
/// day could neither load code nor send data elsewhere). Video uses `blob:`
/// URLs (media sources, the HLS worker); React sets style attributes.
const CONTENT_SECURITY_POLICY: &str = concat!(
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; ",
    "img-src 'self' data: blob:; media-src 'self' blob:; worker-src 'self' blob:; ",
    "connect-src 'self'; font-src 'self' data:; object-src 'none'; base-uri 'none'; ",
    "form-action 'self'; frame-ancestors 'none'"
);

/// Builds the application: `/api/v1/*`, then the static interface with a
/// fallback to `index.html` for client-side routes. Everything requires
/// authentication when it is configured.
pub fn router(state: Arc<ApiState>, static_dir: &FsPath) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/capabilities", get(capabilities))
        .route("/system", get(system))
        .route("/cameras", get(cameras).post(create_camera))
        .route("/cameras/probe", post(probe_url))
        .route("/cameras/preview/{token}", get(crate::preview::preview))
        .route("/cameras/usb/preview", post(crate::preview::usb_token))
        .route("/discovery/scan", post(crate::discovery::scan_network))
        .route("/discovery/streams", post(crate::discovery::streams))
        .route(
            "/cameras/{id}",
            get(camera).put(update_camera).delete(delete_camera),
        )
        .route("/cameras/{id}/probe", post(probe_camera))
        .route("/cameras/{id}/recordings", get(camera_recordings))
        .route("/cameras/{id}/export", post(crate::export::prepare))
        .route("/exports/{file}", get(crate::export::download))
        .route("/cameras/{id}/playlist.m3u8", get(camera_playlist))
        .route("/cameras/{id}/live", get(live::handler))
        .route("/cameras/{id}/live/webrtc", post(live::webrtc))
        .route("/storage", get(storage))
        .route(
            "/recordings/{dest}/{camera}/{day}/{file}",
            get(recording_file),
        )
        .route(
            "/storage/destinations",
            get(list_destinations).post(create_destination),
        )
        .route(
            "/storage/destinations/{id}",
            axum::routing::put(update_destination).delete(delete_destination),
        )
        .route("/events", get(detection::events))
        .route("/events/stats", get(crate::stats::stats))
        .route("/crossings", get(crate::stats::crossings))
        .route("/events/{id}", get(detection::event))
        .route("/events/{id}/thumbnail", get(detection::thumbnail))
        .route("/cameras/{id}/snapshot", get(detection::snapshot))
        .route(
            "/cameras/{id}/motion",
            get(detection::motion).put(detection::set_motion),
        )
        .route(
            "/cameras/{id}/audio",
            axum::routing::put(detection::set_audio),
        )
        .route(
            "/cameras/{id}/pause",
            axum::routing::put(detection::set_camera_paused),
        )
        .route(
            "/detection/pause",
            axum::routing::put(detection::set_detection_paused),
        )
        .route(
            "/cameras/{id}/presence/reset",
            post(crate::stats::reset_presence),
        )
        .route("/devices/video", get(crate::devices::list))
        .route("/cameras/{id}/ptz", get(crate::ptz::presets))
        .route("/cameras/{id}/ptz/move", post(crate::ptz::move_camera))
        .route("/cameras/{id}/ptz/stop", post(crate::ptz::stop))
        .route("/cameras/{id}/reboot", post(crate::ptz::reboot))
        .route("/cameras/{id}/device", get(crate::ptz::device))
        .route("/cameras/{id}/ptz/presets", post(crate::ptz::create_preset))
        .route(
            "/cameras/{id}/ptz/presets/{token}",
            axum::routing::delete(crate::ptz::delete_preset),
        )
        .route(
            "/cameras/{id}/ptz/presets/{token}/goto",
            post(crate::ptz::goto_preset),
        )
        .route("/mqtt", get(detection::mqtt).put(detection::set_mqtt))
        .route("/ai", get(detection::ai).put(detection::set_ai))
        .route("/ai/test", post(detection::ai_test))
        .route("/ai/probe", post(detection::ai_probe))
        .route("/ai/models/{id}", post(detection::ai_download_model))
        .route("/ai/captures", get(crate::replay::days))
        .route(
            "/ai/captures/{camera}/{day}",
            get(crate::replay::pictures).delete(crate::replay::delete),
        )
        .route(
            "/ai/captures/{camera}/{day}/{file}",
            get(crate::replay::picture),
        )
        .route("/ai/replay", post(crate::replay::replay))
        .route(
            "/updates",
            get(detection::updates).put(detection::set_updates),
        )
        .route("/updates/check", post(detection::check_updates))
        .route(
            "/webhooks",
            get(detection::webhooks).put(detection::set_webhooks),
        )
        .route("/webhooks/test", post(detection::test_webhook))
        .route("/backup", post(crate::backup::create))
        .route("/backup/inspect", post(crate::backup::inspect))
        .route("/backup/restore", post(crate::backup::restore))
        .route(
            "/protected",
            get(crate::protect::list).post(crate::protect::protect),
        )
        .route(
            "/protected/{id}",
            axum::routing::delete(crate::protect::unprotect),
        )
        .route("/logs", get(detection::logs))
        .route(
            "/settings",
            get(detection::settings).put(detection::set_settings),
        )
        .route("/auth/status", get(auth::status))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/setup", post(auth::setup))
        .route("/auth/password", axum::routing::put(auth::change_password))
        .route("/users", get(auth::users).post(auth::create_user))
        .route(
            "/users/{name}",
            axum::routing::put(auth::update_user).delete(auth::delete_user),
        )
        .route("/audit", get(auth::audit))
        .route("/ws", get(ws::handler))
        .fallback(api_not_found);

    let index = static_dir.join("index.html");
    let app = Router::new().nest("/api/v1", api);
    let app = if index.is_file() {
        app.fallback_service(ServeDir::new(static_dir).fallback(ServeFile::new(index)))
            .layer(middleware::from_fn(interface_cache))
    } else {
        app.fallback(frontend_missing)
    };
    app.layer(middleware::from_fn_with_state(
        state.clone(),
        auth::require_auth,
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    ))
    .with_state(state)
}
