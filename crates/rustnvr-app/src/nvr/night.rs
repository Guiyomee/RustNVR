//! Night vision of each camera, from its picture
//! ([`rustnvr_motion::night`]): every minute, the key frame of the sub
//! stream (else of the main one) of each running camera is measured. The
//! state is shown in the interface and published to MQTT (« Vision
//! nocturne »).

use super::Shared;
use rustnvr_core::CameraId;
use rustnvr_motion::night::{NightWatch, measure};
use rustnvr_stream::LiveTap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// How often each camera is measured.
const PERIOD: Duration = Duration::from_secs(60);

/// Measures the running cameras until `cancel`.
pub(super) async fn run(shared: Arc<Shared>, cancel: CancellationToken) {
    if !shared.ffmpeg_ok {
        return;
    }
    let mut watches: HashMap<CameraId, NightWatch> = HashMap::new();
    let mut ticker = tokio::time::interval(PERIOD);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = ticker.tick() => {}
        }
        // The lighter stream: the sub stream when there is one.
        let taps: Vec<(CameraId, LiveTap)> = {
            let state = shared.state.lock().await;
            state
                .running
                .iter()
                .map(|(id, rt)| {
                    let tap = rt
                        .sub
                        .as_ref()
                        .map_or_else(|| rt.live.clone(), |(_, t)| t.clone());
                    (id.clone(), tap)
                })
                .collect()
        };
        // Stopped cameras: their state is no longer known.
        watches.retain(|id, _| taps.iter().any(|(t, _)| t == id));
        for (id, tap) in taps {
            match measure(&shared.ffmpeg, &tap).await {
                Ok(colour) => {
                    debug!(camera = %id, colour, "night vision measured");
                    if let Some(night) = watches.entry(id.clone()).or_default().reading(colour) {
                        info!(camera = %id, night, colour, "night vision");
                    }
                }
                // Offline, or no key frame kept: the state stays as it was.
                Err(e) => debug!(camera = %id, error = %e, "night vision not measured"),
            }
        }
        let now: HashMap<CameraId, bool> = watches
            .iter()
            .filter_map(|(id, w)| w.night().map(|n| (id.clone(), n)))
            .collect();
        let mut state = shared.state.lock().await;
        if state.night != now {
            state.night = now;
            shared.publish(&state);
        }
    }
}
