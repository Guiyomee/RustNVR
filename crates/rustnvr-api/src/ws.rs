//! `/api/v1/ws`: pushes system status, camera states, finished segments and
//! detection events.

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

pub(crate) async fn handler(State(state): State<Arc<ApiState>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| client(state, socket))
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

async fn send_snapshot(state: &ApiState, socket: &mut WebSocket) -> bool {
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

async fn client(state: Arc<ApiState>, mut socket: WebSocket) {
    let mut events = state.events.subscribe();
    if !send_snapshot(&state, &mut socket).await {
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
            _ = ticker.tick() => send(&mut socket, &WsMessage::System { system: Box::new(state.system_dto()) }).await,
            event = events.recv() => match event {
                Ok(NvrEvent::StreamState { camera, .. }) => match state.camera(camera.as_str()) {
                    Some(c) => send(&mut socket, &WsMessage::Camera { camera: Box::new(state.camera_dto(&c)) }).await,
                    None => true,
                },
                Ok(NvrEvent::CamerasChanged) => {
                    let cameras = state.cameras().iter().map(|c| state.camera_dto(c)).collect();
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
                Err(RecvError::Lagged(_)) => send_snapshot(&state, &mut socket).await,
                Err(RecvError::Closed) => return,
            },
        };
        if !ok {
            return;
        }
    }
}
