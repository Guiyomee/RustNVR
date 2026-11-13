//! Kept pictures of the object recognition and its replay on them
//! (`ai.keep_pictures`): listed and shown to every account, deleted and
//! replayed by administrators (POST / DELETE).

use crate::control::{CaptureDay, CapturePicture, ReplayResult};
use crate::routes::{ApiError, control};
use crate::state::ApiState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// `GET /ai/captures`: days with kept pictures, the most recent first.
pub(crate) async fn days(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
) -> Result<Json<Vec<CaptureDay>>, ApiError> {
    let mut days = control(&state)?.captures().await?;
    days.retain(|d| crate::auth::sees(principal.as_ref(), &d.camera));
    Ok(Json(days))
}

/// `GET /ai/captures/{camera}/{day}`: the kept pictures of a day.
pub(crate) async fn pictures(
    State(state): State<Arc<ApiState>>,
    Path((camera, day)): Path<(String, String)>,
) -> Result<Json<Vec<CapturePicture>>, ApiError> {
    Ok(Json(control(&state)?.capture_pictures(camera, day).await?))
}

/// `GET /ai/captures/{camera}/{day}/{file}`: one kept picture (JPEG).
pub(crate) async fn picture(
    State(state): State<Arc<ApiState>>,
    Path((camera, day, file)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let jpeg = control(&state)?.capture_picture(camera, day, file).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        jpeg,
    )
        .into_response())
}

/// Answer of `DELETE /ai/captures/{camera}/{day}`.
#[derive(Serialize)]
pub(crate) struct Deleted {
    deleted: usize,
}

/// `DELETE /ai/captures/{camera}/{day}`: deletes the kept pictures of a day.
pub(crate) async fn delete(
    State(state): State<Arc<ApiState>>,
    Path((camera, day)): Path<(String, String)>,
) -> Result<Json<Deleted>, ApiError> {
    let deleted = control(&state)?.delete_captures(camera, day).await?;
    Ok(Json(Deleted { deleted }))
}

/// `POST /ai/replay` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplayRequest {
    camera: String,
    day: String,
    /// Only this picture (absent: the whole day).
    #[serde(default)]
    file: Option<String>,
}

/// `POST /ai/replay`: analyses the kept pictures of a day (or one of them)
/// again, with the recognition settings in use now.
pub(crate) async fn replay(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<ReplayRequest>,
) -> Result<Json<ReplayResult>, ApiError> {
    Ok(Json(
        control(&state)?
            .replay(input.camera, input.day, input.file)
            .await?,
    ))
}
