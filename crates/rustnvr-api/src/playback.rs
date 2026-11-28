//! H.264 copies of recordings, for browsers that cannot decode H.265 (Edge
//! without the HEVC extension, Firefox…): `GET /recordings/…?transcode=h264`.
//!
//! A segment is converted once by FFmpeg (picture only; the sound is copied),
//! kept in a bounded cache and then served like any file (Range requests for
//! seeking). One conversion runs at a time to bound the CPU used.
//!
//! Also the picture of a recording (`?thumbnail=1`, page Vidéos): its first
//! picture, 320 pixels wide, made once by FFmpeg and kept a week.

use crate::routes::ApiError;
use crate::state::ApiState;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tokio::process::Command;

/// Converted copies kept, at most (oldest removed first).
const CACHE_BYTES: u64 = 2_000_000_000;
/// Copies older than this are removed.
const CACHE_AGE: Duration = Duration::from_secs(24 * 3600);
/// A conversion taking longer is abandoned.
const CONVERSION_TIMEOUT: Duration = Duration::from_secs(300);
/// Pictures of recordings kept, at most, and for how long.
const THUMBNAIL_BYTES: u64 = 300_000_000;
const THUMBNAIL_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// A picture taking longer is abandoned.
const THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(30);
/// Pictures made at the same time (a page shows many at once).
const THUMBNAIL_SLOTS: usize = 2;

/// Cache of converted recordings.
pub(crate) struct PlaybackConverter {
    /// Cache directory, set by the NVR (in its data directory, owned by the
    /// account running it); a temporary directory otherwise.
    dir: std::sync::OnceLock<PathBuf>,
    /// One conversion at a time.
    lock: tokio::sync::Mutex<()>,
    /// Pictures of recordings being made.
    thumbnails: tokio::sync::Semaphore,
}

impl Default for PlaybackConverter {
    fn default() -> Self {
        Self {
            dir: std::sync::OnceLock::new(),
            lock: tokio::sync::Mutex::new(()),
            thumbnails: tokio::sync::Semaphore::new(THUMBNAIL_SLOTS),
        }
    }
}

impl PlaybackConverter {
    /// Sets the cache directory (once, at startup).
    pub(crate) fn set_dir(&self, dir: PathBuf) {
        let _ = self.dir.set(dir);
    }

    /// Directory of the exported extracts: `exports` next to the cache.
    pub(crate) fn export_dir(&self) -> PathBuf {
        match self.dir.get() {
            Some(dir) => dir.with_file_name("exports"),
            None => std::env::temp_dir().join(format!("rustnvr-exports-{}", std::process::id())),
        }
    }

    fn dir(&self) -> PathBuf {
        self.dir
            .get()
            .cloned()
            .unwrap_or_else(|| std::env::temp_dir().join("rustnvr-h264"))
    }

    /// Path of the H.264 copy of `source` (identified by `key`), converting it
    /// first if needed.
    pub(crate) async fn get(
        &self,
        state: &ApiState,
        source: &Path,
        key: &str,
    ) -> Result<PathBuf, ApiError> {
        let settings = state.live_settings();
        if !settings.transcode {
            return Err(ApiError::unavailable(
                "H.264 conversion is disabled (live.transcode = false)",
            ));
        }
        let dir = self.dir();
        let target = dir.join(format!("{key}.mp4"));
        if fresh(&target, source) {
            return Ok(target);
        }
        let _one_at_a_time = self.lock.lock().await;
        // Converted while this request was waiting.
        if fresh(&target, source) {
            return Ok(target);
        }
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            tracing::warn!(dir = %dir.display(), error = %e, "cannot create the conversion cache");
            ApiError::internal(format!("cannot create {}: {e}", dir.display()))
        })?;
        let partial = dir.join(format!("{key}.part.mp4"));
        let started = std::time::Instant::now();
        let convert = |hw: Option<&crate::hwaccel::HwEncoder>| {
            let mut cmd = Command::new(&settings.ffmpeg);
            if let Some(hw) = hw {
                hw.apply_env(&mut cmd);
            }
            cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
                .args(crate::hwaccel::input_args(hw))
                .arg("-i")
                .arg(source)
                .args(["-map", "0:v:0", "-map", "0:a?"])
                .args(crate::hwaccel::encoder_args(hw, 26, "veryfast"));
            // At most 1280 pixels wide: faster, and enough for review.
            if let Some(vf) = crate::hwaccel::filter_chain(hw, "scale=w='min(1280,iw)':h=-2") {
                cmd.args(["-vf", &vf]);
            }
            cmd.args(["-c:a", "copy", "-movflags", "+faststart", "-f", "mp4"])
                .arg(&partial)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            async move {
                tokio::time::timeout(CONVERSION_TIMEOUT, cmd.output())
                    .await
                    .map_err(|_| ApiError::unavailable("conversion too long"))?
                    .map_err(|e| ApiError::unavailable(format!("cannot start FFmpeg: {e}")))
            }
        };
        let mut output = convert(settings.hw.as_ref()).await?;
        if !output.status.success() && settings.hw.is_some() {
            let detail = String::from_utf8_lossy(&output.stderr);
            let reason = crate::hwaccel::failure_reason(&detail).unwrap_or_default();
            state.note_hw_failure("relecture", &reason);
            output = convert(None).await?;
        }
        if !output.status.success() {
            let _ = tokio::fs::remove_file(&partial).await;
            let detail = String::from_utf8_lossy(&output.stderr);
            let last = crate::hwaccel::failure_reason(&detail).unwrap_or_default();
            tracing::warn!(recording = key, error = %last, "conversion to H.264 failed");
            return Err(ApiError::internal(format!("conversion failed: {last}")));
        }
        tokio::fs::rename(&partial, &target)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        tracing::info!(
            recording = key,
            ms = started.elapsed().as_millis() as u64,
            "recording converted to H.264 for playback"
        );
        let _ = tokio::task::spawn_blocking(move || prune(&dir, CACHE_AGE, CACHE_BYTES)).await;
        Ok(target)
    }

    /// Path of the picture of `source` (identified by `key`): its first
    /// picture as a JPEG 320 pixels wide, made first if needed.
    pub(crate) async fn thumbnail(
        &self,
        state: &ApiState,
        source: &Path,
        key: &str,
    ) -> Result<PathBuf, ApiError> {
        let dir = self.dir().with_file_name("recording-pictures");
        let target = dir.join(format!("{key}.jpg"));
        if fresh(&target, source) {
            return Ok(target);
        }
        let _slot = self
            .thumbnails
            .acquire()
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if fresh(&target, source) {
            return Ok(target);
        }
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| ApiError::internal(format!("cannot create {}: {e}", dir.display())))?;
        let partial = dir.join(format!("{key}.part.jpg"));
        let mut cmd = Command::new(&state.live_settings().ffmpeg);
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"])
            .arg(source)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=320:-2",
                "-q:v",
                "6",
                "-f",
                "image2",
            ])
            .arg(&partial)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(THUMBNAIL_TIMEOUT, cmd.output())
            .await
            .map_err(|_| ApiError::unavailable("picture too long to make"))?
            .map_err(|e| ApiError::unavailable(format!("cannot start FFmpeg: {e}")))?;
        if !output.status.success() {
            let _ = tokio::fs::remove_file(&partial).await;
            let detail = String::from_utf8_lossy(&output.stderr);
            let last = crate::hwaccel::failure_reason(&detail).unwrap_or_default();
            tracing::debug!(recording = key, error = %last, "no picture for the recording");
            return Err(ApiError::internal(format!("no picture: {last}")));
        }
        tokio::fs::rename(&partial, &target)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let _ =
            tokio::task::spawn_blocking(move || prune(&dir, THUMBNAIL_AGE, THUMBNAIL_BYTES)).await;
        Ok(target)
    }
}

/// The copy exists and is newer than its source.
fn fresh(target: &Path, source: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    matches!((modified(target), modified(source)), (Some(t), Some(s)) if t >= s)
}

/// Removes the files of `dir` older than `max_age`, then the oldest beyond
/// `max_bytes`.
fn prune(dir: &Path, max_age: Duration, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .filter(|(modified, _, path)| {
            let old = now.duration_since(*modified).unwrap_or_default() > max_age;
            if old {
                let _ = std::fs::remove_file(path);
            }
            !old
        })
        .collect();
    files.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
    let mut total = 0;
    for (_, len, path) in files {
        total += len;
        if total > max_bytes {
            let _ = std::fs::remove_file(path);
        }
    }
}
