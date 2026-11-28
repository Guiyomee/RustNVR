//! Deleting by hand a recording or an event (administrators only: a
//! viewer and a read-only token cannot send `DELETE`). Protected periods
//! are never deleted; the segment index and the events follow at once.

use crate::recordings::parse_segment_name;
use crate::routes::ApiError;
use crate::state::ApiState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use rustnvr_core::CameraId;
use rustnvr_storage::{Database, EventDeletion};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Longest a segment may last: the end of a segment the index does not know
/// yet, for the check of protected periods (counted long, to keep more).
const LONGEST_SEGMENT_MS: i64 = 3_600_000;

fn db_error(e: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %e, "database error");
    ApiError::internal(e.to_string())
}

fn conflict(message: &str) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "conflict", message)
}

/// A recording of a destination, once its path is checked.
struct Segment {
    destination: String,
    camera: String,
    file: String,
    path: PathBuf,
}

/// Whether a protected period of `camera` overlaps `[start_ms, end_ms)`.
async fn protected(
    db: Option<&Database>,
    camera: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<bool, ApiError> {
    let Some(db) = db else {
        return Ok(false);
    };
    Ok(db
        .protected_ranges()
        .await
        .map_err(db_error)?
        .iter()
        .any(|p| p.camera == camera && p.start_ms < end_ms && p.end_ms > start_ms))
}

/// Deletes the file of `s` and forgets it in the index.
async fn remove(db: Option<&Database>, s: &Segment) -> Result<(), ApiError> {
    match tokio::fs::remove_file(&s.path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(ApiError::internal(format!(
                "impossible de supprimer l'enregistrement : {e}"
            )));
        }
    }
    if let Some(db) = db {
        db.delete_segment(&s.destination, &s.camera, &s.file)
            .await
            .map_err(db_error)?;
    }
    tracing::info!(camera = %s.camera, destination = %s.destination, file = %s.file, "recording deleted by hand");
    Ok(())
}

/// Root folder of the destination `id`.
fn destination_root(state: &ApiState, id: &str) -> Option<PathBuf> {
    state
        .destinations()
        .into_iter()
        .find(|d| d.id == id)
        .map(|d| d.root)
}

/// `DELETE /recordings/{dest}/{camera}/{day}/{file}`: deletes one recording.
/// 409 when it is in a protected period.
pub(crate) async fn delete_recording(
    State(state): State<Arc<ApiState>>,
    Path((dest, camera, day, file)): Path<(String, String, String, String)>,
) -> Result<StatusCode, ApiError> {
    // Every component is validated, so the path cannot leave the recordings tree.
    let start_ms = parse_segment_name(&file);
    let (Some(start_ms), true, true, true) = (
        start_ms,
        CameraId::new(dest.as_str()).is_ok(),
        CameraId::new(camera.as_str()).is_ok(),
        crate::recordings::valid_day(&day),
    ) else {
        return Err(ApiError::bad_request("invalid recording path"));
    };
    let root = destination_root(&state, &dest)
        .ok_or_else(|| ApiError::not_found(format!("no storage destination {dest:?}")))?;
    let path = root.join(&camera).join(&day).join(&file);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(ApiError::not_found("no such recording"));
    }
    let db = state.database.get();
    let end_ms = match db {
        Some(db) => db
            .segments(&camera, start_ms, start_ms + 1)
            .await
            .map_err(db_error)?
            .into_iter()
            .find(|s| s.destination == dest && s.file == file)
            .map(|s| s.end_ms),
        None => None,
    }
    .unwrap_or(start_ms + LONGEST_SEGMENT_MS);
    if protected(db, &camera, start_ms, end_ms).await? {
        return Err(conflict(
            "enregistrement dans une période protégée : retirez d'abord la protection",
        ));
    }
    let segment = Segment {
        destination: dest,
        camera,
        file,
        path,
    };
    remove(db, &segment).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /events/{id}` query.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteEventQuery {
    /// Also delete the recordings the event is in (whole segments).
    #[serde(default)]
    video: bool,
}

/// What `DELETE /events/{id}` did.
#[derive(Serialize)]
pub(crate) struct EventDeleted {
    /// Recordings deleted with the event.
    recordings_deleted: usize,
    /// Recordings of the event kept because they are protected.
    recordings_protected: usize,
}

/// `DELETE /events/{id}?video=`: deletes a finished event with its
/// thumbnail and, with `video=true`, the recordings it is in. 409 for an
/// event still going on.
pub(crate) async fn delete_event(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Query(q): Query<DeleteEventQuery>,
) -> Result<Json<EventDeleted>, ApiError> {
    let id: i64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("invalid event identifier"))?;
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("events are not available on this server"))?;
    let event = db
        .event(id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| ApiError::not_found(format!("no event {id}")))?;
    let Some(end_ms) = event.end_ms else {
        return Err(conflict("événement en cours : attendez sa fin"));
    };
    let mut done = EventDeleted {
        recordings_deleted: 0,
        recordings_protected: 0,
    };
    if q.video {
        let camera = event.camera.as_str();
        // At least the second the event started.
        let end_ms = end_ms.max(event.start_ms + 1);
        for s in db
            .segments(camera, event.start_ms, end_ms)
            .await
            .map_err(db_error)?
        {
            if protected(Some(db), camera, s.start_ms, s.end_ms).await? {
                done.recordings_protected += 1;
                continue;
            }
            let Some(root) = destination_root(&state, &s.destination) else {
                continue;
            };
            let segment = Segment {
                path: root.join(&s.camera).join(&s.day).join(&s.file),
                destination: s.destination,
                camera: s.camera,
                file: s.file,
            };
            remove(Some(db), &segment).await?;
            done.recordings_deleted += 1;
        }
    }
    match db.delete_event(id).await.map_err(db_error)? {
        EventDeletion::Deleted => {}
        EventDeletion::Missing => return Err(ApiError::not_found(format!("no event {id}"))),
        EventDeletion::Ongoing => return Err(conflict("événement en cours : attendez sa fin")),
    }
    tracing::info!(event = id, camera = %event.camera, recordings = done.recordings_deleted, "event deleted by hand");
    Ok(Json(done))
}
