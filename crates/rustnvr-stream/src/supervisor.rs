//! Long-lived supervision of one camera stream.

use crate::backoff::{Backoff, BackoffConfig};
use crate::error::StreamError;
use crate::fanout::FrameFanout;
use crate::metrics::StreamMetrics;
use crate::session::{RtspSession, SessionOptions, offers_audio};
use retina::client::SessionGroup;
use rustnvr_core::{CameraId, EventBus, NvrEvent, RtspTarget, StreamRole, StreamState};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
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
        audio_checks: 0,
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
    /// Audio checks made since the last session with sound (spaces them out
    /// across reconnections, for a camera that has no microphone).
    audio_checks: u32,
}

/// How a streaming session ended.
enum Outcome {
    Cancelled,
    Failed(StreamError),
    /// The camera now offers the audio track it had left out.
    Reconnect,
}

/// Wait before the `n`-th audio check (from 0), in multiples of `first`
/// (1 min by default: 1, 2, 5, 15 min, then every hour): soon after a camera
/// wakes up, then rarely for one without a microphone.
fn audio_check_delay(first: Duration, n: u32) -> Duration {
    const FACTORS: [u32; 4] = [1, 2, 5, 15];
    first.saturating_mul(FACTORS.get(n as usize).copied().unwrap_or(60))
}

/// Checks now and then, while a session has no sound, whether the camera
/// offers an audio track again.
struct AudioCheck {
    first: Duration,
    next: Option<Instant>,
    task: Option<JoinHandle<bool>>,
    /// Checks made since the last session with sound.
    count: u32,
}

impl AudioCheck {
    fn new(missing: bool, count: u32, first: Duration) -> Self {
        let count = if missing { count } else { 0 };
        Self {
            first,
            next: missing.then(|| Instant::now() + audio_check_delay(first, count)),
            task: None,
            count,
        }
    }

    /// Completes when the camera offers an audio track; never without a
    /// check to make. Cancel-safe: a check under way is kept.
    async fn offered(&mut self, target: &RtspTarget, opts: &SessionOptions) {
        loop {
            if let Some(task) = &mut self.task {
                let found = task.await.unwrap_or(false);
                self.task = None;
                self.count = self.count.saturating_add(1);
                if found {
                    return;
                }
                tracing::debug!(checks = self.count, "still no audio track");
                self.next = Some(Instant::now() + audio_check_delay(self.first, self.count));
            }
            let Some(at) = self.next else {
                return std::future::pending().await;
            };
            tokio::time::sleep_until(at).await;
            self.next = None;
            let (target, opts) = (target.clone(), opts.clone());
            self.task = Some(tokio::spawn(
                async move { offers_audio(&target, &opts).await },
            ));
        }
    }
}

impl Drop for AudioCheck {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
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
                        Outcome::Failed(e) => e,
                        Outcome::Reconnect => {
                            // Not a failure: reconnect at once, with sound.
                            attempt = 0;
                            continue;
                        }
                        Outcome::Cancelled => break,
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

    /// Forwards frames until an error, cancellation, or the camera offers the
    /// audio track it had left out.
    async fn stream(&mut self, mut session: RtspSession, cancel: &CancellationToken) -> Outcome {
        let mut announced = false;
        let mut audio = AudioCheck::new(
            session.audio_missing(),
            self.audio_checks,
            self.config.session.audio_recheck,
        );
        if session.audio_missing() {
            info!("the camera offers no audio track: checking again later");
        }
        let outcome = loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => break Outcome::Cancelled,
                () = audio.offered(&self.config.target, &self.config.session) => {
                    info!("the camera now offers an audio track: reconnecting to record the sound");
                    break Outcome::Reconnect;
                }
                f = session.next_frame() => f,
            };
            let frame = match frame {
                Ok(f) => f,
                Err(e) => break Outcome::Failed(e),
            };
            if !announced {
                announced = true;
                let p = &frame.params;
                let sound = frame
                    .audio
                    .as_ref()
                    .map(|a| format!("{} {} Hz", a.params.rfc6381_codec, a.params.clock_rate));
                info!(session = frame.session, codec = %p.codec, rfc6381 = %p.rfc6381_codec, width = p.width, height = p.height, audio = sound.as_deref().unwrap_or("none"), "streaming");
                self.set_state(StreamState::Streaming {
                    session: frame.session,
                    codec: p.codec,
                    width: p.width,
                    height: p.height,
                    audio: sound,
                    since: SystemTime::now(),
                });
            }
            self.metrics.record_frame(&frame);
            let dropped = self.fanout.send(&frame);
            if dropped > 0 {
                self.metrics.record_dropped(dropped);
            }
        };
        self.audio_checks = audio.count;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_checks_space_out() {
        let first = Duration::from_secs(60);
        let minutes: Vec<u64> = (0..6)
            .map(|n| audio_check_delay(first, n).as_secs() / 60)
            .collect();
        assert_eq!(minutes, [1, 2, 5, 15, 60, 60]);
    }
}
