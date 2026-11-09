//! Long-lived supervision of one camera stream.

use crate::backoff::{Backoff, BackoffConfig};
use crate::error::StreamError;
use crate::fanout::FrameFanout;
use crate::metrics::StreamMetrics;
use crate::session::{RtspSession, SessionOptions};
use retina::client::SessionGroup;
use rustnvr_core::{CameraId, EventBus, NvrEvent, RtspTarget, StreamRole, StreamState};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument as _, info, info_span, warn};

/// What a supervisor connects to and how.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// Camera.
    pub camera: CameraId,
    /// Stream of the camera.
    pub role: StreamRole,
    /// Endpoint and credentials.
    pub target: RtspTarget,
    /// Session options.
    pub session: SessionOptions,
    /// Reconnection policy.
    pub backoff: BackoffConfig,
}

/// Handle to a running supervisor.
pub struct StreamHandle {
    /// Latest state, updated on every transition.
    pub state: watch::Receiver<StreamState>,
    /// Counters.
    pub metrics: Arc<StreamMetrics>,
    /// The supervisor task; completes after cancellation.
    pub task: JoinHandle<()>,
}

/// Spawns a supervisor that keeps the stream connected until `cancel` fires.
///
/// Frames go to `fanout`; state transitions are published on the watch
/// channel of the returned handle and on `events`.
pub fn spawn_supervisor(
    config: SupervisorConfig,
    fanout: FrameFanout,
    events: EventBus,
    cancel: CancellationToken,
) -> StreamHandle {
    let (state_tx, state) = watch::channel(StreamState::Idle);
    let metrics = Arc::new(StreamMetrics::default());
    let span = info_span!("stream", camera = %config.camera, role = %config.role);
    let supervisor = Supervisor {
        config,
        fanout,
        events,
        state_tx,
        metrics: metrics.clone(),
    };
    let task = tokio::spawn(supervisor.run(cancel).instrument(span));
    StreamHandle {
        state,
        metrics,
        task,
    }
}

struct Supervisor {
    config: SupervisorConfig,
    fanout: FrameFanout,
    events: EventBus,
    state_tx: watch::Sender<StreamState>,
    metrics: Arc<StreamMetrics>,
}

impl Supervisor {
    fn set_state(&self, state: StreamState) {
        self.state_tx.send_replace(state.clone());
        // No subscriber is not an error.
        let _ = self.events.send(NvrEvent::StreamState {
            camera: self.config.camera.clone(),
            role: self.config.role,
            state,
        });
    }

    async fn run(mut self, cancel: CancellationToken) {
        let group = Arc::new(SessionGroup::default().named(self.config.camera.to_string()));
        let mut backoff = Backoff::new(self.config.backoff);
        let mut attempt: u32 = 0;
        let mut session_id: u64 = 0;
        info!(endpoint = %self.config.target.redacted(), "starting stream supervisor");

        loop {
            attempt = attempt.saturating_add(1);
            self.set_state(StreamState::Connecting { attempt });

            // Servers with the live555 stale-session bug keep sending on old
            // connections; Retina tracks them and tells us when it is safe.
            let stale = group.stale_sessions();
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = group.await_stale_sessions(&stale) => {}
            }

            session_id += 1;
            let connected = tokio::select! {
                _ = cancel.cancelled() => break,
                r = RtspSession::connect_in_group(&self.config.target, &self.config.session, group.clone(), session_id) => r,
            };
            let error = match connected {
                Ok(session) => {
                    self.metrics.record_session();
                    let started = Instant::now();
                    let outcome = self.stream(session, &cancel).await;
                    self.fanout.end_session(session_id).await;
                    if started.elapsed() >= self.config.backoff.stable_after {
                        backoff.reset();
                        attempt = 1;
                    }
                    match outcome {
                        Some(e) => e,
                        None => break,
                    }
                }
                Err(e) => e,
            };

            self.metrics.record_failure();
            let retry_in = backoff.next_delay();
            warn!(attempt, retry_in_ms = retry_in.as_millis() as u64, error = %error.one_line(), "stream unavailable");
            self.set_state(StreamState::Retrying {
                attempt,
                retry_in,
                error: error.summary(),
            });
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(retry_in) => {}
            }
        }
        self.set_state(StreamState::Stopped);
        info!("stream supervisor stopped");
    }

    /// Forwards frames until an error (returned) or cancellation (`None`).
    async fn stream(
        &mut self,
        mut session: RtspSession,
        cancel: &CancellationToken,
    ) -> Option<StreamError> {
        let mut announced = false;
        loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                f = session.next_frame() => f,
            };
            let frame = match frame {
                Ok(f) => f,
                Err(e) => return Some(e),
            };
            if !announced {
                announced = true;
                let p = &frame.params;
                let audio = frame
                    .audio
                    .as_ref()
                    .map(|a| format!("{} {} Hz", a.params.rfc6381_codec, a.params.clock_rate));
                info!(session = frame.session, codec = %p.codec, rfc6381 = %p.rfc6381_codec, width = p.width, height = p.height, audio = audio.as_deref().unwrap_or("none"), "streaming");
                self.set_state(StreamState::Streaming {
                    session: frame.session,
                    codec: p.codec,
                    width: p.width,
                    height: p.height,
                    audio,
                    since: SystemTime::now(),
                });
            }
            self.metrics.record_frame(&frame);
            let dropped = self.fanout.send(&frame);
            if dropped > 0 {
                self.metrics.record_dropped(dropped);
            }
        }
    }
}
