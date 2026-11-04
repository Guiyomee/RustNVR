//! `/api/v1/cameras/{id}/live`: live video as fragmented MP4 over WebSocket,
//! for Media Source Extensions in the browser. No decoding, no re-encoding:
//! each received frame becomes one `moof` + `mdat` fragment.
//!
//! Protocol (server → client):
//! 1. text `{"type":"init","codec":"avc1.42C01F","width":1920,"height":1080}`;
//! 2. binary initialization segment (`ftyp` + `moov`);
//! 3. one binary fragment per frame, starting at a key frame.
//!
//! Query parameters:
//! * `stream=main|sub` — which camera stream (default: `sub` when configured,
//!   lighter to decode, otherwise `main`);
//! * `transcode=h264` — receive an H.264 conversion made by FFmpeg (see
//!   [`crate::transcode`]), for browsers that cannot decode the camera codec.
//!
//! The server closes the socket when the stream changes (reconnection,
//! new parameters) or when the client cannot keep up; the client reconnects.
//! A close reason starting with `transcode:` reports a conversion failure.

use crate::routes::ApiError;
use crate::state::ApiState;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse as _, Response};
use bytes::Bytes;
use rustnvr_core::VideoFrame;
use rustnvr_recorder::live_mux::{LiveMuxer, Push};
use rustnvr_stream::LiveTap;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

/// Viewers per camera; each costs one copy of the stream on the network.
pub(crate) const MAX_VIEWERS_PER_CAMERA: usize = 8;

#[derive(Deserialize)]
pub(crate) struct LiveQuery {
    stream: Option<String>,
    transcode: Option<String>,
    /// `1`: include the sound of the camera, when it is recorded.
    audio: Option<String>,
}

pub(crate) async fn handler(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Query(q): Query<LiveQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let camera = state
        .camera(&id)
        .ok_or_else(|| ApiError::not_found(format!("no camera {id:?}")))?;
    let (source, stream) = match q.stream.as_deref() {
        None | Some("sub") if camera.live_sub.is_some() => (camera.live_sub.clone(), "sub"),
        // Without a sub stream, `sub` falls back to the main stream.
        None | Some("sub") | Some("main") => (camera.live.clone(), "main"),
        Some(other) => return Err(ApiError::bad_request(format!("unknown stream {other:?}"))),
    };
    let source =
        source.ok_or_else(|| ApiError::not_found(format!("camera {id:?} is not streaming")))?;
    let (tap, failure) = match q.transcode.as_deref() {
        None => (source, None),
        Some("h264") => {
            if !state.live_settings().transcode {
                return Err(ApiError::unavailable(
                    "live H.264 conversion is disabled (live.transcode = false)",
                ));
            }
            let handle = state.transcoders.get_or_start(
                format!("{id}/{stream}"),
                source,
                state.live_settings(),
                state.shutdown.clone(),
            );
            (handle.tap, Some(handle.failure))
        }
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "unsupported transcode target {other:?}"
            )));
        }
    };
    if tap.viewers() >= MAX_VIEWERS_PER_CAMERA {
        return Err(ApiError::unavailable(format!(
            "at most {MAX_VIEWERS_PER_CAMERA} live viewers per camera"
        )));
    }
    // The converted stream keeps the sound (passed around FFmpeg); a camera
    // whose sound is not recorded is not heard live either.
    let audio = q.audio.as_deref() == Some("1") && camera.record_audio;
    Ok(ws.on_upgrade(move |socket| client(state, tap, failure, socket, audio)))
}

async fn close(mut socket: WebSocket, reason: &str) {
    // WebSocket close reasons are limited to 123 bytes.
    let mut end = reason.len().min(120);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let frame = CloseFrame {
        code: 1000,
        reason: reason[..end].to_owned().into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

/// Resolves with the error message once the conversion feeding this viewer failed.
async fn conversion_failure(failure: &mut Option<watch::Receiver<Option<String>>>) -> String {
    let Some(rx) = failure else {
        return std::future::pending().await;
    };
    loop {
        if let Some(reason) = rx.borrow_and_update().clone() {
            return reason;
        }
        if rx.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

/// Sends `frame`, creating the muxer (and sending the init messages) at the first key frame.
/// Returns `Err(reason)` when the connection must end.
async fn forward(
    socket: &mut WebSocket,
    muxer: &mut Option<LiveMuxer>,
    frame: VideoFrame,
    audio: bool,
) -> Result<(), String> {
    let m = match muxer {
        Some(m) => m,
        None => {
            if !frame.is_key {
                return Ok(()); // wait for a decodable starting point
            }
            let m = if audio {
                LiveMuxer::with_audio(&frame)
            } else {
                LiveMuxer::new(&frame)
            };
            let p = &frame.params;
            // `codec` is the video codec (labels, conversion decision);
            // `codecs` is what the media source must accept.
            let init = serde_json::json!({
                "type": "init",
                "codec": p.rfc6381_codec,
                "codecs": m.codecs(),
                "audio": m.audio_codec(),
                "width": p.width,
                "height": p.height,
            });
            socket
                .send(Message::Text(init.to_string().into()))
                .await
                .map_err(|_| "send failed".to_owned())?;
            socket
                .send(Message::Binary(Bytes::from(m.init_segment())))
                .await
                .map_err(|_| "send failed".to_owned())?;
            muxer.insert(m)
        }
    };
    match m.push(frame) {
        Push::Nothing => Ok(()),
        Push::Fragment(bytes) => socket
            .send(Message::Binary(bytes))
            .await
            .map_err(|_| "send failed".to_owned()),
        Push::Reset(reason) => Err(reason.to_owned()),
    }
}

async fn client(
    state: Arc<ApiState>,
    tap: LiveTap,
    mut failure: Option<watch::Receiver<Option<String>>>,
    mut socket: WebSocket,
    audio: bool,
) {
    let (backlog, mut rx) = tap.subscribe();
    let mut muxer = None;
    for frame in backlog {
        if let Err(reason) = forward(&mut socket, &mut muxer, frame, audio).await {
            return close(socket, &reason).await;
        }
    }
    loop {
        let outcome = tokio::select! {
            _ = state.shutdown.cancelled() => Err("server shutting down".to_owned()),
            reason = conversion_failure(&mut failure) => Err(reason),
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => Ok(()),
            },
            frame = rx.recv() => match frame {
                Ok(frame) => forward(&mut socket, &mut muxer, frame, audio).await,
                // The client fell behind: restart it from a key frame.
                Err(RecvError::Lagged(_)) => Err("viewer too slow".to_owned()),
                Err(RecvError::Closed) => Err("stream stopped".to_owned()),
            },
        };
        if let Err(reason) = outcome {
            tracing::debug!(reason, "closing live viewer");
            return close(socket, &reason).await;
        }
    }
}

/// Addresses of this machine a viewer at `client` can reach (ICE host
/// candidates): the interface routing to the client, and the one routing to
/// the outside (local network address), without sending anything.
fn host_ips(client: std::net::IpAddr) -> Vec<std::net::IpAddr> {
    let route = |to: std::net::IpAddr| -> Option<std::net::IpAddr> {
        let bind: std::net::IpAddr = if to.is_ipv6() {
            std::net::Ipv6Addr::UNSPECIFIED.into()
        } else {
            std::net::Ipv4Addr::UNSPECIFIED.into()
        };
        let s = std::net::UdpSocket::bind((bind, 0)).ok()?;
        s.connect((to, 9)).ok()?;
        Some(s.local_addr().ok()?.ip())
    };
    let mut ips = Vec::new();
    for target in [client, std::net::IpAddr::from([192, 0, 2, 1])] {
        if let Some(ip) = route(target)
            && !ip.is_unspecified()
            && !ips.contains(&ip)
        {
            ips.push(ip);
        }
    }
    ips
}

/// `POST /cameras/{id}/live/webrtc?stream=main|sub` (WHEP): SDP offer in the
/// body, SDP answer in the response. H.264 cameras are sent as is; H.265
/// cameras through the live H.264 conversion.
pub(crate) async fn webrtc(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Query(q): Query<LiveQuery>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    offer: String,
) -> Result<Response, ApiError> {
    let camera = state
        .camera(&id)
        .ok_or_else(|| ApiError::not_found(format!("no camera {id:?}")))?;
    let (source, stream) = match q.stream.as_deref() {
        None | Some("sub") if camera.live_sub.is_some() => (camera.live_sub.clone(), "sub"),
        None | Some("sub") | Some("main") => (camera.live.clone(), "main"),
        Some(other) => return Err(ApiError::bad_request(format!("unknown stream {other:?}"))),
    };
    let source =
        source.ok_or_else(|| ApiError::not_found(format!("camera {id:?} is not streaming")))?;
    let codec = source.subscribe().0.first().map(|f| f.params.codec);
    let tap = match codec {
        Some(rustnvr_core::VideoCodec::H264) => source,
        Some(_) => {
            if !state.live_settings().transcode {
                return Err(ApiError::unavailable(
                    "WebRTC needs H.264: enable live.transcode for this H.265 camera",
                ));
            }
            state
                .transcoders
                .get_or_start(
                    format!("{id}/{stream}"),
                    source,
                    state.live_settings(),
                    state.shutdown.clone(),
                )
                .tap
        }
        None => return Err(ApiError::unavailable("no recent picture from the camera")),
    };
    if tap.viewers() >= MAX_VIEWERS_PER_CAMERA {
        return Err(ApiError::unavailable(format!(
            "at most {MAX_VIEWERS_PER_CAMERA} live viewers per camera"
        )));
    }
    let ips = host_ips(peer.ip());
    let answer = rustnvr_webrtc::start_session(&offer, tap, &ips, state.shutdown.child_token())
        .await
        .map_err(ApiError::bad_request)?;
    Ok((
        axum::http::StatusCode::CREATED,
        [(axum::http::header::CONTENT_TYPE, "application/sdp")],
        answer,
    )
        .into_response())
}
