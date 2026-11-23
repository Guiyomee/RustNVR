//! Daily time-lapses of the cameras: `<root>/.timelapse/<camera>/<day>.mp4`
//! on a storage destination (`<day>` is `YYYY-MM-DD`, the local day). The
//! application makes them every night (`timelapse` of the camera's
//! detection settings); here they are listed and served.
//!
//! The folder starts with a dot, and the names are not those of segments:
//! the scans of the recordings (retention, index) leave them alone.

use crate::routes::{ApiError, control};
use crate::state::ApiState;
use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

/// Folder of the time-lapses, at the root of a destination.
pub const DIR: &str = ".timelapse";

/// Whether `day` is a `YYYY-MM-DD` date (the only names joined to a path).
pub fn valid_day(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Where a time-lapse being made is (kept in memory by the API state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Waiting for another one to be made first (one at a time).
    Waiting,
    /// Being made: how far, 0 to 100.
    Making(u8),
    /// It could not be made: why (French).
    Failed(String),
}

/// A time-lapse of a camera: ready, or on its way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimelapseDto {
    /// Local day, `YYYY-MM-DD`.
    pub day: String,
    /// `ready`, `waiting`, `making` or `failed`.
    pub state: &'static str,
    /// Size of the video (0 while it is not ready).
    pub bytes: u64,
    /// Destination holding it (empty while it is not ready).
    pub destination: String,
    /// How far it is made, 0 to 100 (`making` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// Why it could not be made (`failed` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The videos with the time-lapses on their way: one of a day being made
/// again takes the place of its video. Newest first.
fn with_jobs(mut ready: Vec<TimelapseDto>, jobs: Vec<(String, Job)>) -> Vec<TimelapseDto> {
    for (day, job) in jobs {
        ready.retain(|t| t.day != day);
        let (state, percent, error) = match job {
            Job::Waiting => ("waiting", None, None),
            Job::Making(p) => ("making", Some(p.min(100)), None),
            Job::Failed(e) => ("failed", None, Some(e)),
        };
        ready.push(TimelapseDto {
            day,
            state,
            bytes: 0,
            destination: String::new(),
            percent,
            error,
        });
    }
    ready.sort_by(|a, b| b.day.cmp(&a.day));
    ready
}

/// The time-lapses of `camera` on every destination, newest first. Blocking.
fn list(roots: &[(String, PathBuf)], camera: &str) -> Vec<TimelapseDto> {
    let mut out = Vec::new();
    for (destination, root) in roots {
        let Ok(entries) = std::fs::read_dir(root.join(DIR).join(camera)) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(day) = name.to_str().and_then(|n| n.strip_suffix(".mp4")) else {
                continue;
            };
            let Ok(meta) = e.metadata() else { continue };
            if valid_day(day) && meta.is_file() && meta.len() > 0 {
                out.push(TimelapseDto {
                    day: day.to_owned(),
                    state: "ready",
                    bytes: meta.len(),
                    destination: destination.clone(),
                    percent: None,
                    error: None,
                });
            }
        }
    }
    out.sort_by(|a, b| b.day.cmp(&a.day));
    out.dedup_by(|a, b| a.day == b.day);
    out
}

fn roots(state: &ApiState) -> Vec<(String, PathBuf)> {
    state
        .destinations()
        .into_iter()
        .filter(|d| d.remote.is_none())
        .map(|d| (d.id, d.root))
        .collect()
}

fn known_camera(state: &ApiState, camera: &str) -> Result<(), ApiError> {
    if state.cameras().iter().any(|c| c.id.as_str() == camera) {
        Ok(())
    } else {
        Err(ApiError::not_found(format!("no camera {camera:?}")))
    }
}

/// `GET /cameras/{id}/timelapses`: the videos, and those on their way.
pub(crate) async fn timelapses(
    State(state): State<Arc<ApiState>>,
    Path(camera): Path<String>,
) -> Result<Json<Vec<TimelapseDto>>, ApiError> {
    known_camera(&state, &camera)?;
    let roots = roots(&state);
    let jobs = state.timelapse_jobs(&camera);
    let found = tokio::task::spawn_blocking(move || list(&roots, &camera))
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    Ok(Json(with_jobs(found, jobs)))
}

/// `GET /cameras/{id}/timelapses/{day}`: the video (Range requests served).
pub(crate) async fn timelapse(
    State(state): State<Arc<ApiState>>,
    Path((camera, day)): Path<(String, String)>,
    req: Request,
) -> Result<Response, ApiError> {
    known_camera(&state, &camera)?;
    let day = day.strip_suffix(".mp4").unwrap_or(&day).to_owned();
    if !valid_day(&day) {
        return Err(ApiError::bad_request(
            "jour attendu sous la forme AAAA-MM-JJ",
        ));
    }
    let path = roots(&state)
        .into_iter()
        .map(|(_, root)| root.join(DIR).join(&camera).join(format!("{day}.mp4")))
        .find(|p| p.is_file())
        .ok_or_else(|| ApiError::not_found(format!("pas de time-lapse le {day}")))?;
    // Serving a file never fails (`Infallible`): a missing file is a 404.
    let Ok(response) = ServeFile::new(&path).oneshot(req).await;
    Ok(response.into_response())
}

/// `POST /cameras/{id}/timelapses/{day}` (administrators): makes the
/// time-lapse of that day now, in the background.
pub(crate) async fn make_timelapse(
    State(state): State<Arc<ApiState>>,
    Path((camera, day)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    known_camera(&state, &camera)?;
    if !valid_day(&day) {
        return Err(ApiError::bad_request(
            "jour attendu sous la forme AAAA-MM-JJ",
        ));
    }
    control(&state)?.make_timelapse(camera, day).await?;
    Ok(StatusCode::ACCEPTED)
}

/// `DELETE /cameras/{id}/timelapses/{day}` (administrators): stops the
/// time-lapse of that day if it is on its way, else removes its video (on
/// every destination) and forgets a failure.
pub(crate) async fn delete_timelapse(
    State(state): State<Arc<ApiState>>,
    Path((camera, day)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    known_camera(&state, &camera)?;
    if !valid_day(&day) {
        return Err(ApiError::bad_request(
            "jour attendu sous la forme AAAA-MM-JJ",
        ));
    }
    if control(&state)?
        .stop_timelapse(camera.clone(), day.clone())
        .await
    {
        tracing::info!(camera = %camera, day = %day, "time-lapse stopped");
        return Ok(StatusCode::NO_CONTENT);
    }
    let failed = state
        .timelapse_jobs(&camera)
        .iter()
        .any(|(d, j)| *d == day && matches!(j, Job::Failed(_)));
    state.set_timelapse_job(&camera, &day, None);
    let paths: Vec<PathBuf> = roots(&state)
        .into_iter()
        .map(|(_, root)| root.join(DIR).join(&camera).join(format!("{day}.mp4")))
        .collect();
    let removed = tokio::task::spawn_blocking(move || {
        paths
            .iter()
            .filter(|p| std::fs::remove_file(p).is_ok())
            .count()
    })
    .await
    .map_err(|e| ApiError::unavailable(e.to_string()))?;
    if removed == 0 && !failed {
        return Err(ApiError::not_found(format!("pas de time-lapse le {day}")));
    }
    tracing::info!(camera = %camera, day = %day, files = removed, "time-lapse removed");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_days_and_lists_videos() {
        assert!(valid_day("2026-11-18"));
        for bad in [
            "2026-11-1",
            "2026/11/18",
            "../../etc/x",
            "2026-11-18.mp4",
            "abcd-ef-gh",
        ] {
            assert!(!valid_day(bad), "{bad}");
        }
        let nas = tempfile::tempdir().expect("tmp");
        let local = tempfile::tempdir().expect("tmp");
        let put = |root: &std::path::Path, name: &str, bytes: &[u8]| {
            let dir = root.join(DIR).join("garage");
            std::fs::create_dir_all(&dir).expect("dir");
            std::fs::write(dir.join(name), bytes).expect("write");
        };
        put(nas.path(), "2026-11-17.mp4", b"abc");
        put(nas.path(), "2026-11-18.mp4", b"abcd");
        put(local.path(), "2026-11-18.mp4", b"x");
        put(nas.path(), "2026-11-19.mp4.part", b"partial");
        put(nas.path(), "notes.txt", b"other");
        put(nas.path(), "2026-11-16.mp4", b"");
        let roots = vec![
            ("nas".to_owned(), nas.path().to_owned()),
            ("local".to_owned(), local.path().to_owned()),
        ];
        let found = list(&roots, "garage");
        let days: Vec<(&str, u64, &str)> = found
            .iter()
            .map(|t| (t.day.as_str(), t.bytes, t.destination.as_str()))
            .collect();
        // Newest first, once per day, unfinished and empty files left out.
        assert_eq!(days, [("2026-11-18", 4, "nas"), ("2026-11-17", 3, "nas")]);
        assert!(list(&roots, "jardin").is_empty());

        // With those on their way: a day made again takes the place of its
        // video; a failure says why.
        let all = with_jobs(
            found,
            vec![
                ("2026-11-18".to_owned(), Job::Making(42)),
                ("2026-11-19".to_owned(), Job::Waiting),
                (
                    "2026-11-10".to_owned(),
                    Job::Failed("aucun enregistrement".into()),
                ),
            ],
        );
        let states: Vec<(&str, &str, Option<u8>)> = all
            .iter()
            .map(|t| (t.day.as_str(), t.state, t.percent))
            .collect();
        assert_eq!(
            states,
            [
                ("2026-11-19", "waiting", None),
                ("2026-11-18", "making", Some(42)),
                ("2026-11-17", "ready", None),
                ("2026-11-10", "failed", None),
            ]
        );
        assert_eq!(all[3].error.as_deref(), Some("aucun enregistrement"));
    }
}
