//! Bookmarks on the timeline: a moment of a camera with a note
//! (`GET/POST /bookmarks`, `DELETE /bookmarks/{id}`).

use crate::auth::Principal;
use crate::routes::ApiError;
use crate::state::{ApiState, unix_ms};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use rustnvr_storage::{Bookmark, Database};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Longest note.
const MAX_NOTE: usize = 200;
/// Widest period of one listing.
const MAX_PERIOD_MS: i64 = 31 * 86_400_000;
/// Most bookmarks in one listing.
const MAX_LIST: u32 = 2000;

/// A bookmark.
#[derive(Serialize)]
pub(crate) struct BookmarkDto {
    id: i64,
    camera: String,
    at_ms: i64,
    note: String,
    /// Account that added it (`null` without accounts).
    author: Option<String>,
    created_ms: i64,
}

impl From<Bookmark> for BookmarkDto {
    fn from(b: Bookmark) -> Self {
        Self {
            id: b.id,
            camera: b.camera,
            at_ms: b.at_ms,
            note: b.note,
            author: b.author,
            created_ms: b.created_ms,
        }
    }
}

/// `GET /bookmarks` query.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    from: i64,
    to: i64,
    #[serde(default)]
    camera: Option<String>,
}

/// Body of `POST /bookmarks`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BookmarkInput {
    camera: String,
    at_ms: i64,
    note: String,
}

fn database(state: &ApiState) -> Result<&Database, ApiError> {
    state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("base de données indisponible : pas de signets"))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(e.to_string())
}

/// `GET /bookmarks?from=&to=[&camera=]`: the bookmarks of a period, oldest
/// first, on the cameras this account sees.
pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    principal: Option<Extension<Principal>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<BookmarkDto>>, ApiError> {
    if q.to <= q.from || q.to - q.from > MAX_PERIOD_MS {
        return Err(ApiError::bad_request("période invalide (31 jours au plus)"));
    }
    let list = database(&state)?
        .bookmarks(q.from, q.to, q.camera.as_deref(), MAX_LIST)
        .await
        .map_err(internal)?;
    Ok(Json(
        list.into_iter()
            .filter(|b| crate::auth::sees(principal.as_ref(), &b.camera))
            .map(BookmarkDto::from)
            .collect(),
    ))
}

/// `POST /bookmarks`: marks a moment of a camera.
pub(crate) async fn add(
    State(state): State<Arc<ApiState>>,
    principal: Option<Extension<Principal>>,
    Json(input): Json<BookmarkInput>,
) -> Result<(StatusCode, Json<BookmarkDto>), ApiError> {
    if state.camera(&input.camera).is_none()
        || !crate::auth::sees(principal.as_ref(), &input.camera)
    {
        return Err(ApiError::not_found(format!(
            "aucune caméra {:?}",
            input.camera
        )));
    }
    let note = input.note.trim();
    if note.is_empty() || note.chars().count() > MAX_NOTE {
        return Err(ApiError::bad_request(
            "la note doit avoir de 1 à 200 caractères",
        ));
    }
    let now = unix_ms(std::time::SystemTime::now());
    if input.at_ms <= 0 || input.at_ms > now + 60_000 {
        return Err(ApiError::bad_request("moment invalide"));
    }
    let author = principal.as_ref().and_then(|p| p.username.clone());
    let b = database(&state)?
        .add_bookmark(&input.camera, input.at_ms, note, author.as_deref(), now)
        .await
        .map_err(internal)?;
    Ok((StatusCode::CREATED, Json(b.into())))
}

/// `DELETE /bookmarks/{id}`.
pub(crate) async fn remove(
    State(state): State<Arc<ApiState>>,
    principal: Option<Extension<Principal>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let db = database(&state)?;
    let missing = || ApiError::not_found(format!("aucun signet {id}"));
    let b = db
        .bookmark(id)
        .await
        .map_err(internal)?
        .ok_or_else(missing)?;
    if !crate::auth::sees(principal.as_ref(), &b.camera) {
        return Err(missing());
    }
    db.delete_bookmark(id).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}
