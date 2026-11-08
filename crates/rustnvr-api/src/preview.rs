//! Live preview of a camera being added: after a successful connection test
//! (or the choice of a webcam), the form shows the camera live for a minute,
//! before it is saved.
//!
//! The test leaves a single-use token (the address and credentials stay on
//! the server); `GET /cameras/preview/{token}` plays the camera with the
//! RTSP client used for recording and FFmpeg turns its pictures into a
//! motion JPEG (`multipart/x-mixed-replace`), which an `<img>` shows as is.
//! A webcam is read by FFmpeg itself (`POST /cameras/usb/preview`).

use crate::routes::ApiError;
use crate::state::ApiState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use rustnvr_core::RtspTarget;
use rustnvr_core::config::UsbConfig;
use rustnvr_recorder::fmp4;
use rustnvr_recorder::live_mux::{LiveMuxer, Push};
use rustnvr_stream::SessionOptions;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;
use tokio::time::Instant;

/// A token is used within this long, or forgotten.
const TOKEN_LIFETIME: Duration = Duration::from_secs(120);
/// A preview stops after this long (the camera may accept few connections).
const PREVIEW_LENGTH: Duration = Duration::from_secs(60);
/// At most this many tokens are kept.
const MAX_TOKENS: usize = 16;
/// Pictures per second of the preview, and their largest width.
const PREVIEW_FPS: u32 = 4;
const PREVIEW_WIDTH: u32 = 640;

/// What a preview shows.
pub(crate) enum PreviewSource {
    /// An RTSP camera, played as for the recording.
    Rtsp(RtspTarget, SessionOptions),
    /// A webcam, read by FFmpeg.
    Usb(UsbConfig),
}

/// Cameras tested a moment ago, by token.
#[derive(Default)]
pub(crate) struct Previews(Mutex<HashMap<String, (PreviewSource, Instant)>>);

impl Previews {
    /// A new single-use token for `source`.
    pub(crate) fn register(&self, source: PreviewSource) -> Option<String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).ok()?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|_, (_, at)| at.elapsed() < TOKEN_LIFETIME);
        if all.len() >= MAX_TOKENS
            && let Some(oldest) = all
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
        {
            all.remove(&oldest);
        }
        all.insert(token.clone(), (source, Instant::now()));
        Some(token)
    }

    /// The camera of `token`, which can no longer be used.
    fn take(&self, token: &str) -> Option<PreviewSource> {
        let mut all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (source, at) = all.remove(token)?;
        (at.elapsed() < TOKEN_LIFETIME).then_some(source)
    }
}

/// FFmpeg arguments: fragmented MP4 on standard input, motion JPEG out.
fn ffmpeg_args() -> Vec<String> {
    [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-probesize",
        "32k",
        "-analyzeduration",
        "0",
        "-fflags",
        "nobuffer",
        "-f",
        "mp4",
        "-i",
        "pipe:0",
        "-vf",
        &format!("fps={PREVIEW_FPS},scale='min({PREVIEW_WIDTH},iw)':-2"),
        "-q:v",
        "6",
        "-f",
        "mpjpeg",
        "pipe:1",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// FFmpeg arguments: the webcam `usb` read directly, motion JPEG out, for
/// the length of a preview.
fn usb_args(usb: &UsbConfig) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-f",
        "v4l2",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    if let Some(f) = &usb.input_format {
        args.extend(["-input_format".to_owned(), f.clone()]);
    }
    if let (Some(w), Some(h)) = (usb.width, usb.height) {
        args.extend(["-video_size".to_owned(), format!("{w}x{h}")]);
    }
    if let Some(f) = usb.framerate {
        args.extend(["-framerate".to_owned(), f.to_string()]);
    }
    args.extend([
        "-i".to_owned(),
        usb.device.clone(),
        "-t".to_owned(),
        PREVIEW_LENGTH.as_secs().to_string(),
        "-an".to_owned(),
        "-vf".to_owned(),
        format!("fps={PREVIEW_FPS},scale='min({PREVIEW_WIDTH},iw)':-2"),
        "-q:v".to_owned(),
        "6".to_owned(),
        "-f".to_owned(),
        "mpjpeg".to_owned(),
        "pipe:1".to_owned(),
    ]);
    args
}

/// The preview of a webcam: FFmpeg reads it (a minute at most) and stops
/// when the viewer leaves (its output breaks).
fn usb_preview(state: &ApiState, usb: &UsbConfig) -> Result<Response, ApiError> {
    let mut child = tokio::process::Command::new(&state.live_settings().ffmpeg)
        .args(usb_args(usb))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("FFmpeg ne démarre pas : {e}")))?;
    let Some(stdout) = child.stdout.take() else {
        return Err(ApiError::internal("FFmpeg sans sortie"));
    };
    tokio::spawn(async move {
        let _ = child.wait().await;
        tracing::debug!("webcam preview ended");
    });
    mjpeg(stdout)
}

/// A motion JPEG answer from FFmpeg's output.
fn mjpeg(stdout: tokio::process::ChildStdout) -> Result<Response, ApiError> {
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "multipart/x-mixed-replace;boundary=ffmpeg",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(stdout)))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// The camera that uses the webcam `device` now, if any (a webcam cannot be
/// opened twice).
fn webcam_user(state: &ApiState, device: &str) -> Option<String> {
    let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| p.into());
    let wanted = real(device);
    state
        .cameras()
        .into_iter()
        .filter(|c| c.enabled)
        .find(|c| {
            c.endpoint
                .strip_prefix("usb:")
                .is_some_and(|d| real(d) == wanted)
        })
        .map(|c| c.name.clone().unwrap_or_else(|| c.id.to_string()))
}

/// `POST /cameras/usb/preview` answer.
#[derive(serde::Serialize)]
pub(crate) struct UsbPreviewDto {
    preview: String,
}

/// `POST /cameras/usb/preview` (the webcam settings of the form): the
/// address of its live preview.
pub(crate) async fn usb_token(
    State(state): State<Arc<ApiState>>,
    axum::Json(usb): axum::Json<UsbConfig>,
) -> Result<axum::Json<UsbPreviewDto>, ApiError> {
    if !state.ffmpeg_ok() {
        return Err(ApiError::unavailable(
            "l'aperçu a besoin de FFmpeg, qui ne démarre pas (voir live.ffmpeg)",
        ));
    }
    let problems = usb.problems();
    if !problems.is_empty() {
        return Err(ApiError::bad_request(problems.join(" ; ")));
    }
    if let Some(camera) = webcam_user(&state, &usb.device) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "busy",
            format!(
                "la webcam est déjà utilisée par la caméra « {camera} » : son direct est sur sa page"
            ),
        ));
    }
    let token = state
        .previews
        .register(PreviewSource::Usb(usb))
        .ok_or_else(|| ApiError::internal("pas de jeton"))?;
    Ok(axum::Json(UsbPreviewDto {
        preview: format!("/api/v1/cameras/preview/{token}"),
    }))
}

/// `GET /cameras/preview/{token}`: the camera live, as a motion JPEG, for a
/// minute at most.
pub(crate) async fn preview(
    State(state): State<Arc<ApiState>>,
    Path(token): Path<String>,
) -> Result<Response, ApiError> {
    if !state.ffmpeg_ok() {
        return Err(ApiError::unavailable(
            "l'aperçu a besoin de FFmpeg, qui ne démarre pas (voir live.ffmpeg)",
        ));
    }
    let source = state
        .previews
        .take(&token)
        .ok_or_else(|| ApiError::not_found("aperçu expiré : refaites le test de connexion"))?;
    let (target, opts) = match source {
        PreviewSource::Usb(usb) => return usb_preview(&state, &usb),
        PreviewSource::Rtsp(target, opts) => (target, opts),
    };
    let mut session = rustnvr_stream::probe::play(&target, &opts)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "probe_failed", e.summary()))?;
    let mut child = tokio::process::Command::new(&state.live_settings().ffmpeg)
        .args(ffmpeg_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("FFmpeg ne démarre pas : {e}")))?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(ApiError::internal("FFmpeg sans entrée ni sortie"));
    };
    // The camera's pictures go to FFmpeg until the minute is over, the
    // viewer leaves (FFmpeg stops, its input breaks) or the camera stops.
    tokio::spawn(async move {
        let _child = child;
        let deadline = Instant::now() + PREVIEW_LENGTH;
        let mut muxer: Option<LiveMuxer> = None;
        loop {
            let frame = match tokio::time::timeout_at(deadline, session.next_frame()).await {
                Ok(Ok(f)) => f,
                _ => break,
            };
            let bytes = match muxer.as_mut() {
                // From the first key frame.
                None if !frame.is_key => continue,
                None => {
                    let m = LiveMuxer::tolerant(&frame);
                    muxer = Some(m);
                    let mut init = fmp4::init_segment(&frame.params);
                    if let Some(Push::Fragment(b)) = muxer.as_mut().map(|m| m.push(frame)) {
                        init.extend_from_slice(&b);
                    }
                    init
                }
                Some(m) => match m.push(frame) {
                    Push::Fragment(b) => b.to_vec(),
                    Push::Nothing => continue,
                    Push::Reset(_) => break,
                },
            };
            if stdin.write_all(&bytes).await.is_err() {
                break;
            }
        }
        tracing::debug!("camera preview ended");
    });
    mjpeg(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtsp() -> PreviewSource {
        let target = RtspTarget::parse("rtsp://10.0.0.2/live", None, None).expect("target");
        PreviewSource::Rtsp(target, SessionOptions::default())
    }

    #[test]
    fn tokens_are_used_once() {
        let previews = Previews::default();
        let token = previews.register(rtsp()).expect("token");
        assert_eq!(token.len(), 32);
        assert!(previews.take(&token).is_some());
        assert!(previews.take(&token).is_none(), "single use");
        assert!(previews.take("unknown").is_none());
        // Never more than MAX_TOKENS kept.
        for _ in 0..MAX_TOKENS + 5 {
            previews.register(rtsp());
        }
        assert_eq!(previews.0.lock().expect("lock").len(), MAX_TOKENS);
        assert!(ffmpeg_args().join(" ").contains("-f mpjpeg pipe:1"));
    }

    #[test]
    fn a_webcam_preview_reads_the_device() {
        let usb = UsbConfig {
            device: "/dev/video0".into(),
            input_format: Some("mjpeg".into()),
            width: Some(1280),
            height: Some(720),
            framerate: Some(15),
            bitrate_kbps: 2000,
        };
        let args = usb_args(&usb).join(" ");
        assert!(
            args.starts_with("-hide_banner -nostats -loglevel error -f v4l2 -input_format mjpeg -video_size 1280x720 -framerate 15 -i /dev/video0 -t 60 -an"),
            "{args}"
        );
        assert!(args.ends_with("-f mpjpeg pipe:1"), "{args}");
    }
}
