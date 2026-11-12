//! Protected recordings: periods kept whatever the retention and the
//! free-space reserve (`GET/POST /protected`, `DELETE /protected/{id}`).

use crate::routes::ApiError;
use crate::state::{ApiState, unix_ms};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use rustnvr_storage::{Database, ProtectedRange};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Longest protected period: a forgotten protection must not fill the disk.
const MAX_PROTECTED_MS: i64 = 31 * 86_400_000;
const MAX_NOTE: usize = 200;

/// A protected period.
#[derive(Serialize)]
pub(crate) struct ProtectedDto {
    id: i64,
    camera: String,
    start_ms: i64,
    end_ms: i64,
    note: String,
    created_ms: i64,
}

impl From<ProtectedRange> for ProtectedDto {
    fn from(p: ProtectedRange) -> Self {
        Self {
            id: p.id,
            camera: p.camera,
            start_ms: p.start_ms,
            end_ms: p.end_ms,
            note: p.note,
            created_ms: p.created_ms,
        }
    }
}

/// Body of `POST /protected`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProtectInput {
    camera: String,
    start_ms: i64,
    end_ms: i64,
    #[serde(default)]
    note: String,
}

fn database(state: &ApiState) -> Result<&Database, ApiError> {
    state.database.get().ok_or_else(|| {
        ApiError::unavailable("base de données indisponible : protection impossible")
    })
}

/// `GET /protected`: every protected period, most recent first.
pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
) -> Result<Json<Vec<ProtectedDto>>, ApiError> {
    let list = database(&state)?
        .protected_ranges()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        list.into_iter()
            .filter(|p| crate::auth::sees(principal.as_ref(), &p.camera))
            .map(ProtectedDto::from)
            .collect(),
    ))
}

/// `POST /protected`: protects a period of a camera.
pub(crate) async fn protect(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<ProtectInput>,
) -> Result<(StatusCode, Json<ProtectedDto>), ApiError> {
    if state.camera(&input.camera).is_none() {
        return Err(ApiError::not_found(format!(
            "aucune caméra {:?}",
            input.camera
        )));
    }
    if input.end_ms <= input.start_ms {
        return Err(ApiError::bad_request("la fin doit être après le début"));
    }
    if input.end_ms - input.start_ms > MAX_PROTECTED_MS {
        return Err(ApiError::bad_request("31 jours au plus par protection"));
    }
    let note = input.note.trim();
    if note.chars().count() > MAX_NOTE {
        return Err(ApiError::bad_request(
            "note trop longue (200 caractères au plus)",
        ));
    }
    let p = database(&state)?
        .protect(
            &input.camera,
            input.start_ms,
            input.end_ms,
            note,
            unix_ms(std::time::SystemTime::now()),
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    tracing::info!(camera = %p.camera, id = p.id, seconds = (p.end_ms - p.start_ms) / 1000, "recordings protected");
    Ok((StatusCode::CREATED, Json(p.into())))
}

/// `DELETE /protected/{id}`: the period goes back under the retention.
pub(crate) async fn unprotect(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if database(&state)?
        .unprotect(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
    {
        tracing::info!(id, "recordings no longer protected");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("aucune protection {id}")))
    }
}
