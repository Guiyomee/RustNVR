//! `/api/v1/ws`: pushes system status, camera states, finished segments and
//! detection events. An account limited to some cameras hears of those only;
//! its rights are read again every few seconds, the connection being open
//! for hours.

use crate::auth::Principal;
use crate::dto::{EventDto, RecordingDto, WsMessage};
use crate::state::{ApiState, unix_ms};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use rustnvr_core::{NvrEvent, SegmentInfo};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

const SYSTEM_PERIOD: Duration = Duration::from_secs(5);

pub(crate) async fn handler(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
    ws: WebSocketUpgrade,
) -> Response {
    let who = principal.map(|p| p.0);
    ws.on_upgrade(move |socket| client(state, socket, who))
}

/// Whether the account of this connection sees `camera` (no account: yes).
fn sees(who: Option<&Principal>, camera: &str) -> bool {
    who.is_none_or(|p| p.sees(camera))
}

async fn send(socket: &mut WebSocket, msg: &WsMessage) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(e) => {
            tracing::warn!(error = %e, "cannot serialize WebSocket message");
            true
        }
    }
}

async fn send_snapshot(state: &ApiState, socket: &mut WebSocket, who: Option<&Principal>) -> bool {
    if !send(
        socket,
        &WsMessage::System {
            system: Box::new(state.system_dto()),
        },
    )
    .await
    {
        return false;
    }
    for c in &state.cameras() {
        if !sees(who, c.id.as_str()) {
            continue;
        }
        if !send(
            socket,
            &WsMessage::Camera {
                camera: Box::new(state.camera_dto(c)),
            },
        )
        .await
        {
            return false;
        }
    }
    true
}

fn recording_dto(state: &ApiState, s: &SegmentInfo) -> Option<RecordingDto> {
    // The destination is the one whose root contains the file (longest match).
    let dest = state
        .destinations()
        .into_iter()
        .filter(|d| s.path.starts_with(&d.root))
        .max_by_key(|d| d.root.as_os_str().len())?;
    let day = s.path.parent()?.file_name()?.to_str()?.to_owned();
    let name = s.path.file_name()?.to_str()?.to_owned();
    let start_ms = unix_ms(s.start);
    // The recorder computed the duration from timestamps: no file access needed.
    let duration_ms = u64::try_from(s.duration.as_millis()).unwrap_or(u64::MAX);
    Some(RecordingDto {
        camera: s.camera.to_string(),
        start_ms,
        end_ms: start_ms + i64::try_from(duration_ms).unwrap_or(0),
        duration_ms,
        bytes: s.bytes,
        url: format!("/api/v1/recordings/{}/{}/{day}/{name}", dest.id, s.camera),
    })
}

async fn client(state: Arc<ApiState>, mut socket: WebSocket, mut who: Option<Principal>) {
    let mut events = state.events.subscribe();
    if !send_snapshot(&state, &mut socket, who.as_ref()).await {
        return;
    }
    let mut ticker = tokio::time::interval(SYSTEM_PERIOD);
    ticker.tick().await;
    loop {
        let ok = tokio::select! {
            _ = state.shutdown.cancelled() => {
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => true, // clients do not send commands yet
            },
            _ = ticker.tick() => {
                // The account may have been removed, or limited to other cameras.
                let before = who.clone();
                if let Some(p) = &who {
                    match crate::auth::refreshed(&state, p).await {
                        Some(now) => who = Some(now),
                        None => {
                            let _ = socket.send(Message::Close(None)).await;
                            return;
                        }
                    }
                }
                if who != before {
                    let cameras = state.cameras().iter().filter(|c| sees(who.as_ref(), c.id.as_str())).map(|c| state.camera_dto(c)).collect();
                    if !send(&mut socket, &WsMessage::Cameras { cameras }).await {
                        return;
                    }
                }
                send(&mut socket, &WsMessage::System { system: Box::new(state.system_dto()) }).await
            },
            event = events.recv() => match event {
                // Nothing about a camera this account does not see.
                Ok(NvrEvent::StreamState { camera, .. } | NvrEvent::RecorderError { camera, .. } | NvrEvent::MotionSettingsChanged { camera, .. } | NvrEvent::Crossing { camera, .. }) if !sees(who.as_ref(), camera.as_str()) => true,
                Ok(NvrEvent::SegmentFinished(info)) if !sees(who.as_ref(), info.camera.as_str()) => true,
                Ok(NvrEvent::Detection(event)) if !sees(who.as_ref(), event.camera.as_str()) => true,
                Ok(NvrEvent::StreamState { camera, .. }) => match state.camera(camera.as_str()) {
                    Some(c) => send(&mut socket, &WsMessage::Camera { camera: Box::new(state.camera_dto(&c)) }).await,
                    None => true,
                },
                Ok(NvrEvent::CamerasChanged) => {
                    let cameras = state.cameras().iter().filter(|c| sees(who.as_ref(), c.id.as_str())).map(|c| state.camera_dto(c)).collect();
                    send(&mut socket, &WsMessage::Cameras { cameras }).await
                }
                Ok(NvrEvent::SegmentFinished(info)) => match recording_dto(&state, &info) {
                    Some(recording) => send(&mut socket, &WsMessage::Segment { recording }).await,
                    None => true,
                },
                Ok(NvrEvent::RecorderError { camera, message, .. }) => {
                    send(&mut socket, &WsMessage::RecorderError { camera: camera.to_string(), message }).await
                }
                Ok(NvrEvent::Detection(event)) => {
                    // The camera's `motion.active` changes with the event.
                    let camera = state.camera(event.camera.as_str());
                    send(&mut socket, &WsMessage::Event { event: EventDto::from(&event) }).await
                        && match camera {
                            Some(c) => send(&mut socket, &WsMessage::Camera { camera: Box::new(state.camera_dto(&c)) }).await,
                            None => true,
                        }
                }
                Ok(NvrEvent::MotionSettingsChanged { camera, .. }) => match state.camera(camera.as_str()) {
                    Some(c) => send(&mut socket, &WsMessage::Camera { camera: Box::new(state.camera_dto(&c)) }).await,
                    None => true,
                },
                Ok(NvrEvent::Crossing { camera, line, forward, at_ms, today, object, .. }) => {
                    let crossing = crate::dto::CrossingDto {
                        camera: camera.to_string(),
                        line,
                        direction: if forward { "forward" } else { "backward" },
                        at_ms,
                        today,
                        object,
                    };
                    send(&mut socket, &WsMessage::Crossing { crossing }).await
                }
                // The state of the destinations comes with `system`; the presence counts with `GET /crossings`.
                Ok(NvrEvent::Snapshot { .. } | NvrEvent::ObjectSnapshot { .. } | NvrEvent::StorageState { .. } | NvrEvent::Presence { .. }) => true,
                // Missed events: resend the full state.
                Err(RecvError::Lagged(_)) => send_snapshot(&state, &mut socket, who.as_ref()).await,
                Err(RecvError::Closed) => return,
            },
        };
        if !ok {
            return;
        }
    }
}
