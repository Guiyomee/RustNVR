//! License plate reading (`ai.plates`): once the recognition finds
//! vehicles in an event picture, a picture of the main stream is taken
//! (the sub stream's, 640 pixels wide, makes plates of 30 pixels) and the
//! plate of each vehicle is read by the two built-in plate models
//! (`rustnvr_ai::plates`). Without the `onnx` feature, nothing is read.

use super::Shared;
use bytes::Bytes;
use rustnvr_core::{CameraId, DetectedObject};

/// Labels whose plate is read.
const VEHICLES: [&str; 4] = ["car", "truck", "bus", "motorcycle"];

/// The plate models, loaded on first use.
#[derive(Default)]
pub(super) struct Plates {
    /// The models, or why they cannot be used; `None` until first use
    /// (and after a plate model is downloaded).
    #[cfg(feature = "onnx")]
    reader:
        tokio::sync::Mutex<Option<Result<std::sync::Arc<rustnvr_ai::plates::PlateReader>, String>>>,
}

impl Plates {
    /// Loads the models again at the next use (a plate model was downloaded).
    pub(super) async fn forget(&self) {
        #[cfg(feature = "onnx")]
        {
            *self.reader.lock().await = None;
        }
    }
}

/// Whether `objects` has a vehicle whose plate could be read.
pub(super) fn has_vehicle(objects: &[DetectedObject]) -> bool {
    objects
        .iter()
        .any(|o| o.bbox.is_some() && VEHICLES.contains(&o.label.as_str()))
}

/// Reads the plate of the vehicles of `objects` (at most 4), found on
/// `event_jpeg`, on a picture of the main stream of `camera` (privacy
/// `masks` applied); `event_jpeg` itself when the main stream gives none.
#[cfg(feature = "onnx")]
pub(super) async fn read(
    shared: &Shared,
    camera: &CameraId,
    objects: &mut [DetectedObject],
    event_jpeg: &Bytes,
    masks: &[[f32; 4]],
) {
    use tracing::{debug, info, warn};
    /// Vehicles read in one picture.
    const MAX_VEHICLES: usize = 4;
    /// Width of the main stream picture (as the clips).
    const PICTURE_WIDTH: u32 = 1920;

    let vehicles: Vec<(usize, [f32; 4])> = objects
        .iter()
        .enumerate()
        .filter(|(_, o)| VEHICLES.contains(&o.label.as_str()))
        .filter_map(|(i, o)| o.bbox.map(|b| (i, b)))
        .take(MAX_VEHICLES)
        .collect();
    if vehicles.is_empty() {
        return;
    }
    let Some(reader) = reader(shared).await else {
        return;
    };
    let known = shared
        .ai_settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0
        .known_plates
        .clone();
    let main = {
        let state = shared.state.lock().await;
        state.running.get(camera).map(|rt| rt.live.clone())
    };
    let jpeg = match main {
        Some(tap) => {
            match rustnvr_motion::snapshot_jpeg(&shared.ffmpeg, &tap, PICTURE_WIDTH, masks).await {
                Ok(jpeg) => jpeg,
                Err(e) => {
                    debug!(%camera, error = %e, "no main stream picture for the plates");
                    event_jpeg.clone()
                }
            }
        }
        None => event_jpeg.clone(),
    };
    let boxes: Vec<[f32; 4]> = vehicles.iter().map(|(_, b)| *b).collect();
    let started = std::time::Instant::now();
    let read = tokio::task::spawn_blocking(move || reader.read_blocking(&jpeg, &boxes)).await;
    let ms = started.elapsed().as_millis() as u64;
    let plates = match read {
        Ok(Ok(plates)) => plates,
        Ok(Err(e)) => {
            warn!(%camera, error = %e, "plate reading failed");
            return;
        }
        Err(e) => {
            warn!(%camera, error = %e, "plate reading stopped");
            return;
        }
    };
    let mut texts = Vec::new();
    for ((index, _), plate) in vehicles.iter().zip(plates) {
        let Some(plate) = plate else { continue };
        // Two boxes on one vehicle (a car also seen as a truck): one plate.
        if objects
            .iter()
            .any(|o| o.plate.as_deref() == Some(&plate.text))
        {
            continue;
        }
        let owner = known
            .iter()
            .find(|k| rustnvr_core::config::plate_key(&k.plate) == plate.text)
            .map(|k| k.name.trim().to_owned());
        texts.push(match &owner {
            Some(name) => format!("{} ({name})", plate.text),
            None => plate.text.clone(),
        });
        if let Some(o) = objects.get_mut(*index) {
            o.plate = Some(plate.text);
            o.plate_owner = owner;
        }
    }
    if texts.is_empty() {
        debug!(%camera, vehicles = vehicles.len(), ms, "no plate read");
    } else {
        info!(%camera, plates = ?texts, ms, "plates read");
    }
}

/// Without the built-in recognition: nothing is read.
#[cfg(not(feature = "onnx"))]
pub(super) async fn read(
    _shared: &Shared,
    _camera: &CameraId,
    _objects: &mut [DetectedObject],
    _event_jpeg: &Bytes,
    _masks: &[[f32; 4]],
) {
}

/// The plate models, loaded once (a failure is reported once, then kept
/// until a plate model is downloaded again).
#[cfg(feature = "onnx")]
async fn reader(shared: &Shared) -> Option<std::sync::Arc<rustnvr_ai::plates::PlateReader>> {
    use tracing::{info, warn};
    let mut slot = shared.plates.reader.lock().await;
    if slot.is_none() {
        let dir = shared.data_dir.join("models");
        let [detector, reader] = rustnvr_ai::models::PLATE_MODELS else {
            return None;
        };
        let (detector_path, reader_path) = (dir.join(detector.file), dir.join(reader.file));
        let missing = [(detector, &detector_path), (reader, &reader_path)]
            .into_iter()
            .find(|(m, p)| !std::fs::metadata(p).is_ok_and(|f| f.len() == m.bytes))
            .map(|(m, _)| m.name);
        let loaded = match missing {
            Some(name) => Err(format!("modèle « {name} » pas téléchargé")),
            None => tokio::task::spawn_blocking(move || {
                rustnvr_ai::plates::PlateReader::load(&detector_path, &reader_path)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r)
            .map(std::sync::Arc::new),
        };
        match &loaded {
            Ok(r) => info!(engine = r.engine(), "plate models loaded"),
            Err(e) => warn!(error = %e, "plate reading unavailable"),
        }
        *slot = Some(loaded);
    }
    slot.as_ref().and_then(|r| r.as_ref().ok()).cloned()
}
