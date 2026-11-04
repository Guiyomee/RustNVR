//! Runs a [`RecorderCore`] on a dedicated OS thread.
//!
//! The thread blocks on the frame queue and on disk I/O, keeping both off the
//! async runtime. It exits when every sender of the queue has been dropped
//! (the stream supervisor stopped), after closing the open segment.

use crate::gate::{GateAction, MotionGate};
use crate::recorder::{RecorderCore, RecorderMetrics};
use rustnvr_core::{EventBus, StreamItem};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Handle to a recorder thread.
pub struct RecorderHandle {
    done: oneshot::Receiver<()>,
    metrics: Arc<RecorderMetrics>,
}

impl RecorderHandle {
    /// Counters of the recorder.
    pub fn metrics(&self) -> &Arc<RecorderMetrics> {
        &self.metrics
    }

    /// Waits for the thread to finish, at most `timeout`. Returns false on timeout.
    pub async fn wait(self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.done).await.is_ok()
    }
}

/// Starts a thread named `rec-<camera>` that records items from `rx`; with a
/// `gate`, only the frames it lets through (motion-triggered recording).
pub fn spawn_recorder(
    mut core: RecorderCore,
    mut rx: mpsc::Receiver<StreamItem>,
    events: EventBus,
    mut gate: Option<MotionGate>,
) -> io::Result<RecorderHandle> {
    let metrics = core.metrics();
    let (done_tx, done) = oneshot::channel();
    let name = format!("rec-{}", core.camera());
    std::thread::Builder::new().name(name).spawn(move || {
        let publish = |core: &mut RecorderCore| {
            for event in core.drain_events() {
                // A bus without subscribers is fine.
                let _ = events.send(event);
            }
        };
        while let Some(item) = rx.blocking_recv() {
            match (item, &mut gate) {
                (StreamItem::Frame(frame), None) => core.handle_frame(frame),
                (StreamItem::Frame(frame), Some(gate)) => match gate.push(frame) {
                    GateAction::Write(frames) => {
                        for f in frames {
                            core.handle_frame(f);
                        }
                    }
                    GateAction::Close => core.shutdown(),
                    GateAction::Hold => {}
                },
                (StreamItem::SessionEnd { session }, gate) => {
                    if let Some(g) = gate {
                        g.reset();
                    }
                    core.handle_session_end(session);
                }
            }
            publish(&mut core);
        }
        core.shutdown();
        publish(&mut core);
        let _ = done_tx.send(());
    })?;
    Ok(RecorderHandle { done, metrics })
}
