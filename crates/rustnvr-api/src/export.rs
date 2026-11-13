//! Export of a recording extract as one MP4 file, in two steps:
//! `POST /cameras/{id}/export` prepares the file and answers when it is
//! ready (or why it failed), then `GET /exports/{file}` downloads it.
//!
//! The segments covering the period are joined by FFmpeg without
//! re-encoding (the extract starts at the key frame before `from`), or
//! converted to H.264 for players without H.265. Files are prepared in a
//! directory next to the H.264 cache and removed after a while.

use crate::dto::RecordingDto;
use crate::routes::{ApiError, RangeQuery, camera_recordings};
use crate::state::ApiState;
use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use rustnvr_recorder::scan::scan;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use tokio::process::Command;
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

/// Longest extract.
const MAX_EXPORT_MS: i64 = 2 * 3600 * 1000;
/// A prepared file is removed after this delay.
const KEEP_EXPORT: Duration = Duration::from_secs(15 * 60);
/// An export taking longer is abandoned.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Separates the working prefix from the download name in a file name.
const NAME_MARK: &str = "--";

/// Body of `POST /cameras/{id}/export`.
#[derive(Deserialize)]
pub(crate) struct ExportInput {
    from_ms: i64,
    to_ms: i64,
    /// Converts the picture to H.264 (for players without H.265).
    #[serde(default)]
    h264: bool,
}

/// A prepared extract.
#[derive(Serialize)]
pub(crate) struct ExportDto {
    /// Download URL (`GET`, Range requests supported).
    url: String,
    /// Suggested file name.
    name: String,
    /// File size.
    bytes: u64,
    /// Recorded duration in the file (the gaps are not included).
    duration_ms: i64,
    /// Start of the extract: the requested time, or the key frame before it
    /// when the picture is copied (or the first recording, if later).
    start_ms: i64,
    /// The picture was converted to H.264.
    h264: bool,
}

/// One export at a time (a conversion uses every core).
static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
/// Distinguishes the files of successive exports.
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Segments to join, back-to-back: a segment overlapping the previous one
/// (the same period on another destination) is skipped.
fn chain(list: Vec<RecordingDto>) -> Vec<RecordingDto> {
    let mut out: Vec<RecordingDto> = Vec::new();
    for r in list {
        if out
            .last()
            .is_none_or(|last| r.start_ms >= last.end_ms - 500)
        {
            out.push(r);
        }
    }
    out
}

/// Milliseconds of recording in `[from, to)`: FFmpeg joins the segments
/// end to end, so the gaps between them do not count.
fn recorded_ms(segments: &[RecordingDto], from: i64, to: i64) -> i64 {
    segments
        .iter()
        .map(|r| (r.end_ms.min(to) - r.start_ms.max(from)).max(0))
        .sum()
}

/// Time (ms from the start of the segment `path`) of the last fragment
/// starting with a key frame at or before `offset_ms`; 0 if unknown.
/// Blocking.
fn key_frame_before(path: &std::path::Path, offset_ms: i64) -> i64 {
    let Ok(report) = std::fs::File::open(path).and_then(|mut f| scan(&mut f)) else {
        return 0;
    };
    let Some(timescale) = report.timescale.filter(|t| *t > 0) else {
        return 0;
    };
    let origin = report.fragments.first().map_or(0, |f| f.base_decode_time);
    report
        .fragments
        .iter()
        .filter(|f| f.starts_with_key)
        .map(|f| {
            let ticks = f.base_decode_time.saturating_sub(origin);
            i64::try_from(ticks * 1000 / u64::from(timescale)).unwrap_or(i64::MAX)
        })
        .take_while(|ms| *ms <= offset_ms)
        .last()
        .unwrap_or(0)
}

/// `'…'` quoting for an FFmpeg concat list.
fn quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// Characters kept in file names.
fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Download name in a prepared file name (`None` for any other name).
fn download_name(file: &str) -> Option<&str> {
    if file.starts_with('.') || !file.ends_with(".mp4") || !file.chars().all(plain) {
        return None;
    }
    let (_, name) = file.split_once(NAME_MARK)?;
    (!name.is_empty()).then_some(name)
}

/// The camera of an extract, from its download name
/// (`<camera>_<date-time>_<seconds>s[-h264].mp4`; the camera may contain `_`).
fn export_camera(name: &str) -> Option<&str> {
    let mut parts = name.rsplitn(3, '_');
    let (_seconds, _stamp) = (parts.next()?, parts.next()?);
    parts.next()
}

/// Removes the extracts left over (by a restart, for instance).
async fn prune(dir: &std::path::Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let old = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > KEEP_EXPORT);
        if old {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// `POST /cameras/{id}/export`: prepares the extract.
pub(crate) async fn prepare(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(input): Json<ExportInput>,
) -> Result<Json<ExportDto>, ApiError> {
    let (from, to) = (input.from_ms, input.to_ms);
    if to <= from {
        return Err(ApiError::bad_request("la fin doit être après le début"));
    }
    if to - from > MAX_EXPORT_MS {
        return Err(ApiError::bad_request("2 heures au plus par extrait"));
    }
    if !state.ffmpeg_ok() {
        return Err(ApiError::unavailable(
            "l'export nécessite FFmpeg, qui n'a pas pu être lancé (voir live.ffmpeg)",
        ));
    }
    let Json(list) = camera_recordings(
        State(state.clone()),
        Path(id.clone()),
        Query(RangeQuery { from, to }),
    )
    .await?;
    // Privacy masks are burned into the picture: the extract is converted.
    let masks = state
        .camera(&id)
        .map(|c| c.privacy_masks)
        .unwrap_or_default();
    let h264 = input.h264 || !masks.is_empty();
    let segments = chain(list);
    let none = || ApiError::not_found("aucun enregistrement sur cette période");
    let first = segments.first().ok_or_else(none)?;
    let roots: Vec<(String, PathBuf)> = state
        .destinations()
        .into_iter()
        .map(|d| (d.id, d.root))
        .collect();
    let files: Vec<PathBuf> = segments
        .iter()
        .filter_map(|r| {
            // `/api/v1/recordings/<dest>/<camera>/<day>/<file>`
            let rest = r.url.strip_prefix("/api/v1/recordings/")?;
            let (dest, path) = rest.split_once('/')?;
            Some(roots.iter().find(|(d, _)| d == dest)?.1.join(path))
        })
        .collect();
    // Where to cut in the first segment: at the requested time when the
    // picture is converted (it is decoded from the segment start), at the key
    // frame before it when copied (a copy starting elsewhere would show a
    // broken picture until the next key frame).
    let offset_ms = (from - first.start_ms).max(0);
    let cut_ms = match files.first() {
        Some(path) if !h264 => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || key_frame_before(&path, offset_ms))
                .await
                .unwrap_or(0)
        }
        _ => offset_ms,
    };
    let start = first.start_ms + cut_ms;
    let duration_ms = recorded_ms(&segments, start, to);
    if duration_ms <= 0 {
        return Err(none());
    }

    let _permit = ONE_AT_A_TIME
        .acquire()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let dir = state.export_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::internal(format!("cannot create {}: {e}", dir.display())))?;
    prune(&dir).await;
    let stamp = jiff::Timestamp::from_millisecond(start)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y%m%d-%H%M%S")
                .to_string()
        })
        .unwrap_or_else(|_| start.to_string());
    let camera: String = id
        .chars()
        .map(|c| if plain(c) && c != '.' { c } else { '_' })
        .collect();
    let seconds = (duration_ms + 500) / 1000;
    let suffix = if h264 { "-h264" } else { "" };
    let name = format!("{camera}_{stamp}_{seconds}s{suffix}.mp4");
    let work = format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let file = format!("{work}{NAME_MARK}{name}");
    let list_path = dir.join(format!("{work}.txt"));
    let out_path = dir.join(&file);
    let list_body: String = std::iter::once("ffconcat version 1.0\n".to_owned())
        .chain(files.iter().map(|f| format!("file {}\n", quote(f))))
        .collect();
    tokio::fs::write(&list_path, list_body)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let started = std::time::Instant::now();
    let seconds_text = |ms: i64| format!("{}.{:03}", ms / 1000, ms % 1000);
    // `-ss` after `-i`: the concat input is read from its start and cut
    // there (seeking in it may land after the key frame). A copy also drops
    // the non-key frames before the first key frame. The cut is 1 ms early
    // so that the key frame itself is kept.
    let settings = state.live_settings();
    let export = |hw: Option<&crate::hwaccel::HwEncoder>| {
        let mut cmd = Command::new(&settings.ffmpeg);
        if let Some(hw) = hw {
            hw.apply_env(&mut cmd);
        }
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
            .args(crate::hwaccel::input_args(hw))
            .args(["-f", "concat", "-safe", "0", "-i"])
            .arg(&list_path)
            .args(["-ss", &seconds_text((cut_ms - 1).max(0))])
            .args(["-t", &seconds_text(duration_ms + 1)])
            .args(["-map", "0:v:0", "-map", "0:a?"]);
        if h264 {
            if let Some(vf) =
                crate::hwaccel::filter_chain(hw, &rustnvr_motion::mask_filters(&masks))
            {
                cmd.args(["-vf", &vf]);
            }
            cmd.args(crate::hwaccel::encoder_args(hw, 23, "veryfast"))
                .args(["-c:a", "copy"]);
        } else {
            cmd.args(["-c", "copy"]);
        }
        cmd.args(["-movflags", "+faststart", "-f", "mp4"])
            .arg(&out_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        async move { tokio::time::timeout(EXPORT_TIMEOUT, cmd.output()).await }
    };
    // The card only matters when converting.
    let hw = settings.hw.as_ref().filter(|_| h264);
    let mut output = export(hw).await;
    if hw.is_some()
        && let Ok(Ok(o)) = &output
        && !o.status.success()
    {
        let detail = String::from_utf8_lossy(&o.stderr);
        let reason = crate::hwaccel::failure_reason(&detail).unwrap_or_default();
        state.note_hw_failure("export", &reason);
        output = export(None).await;
    }
    let _ = tokio::fs::remove_file(&list_path).await;
    let error = match output {
        Err(_) => Some(ApiError::unavailable("export trop long, abandonné")),
        Ok(Err(e)) => Some(ApiError::unavailable(format!(
            "FFmpeg n'a pas pu être lancé : {e}"
        ))),
        Ok(Ok(output)) if !output.status.success() => {
            let detail = String::from_utf8_lossy(&output.stderr);
            let last = crate::hwaccel::failure_reason(&detail).unwrap_or_default();
            tracing::warn!(camera = %id, error = %last, "export failed");
            Some(ApiError::internal(format!("export impossible : {last}")))
        }
        Ok(Ok(_)) => None,
    };
    if let Some(error) = error {
        let _ = tokio::fs::remove_file(&out_path).await;
        return Err(error);
    }
    let bytes = tokio::fs::metadata(&out_path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .len();
    tracing::info!(
        camera = %id,
        seconds,
        segments = files.len(),
        h264 = h264,
        bytes,
        ms = started.elapsed().as_millis() as u64,
        "extract exported"
    );
    // Removed later: the browser may resume the download meanwhile.
    let later = out_path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(KEEP_EXPORT).await;
        let _ = tokio::fs::remove_file(later).await;
    });
    Ok(Json(ExportDto {
        url: format!("/api/v1/exports/{file}"),
        name,
        bytes,
        duration_ms,
        start_ms: start,
        h264,
    }))
}

/// `GET /exports/{file}`: downloads a prepared extract.
pub(crate) async fn download(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Path(file): Path<String>,
    req: Request,
) -> Result<Response, ApiError> {
    let gone = || ApiError::not_found("extrait introuvable ou expiré, relancez l'export");
    let name = download_name(&file).ok_or_else(gone)?.to_owned();
    // The extract of a camera this account does not see: as if there were none.
    if crate::auth::scope(principal.as_ref()).is_some()
        && !export_camera(&name).is_some_and(|c| crate::auth::sees(principal.as_ref(), c))
    {
        return Err(gone());
    }
    let path = state.export_dir().join(&file);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(gone());
    }
    // Serving a file never fails (`Infallible`): a missing file is a 404.
    let Ok(response) = ServeFile::new(&path).oneshot(req).await;
    let mut response = response.into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(start: i64, end: i64, dest: &str) -> RecordingDto {
        RecordingDto {
            camera: "a".into(),
            start_ms: start,
            end_ms: end,
            duration_ms: (end - start).unsigned_abs(),
            bytes: 1,
            url: format!("/api/v1/recordings/{dest}/a/2026-11-03/{start}.mp4"),
        }
    }

    #[test]
    fn chains_segments_and_counts_recorded_time() {
        // A segment overlapping the previous one (another destination) is
        // skipped; a later one after a gap is kept.
        let list = vec![
            rec(0, 60_000, "local"),
            rec(60_000, 120_000, "local"),
            rec(60_100, 120_000, "nas"),
            rec(180_000, 240_000, "nas"),
        ];
        let kept = chain(list);
        assert_eq!(
            kept.iter().map(|r| r.start_ms).collect::<Vec<_>>(),
            [0, 60_000, 180_000]
        );
        // 30 s → 200 s: 110 s recorded (the 60 s gap does not count).
        assert_eq!(recorded_ms(&kept, 30_000, 200_000), 90_000 + 20_000);
        assert_eq!(
            quote(std::path::Path::new("/r/l'a.mp4")),
            r"'/r/l'\''a.mp4'"
        );
    }

    #[test]
    fn finds_the_key_frame_before_the_start() {
        use rustnvr_recorder::fmp4::{self, SampleMeta};
        let params = rustnvr_core::VideoParams {
            codec: rustnvr_core::VideoCodec::H264,
            rfc6381_codec: "avc1.42C01E".into(),
            width: 320,
            height: 240,
            clock_rate: 90_000,
            sample_entry: bytes::Bytes::from_static(&[0, 0, 0, 8, b'a', b'v', b'c', b'1']),
        };
        // Fragments of 2 s at 0, 2 and 4 s; the one at 2 s does not start
        // with a key frame.
        let mut file = fmp4::init_segment(&params).to_vec();
        for (seq, key) in [(1u32, true), (2, false), (3, true)] {
            let meta = [SampleMeta {
                duration: 180_000,
                size: 4,
                is_key: key,
            }];
            let start = u64::from(seq - 1) * 180_000;
            file.extend_from_slice(&fmp4::fragment_header(seq, start, &meta));
            file.extend_from_slice(&[0, 0, 0, 0]);
        }
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("s.mp4");
        std::fs::write(&path, &file).expect("write");
        assert_eq!(key_frame_before(&path, 1_500), 0);
        assert_eq!(key_frame_before(&path, 3_000), 0);
        assert_eq!(key_frame_before(&path, 4_000), 4_000);
        assert_eq!(key_frame_before(&path, 9_000), 4_000);
        let missing = std::path::Path::new("/nonexistent");
        assert_eq!(key_frame_before(missing, 9_000), 0);
    }

    #[test]
    fn finds_the_camera_of_an_extract() {
        assert_eq!(
            export_camera("jardin_20261103-101500_60s.mp4"),
            Some("jardin")
        );
        assert_eq!(
            export_camera("porte_d-entree_20261103-101500_5s-h264.mp4"),
            Some("porte_d-entree")
        );
        assert_eq!(export_camera("sans-date.mp4"), None);
    }

    #[test]
    fn accepts_only_prepared_file_names() {
        assert_eq!(
            download_name("12-3--jardin_20261103-101500_60s.mp4"),
            Some("jardin_20261103-101500_60s.mp4")
        );
        for bad in [
            "../x--a.mp4",
            ".x--a.mp4",
            "12-3--a.txt",
            "12-3.mp4",
            "12-3--",
            "a/b--c.mp4",
            "a--b c.mp4",
        ] {
            assert_eq!(download_name(bad), None, "{bad}");
        }
    }
}
