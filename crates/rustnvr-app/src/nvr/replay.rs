//! Kept pictures (`ai.keep_pictures`) for the API, and the replay of the
//! object recognition on them (the files are in [`crate::captures`]).

use super::Shared;
use super::detection::{at_line, camera_settings, place};
use crate::captures::{self, Capture};
use rustnvr_api::{CaptureDay, CapturePicture, ControlError, ReplayPicture, ReplayResult};
use rustnvr_core::{CameraId, DetectedObject, MotionSettings};
use std::path::PathBuf;
use tracing::info;

fn root(shared: &Shared) -> Result<PathBuf, ControlError> {
    shared
        .captures_root
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|(root, _)| root.clone())
        .ok_or_else(|| {
            ControlError::Invalid(
                "stockage pas encore vérifié : réessayez dans quelques secondes".to_owned(),
            )
        })
}

fn io_error(e: std::io::Error) -> ControlError {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            ControlError::NotFound("aucune image gardée ici".to_owned())
        }
        std::io::ErrorKind::InvalidInput => ControlError::Invalid(e.to_string()),
        _ => ControlError::Internal(e.to_string()),
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, ControlError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ControlError::Internal("lecture interrompue".to_owned()))?
        .map_err(io_error)
}

fn picture_dto(file: String, c: Capture) -> CapturePicture {
    CapturePicture {
        file,
        at_ms: c.at_ms,
        event: c.event,
        picture: c.picture,
        line: c.line,
        provider: c.provider,
        ms: c.ms,
        found: c.found,
        kept: c.kept,
    }
}

/// Days with kept pictures, the most recent first.
pub(super) async fn days(shared: &Shared) -> Result<Vec<CaptureDay>, ControlError> {
    let root = root(shared)?;
    let days = blocking(move || Ok(captures::days(&root))).await?;
    Ok(days
        .into_iter()
        .map(|d| CaptureDay {
            camera: d.camera,
            day: d.day,
            count: d.count,
            bytes: d.bytes,
        })
        .collect())
}

/// Kept pictures of `camera` on `day`.
pub(super) async fn pictures(
    shared: &Shared,
    camera: String,
    day: String,
) -> Result<Vec<CapturePicture>, ControlError> {
    let root = root(shared)?;
    let list = blocking(move || captures::list(&root, &camera, &day)).await?;
    Ok(list.into_iter().map(|(f, c)| picture_dto(f, c)).collect())
}

/// The JPEG of a kept picture.
pub(super) async fn picture(
    shared: &Shared,
    camera: String,
    day: String,
    file: String,
) -> Result<Vec<u8>, ControlError> {
    let root = root(shared)?;
    blocking(move || captures::read(&root, &camera, &day, &file)).await
}

/// Deletes the kept pictures of `camera` on `day`.
pub(super) async fn delete(
    shared: &Shared,
    camera: String,
    day: String,
) -> Result<usize, ControlError> {
    let root = root(shared)?;
    let (c, d) = (camera.clone(), day.clone());
    let removed = blocking(move || captures::delete_day(&root, &c, &d)).await?;
    info!(%camera, day, pictures = removed, "kept pictures deleted");
    Ok(removed)
}

/// Labels of `found` that count for the camera now: for a crossing picture,
/// the objects of its line seen at the line; else those the camera keeps
/// (its objects, its zones).
fn counted(found: &[DetectedObject], capture: &Capture, settings: &MotionSettings) -> Vec<String> {
    match &capture.line {
        Some(line) => settings
            .lines
            .iter()
            .find(|l| l.name == *line)
            .map(|l| {
                found
                    .iter()
                    .filter(|o| l.objects.contains(&o.label) && at_line(o, l))
                    .map(|o| o.label.clone())
                    .collect()
            })
            .unwrap_or_default(),
        None => place(found.to_vec(), settings)
            .into_iter()
            .map(|o| o.label)
            .collect(),
    }
}

/// Labels once each, sorted.
fn label_set(labels: &[String]) -> Vec<&str> {
    let mut v: Vec<&str> = labels.iter().map(String::as_str).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Analyses again the kept pictures of `camera` on `day` (only `file` when
/// given; at most
/// [`captures::MAX_REPLAY`]) with the recognition in use now.
pub(super) async fn replay(
    shared: &Shared,
    camera: String,
    day: String,
    file: Option<String>,
) -> Result<ReplayResult, ControlError> {
    let ai = shared.ai().ok_or_else(|| {
        ControlError::Invalid("reconnaissance d'objets arrêtée : rien pour rejouer".to_owned())
    })?;
    let id = CameraId::new(&camera)
        .map_err(|_| ControlError::NotFound(format!("caméra inconnue : {camera}")))?;
    let root = root(shared)?;
    let (r, c, d) = (root.clone(), camera.clone(), day.clone());
    let mut list = blocking(move || captures::list(&r, &c, &d)).await?;
    if let Some(file) = &file {
        list.retain(|(f, _)| f == file);
        if list.is_empty() {
            return Err(ControlError::NotFound(format!("aucune image {file}")));
        }
    }
    let skipped = list.len().saturating_sub(captures::MAX_REPLAY);
    let settings = camera_settings(shared, &id).await;
    let mut pictures = Vec::new();
    for (file, capture) in list.into_iter().take(captures::MAX_REPLAY) {
        let (r, c, d, f) = (root.clone(), camera.clone(), day.clone(), file.clone());
        let jpeg = blocking(move || captures::read(&r, &c, &d, &f)).await?;
        let started = std::time::Instant::now();
        let found = ai
            .analyze(&jpeg)
            .await
            .map_err(|e| ControlError::Internal(format!("reconnaissance : {e}")))?;
        let ms = started.elapsed().as_millis() as u64;
        let kept = counted(&found, &capture, &settings);
        let changed = label_set(&kept) != label_set(&capture.kept);
        pictures.push(ReplayPicture {
            before: picture_dto(file, capture),
            found,
            kept,
            ms,
            changed,
        });
    }
    let changed = pictures.iter().filter(|p| p.changed).count();
    info!(%camera, day, pictures = pictures.len(), changed, provider = ai.provider(), "recognition replayed");
    Ok(ReplayResult {
        provider: ai.provider().to_owned(),
        pictures,
        changed,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::{CrossingDirection, CrossingLine, MotionZone};

    fn object(label: &str, bbox: [f32; 4]) -> DetectedObject {
        DetectedObject {
            label: label.into(),
            score: 0.8,
            bbox: Some(bbox),
            zone: None,
        }
    }

    #[test]
    fn counts_with_the_current_settings() {
        let capture = |line: Option<&str>| Capture {
            at_ms: 0,
            event: line.is_none().then_some(1),
            picture: None,
            line: line.map(str::to_owned),
            provider: "onnx".into(),
            ms: 1,
            found: Vec::new(),
            kept: Vec::new(),
        };
        let settings = MotionSettings {
            zones: vec![MotionZone {
                name: "Allée".into(),
                points: vec![[0.0, 0.5], [0.5, 0.5], [0.5, 1.0], [0.0, 1.0]],
                sensitivity: 50,
                objects: Vec::new(),
            }],
            lines: vec![CrossingLine {
                name: "Porte".into(),
                a: [0.0, 0.7],
                b: [1.0, 0.7],
                direction: CrossingDirection::Both,
                objects: vec!["person".into()],
                presence: false,
            }],
            ..MotionSettings::default()
        };
        let in_zone = object("person", [0.1, 0.3, 0.3, 0.8]);
        let outside = object("car", [0.6, 0.1, 0.9, 0.3]);
        let found = vec![in_zone, outside];
        // An event picture: only what is in a zone counts.
        assert_eq!(counted(&found, &capture(None), &settings), ["person"]);
        // A crossing picture: the line's objects at the line.
        assert_eq!(
            counted(&found, &capture(Some("Porte")), &settings),
            ["person"]
        );
        // A line removed since: nothing counts.
        assert!(counted(&found, &capture(Some("Autre")), &settings).is_empty());
        let a = vec!["person".to_owned(), "person".to_owned()];
        assert_eq!(label_set(&a), label_set(&["person".to_owned()]));
    }
}
