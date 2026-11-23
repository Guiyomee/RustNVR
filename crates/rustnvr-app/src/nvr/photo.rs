//! Photos given by the cameras themselves (ONVIF `GetSnapshotUri`), for the
//! cameras whose detection settings ask for it (`camera_photo`): a JPEG in
//! full definition, fetched over HTTP, instead of a picture decoded from the
//! detection stream. The photo address is looked up once per camera (the
//! profile giving the largest photo, all of them tried) and kept while the
//! camera's ONVIF settings stay the same; any failure forgets it, and the
//! event takes the usual picture.

use super::Shared;
use bytes::Bytes;
use rustnvr_core::CameraId;
use rustnvr_discovery::onvif::Client;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// Timeout of each request to the camera.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The photo address of each camera, with the ONVIF service and user it was
/// read for.
static ADDRESSES: LazyLock<Mutex<HashMap<String, (String, String)>>> =
    LazyLock::new(Mutex::default);
/// Cameras whose last photo failed (warned once, until a photo works).
static FAILING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// The photo of `camera`, at most `max_width` wide (0: as the camera gives
/// it), `masks` blacked out.
pub(super) async fn take(
    shared: &Shared,
    camera: &CameraId,
    masks: &[[f32; 4]],
    max_width: u32,
) -> Result<Bytes, String> {
    if !shared.ffmpeg_ok {
        return Err("FFmpeg indisponible".to_owned());
    }
    let target = super::ptz::target(shared, camera.as_str())
        .await
        .map_err(|e| e.to_string())?;
    let key = format!(
        "{}@{}",
        target.credentials.as_ref().map_or("", |(u, _)| u.as_str()),
        target.device_url
    );
    let mut client = Client::new(target.credentials, TIMEOUT).map_err(|e| e.to_string())?;
    let known = ADDRESSES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(camera.as_str())
        .filter(|(k, _)| *k == key)
        .map(|(_, url)| url.clone());
    let jpeg = match known {
        Some(url) => match client.photo(&url).await {
            Ok(j) => j,
            Err(e) => {
                forget(camera);
                return Err(e.to_string());
            }
        },
        None => {
            client
                .sync_clock(&target.device_url)
                .await
                .map_err(|e| e.to_string())?;
            let media = client
                .media_url(&target.device_url)
                .await
                .map_err(|e| e.to_string())?;
            let urls = client
                .snapshot_urls(&media)
                .await
                .map_err(|e| e.to_string())?;
            // Every profile's photo once: some cameras give the small one
            // first, or the same whatever the profile.
            let (url, jpeg, (width, height)) = client
                .largest_photo(&urls)
                .await
                .map_err(|e| e.to_string())?;
            info!(%camera, width, height, profiles = urls.len(), "camera photo found");
            ADDRESSES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(camera.as_str().to_owned(), (key, url));
            jpeg
        }
    };
    // Nothing to hide and small enough: the camera's own JPEG, not encoded
    // again.
    let fits = max_width == 0
        || rustnvr_discovery::onvif::jpeg_size(&jpeg)
            .is_some_and(|(w, _)| u32::from(w) <= max_width);
    if masks.is_empty() && fits {
        return Ok(Bytes::from(jpeg));
    }
    let width = if max_width == 0 {
        u32::from(u16::MAX)
    } else {
        max_width
    };
    rustnvr_motion::photo_jpeg(&shared.ffmpeg, Bytes::from(jpeg), width, masks).await
}

/// Forgets the photo address of `camera` (looked up again next time).
pub(super) fn forget(camera: &CameraId) {
    ADDRESSES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(camera.as_str());
}

/// Notes how the photo of `camera` went: a failure is warned once (the
/// event takes the usual picture), the first success after it said so.
pub(super) fn report(camera: &CameraId, result: &Result<Bytes, String>) {
    let mut failing = FAILING.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(_) => {
            if failing.remove(camera.as_str()) {
                info!(%camera, "camera photo works again");
            }
        }
        Err(e) => {
            if failing.insert(camera.as_str().to_owned()) {
                warn!(%camera, error = %e, "no camera photo: the picture of the detection stream is used");
            }
        }
    }
}
