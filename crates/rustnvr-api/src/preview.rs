//! Live preview of a camera being added: after a successful connection test,
//! the form shows the camera live for a minute, before it is saved.
//!
//! The test leaves a single-use token (the address and credentials stay on
//! the server); `GET /cameras/preview/{token}` plays the camera with the
//! RTSP client used for recording and FFmpeg turns its pictures into a
//! motion JPEG (`multipart/x-mixed-replace`), which an `<img>` shows as is.

use crate::routes::ApiError;
use crate::state::ApiState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use rustnvr_core::RtspTarget;
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

/// Cameras tested a moment ago, by token.
#[derive(Default)]
pub(crate) struct Previews(Mutex<HashMap<String, (RtspTarget, SessionOptions, Instant)>>);

impl Previews {
    /// A new single-use token for `target`.
    pub(crate) fn register(&self, target: RtspTarget, opts: SessionOptions) -> Option<String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).ok()?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|_, (_, _, at)| at.elapsed() < TOKEN_LIFETIME);
        if all.len() >= MAX_TOKENS
            && let Some(oldest) = all
                .iter()
                .min_by_key(|(_, (_, _, at))| *at)
                .map(|(k, _)| k.clone())
        {
            all.remove(&oldest);
        }
        all.insert(token.clone(), (target, opts, Instant::now()));
        Some(token)
    }

    /// The camera of `token`, which can no longer be used.
    fn take(&self, token: &str) -> Option<(RtspTarget, SessionOptions)> {
        let mut all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (target, opts, at) = all.remove(token)?;
        (at.elapsed() < TOKEN_LIFETIME).then_some((target, opts))
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
    let (target, opts) = state
        .previews
        .take(&token)
        .ok_or_else(|| ApiError::not_found("aperçu expiré : refaites le test de connexion"))?;
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
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "multipart/x-mixed-replace;boundary=ffmpeg",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(stdout)))
        .map_err(|e| ApiError::internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_used_once() {
        let previews = Previews::default();
        let target = RtspTarget::parse("rtsp://10.0.0.2/live", None, None).expect("target");
        let token = previews
            .register(target, SessionOptions::default())
            .expect("token");
        assert_eq!(token.len(), 32);
        assert!(previews.take(&token).is_some());
        assert!(previews.take(&token).is_none(), "single use");
        assert!(previews.take("unknown").is_none());
        // Never more than MAX_TOKENS kept.
        for _ in 0..MAX_TOKENS + 5 {
            let t = RtspTarget::parse("rtsp://10.0.0.2/live", None, None).expect("target");
            previews.register(t, SessionOptions::default());
        }
        assert_eq!(previews.0.lock().expect("lock").len(), MAX_TOKENS);
        assert!(ffmpeg_args().join(" ").contains("-f mpjpeg pipe:1"));
    }
}
