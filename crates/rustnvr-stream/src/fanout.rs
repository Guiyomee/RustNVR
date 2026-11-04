//! Bounded, non-blocking distribution of frames to consumers.
//!
//! Each consumer has its own bounded queue. Ingestion never waits for a
//! consumer: if a queue is full the frame is dropped for that consumer only
//! and counted. Consumers detect drops through gaps in [`VideoFrame::seq`].

use crate::live::LiveTap;
use rustnvr_core::{StreamItem, VideoFrame};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// How long [`FrameFanout::end_session`] waits for room in each queue.
const SESSION_END_TIMEOUT: Duration = Duration::from_secs(1);

/// Delivery counters of one subscriber.
#[derive(Debug, Default)]
pub struct SubscriberStats {
    /// Frames queued.
    pub delivered: AtomicU64,
    /// Frames dropped because the queue was full.
    pub dropped: AtomicU64,
}

struct Subscriber {
    name: String,
    tx: mpsc::Sender<StreamItem>,
    stats: Arc<SubscriberStats>,
}

/// Distributes frames of one stream to its subscribers.
#[derive(Default)]
pub struct FrameFanout {
    subscribers: Vec<Subscriber>,
    live: Option<LiveTap>,
}

impl FrameFanout {
    /// Creates a fan-out without subscribers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a subscriber with a queue of `capacity` items.
    pub fn subscribe(
        &mut self,
        name: impl Into<String>,
        capacity: usize,
    ) -> (mpsc::Receiver<StreamItem>, Arc<SubscriberStats>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        let stats = Arc::new(SubscriberStats::default());
        self.subscribers.push(Subscriber {
            name: name.into(),
            tx,
            stats: stats.clone(),
        });
        (rx, stats)
    }

    /// Number of live subscribers.
    pub fn len(&self) -> usize {
        self.subscribers.len()
    }

    /// True without subscribers.
    pub fn is_empty(&self) -> bool {
        self.subscribers.is_empty()
    }

    /// Returns the live tap of this stream, creating it on first call.
    pub fn live_tap(&mut self) -> LiveTap {
        self.live.get_or_insert_with(LiveTap::new).clone()
    }

    /// Offers `frame` to the live tap and to every subscriber without
    /// waiting. Returns the number of subscribers that dropped it.
    /// Subscribers whose receiver was dropped are removed.
    pub fn send(&mut self, frame: &VideoFrame) -> usize {
        if let Some(live) = &self.live {
            live.publish(frame);
        }
        let mut dropped = 0;
        self.subscribers.retain(|s| {
            match s.tx.try_send(StreamItem::Frame(frame.clone())) {
                Ok(()) => {
                    s.stats.delivered.fetch_add(1, Ordering::Relaxed);
                    true
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    let total = s.stats.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                    // Log the first drop and then every 100th to avoid log floods.
                    if total == 1 || total.is_multiple_of(100) {
                        tracing::warn!(subscriber = %s.name, dropped_total = total, "consumer queue full, dropping frame");
                    }
                    dropped += 1;
                    true
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!(subscriber = %s.name, "subscriber gone");
                    false
                }
            }
        });
        dropped
    }

    /// Notifies subscribers that `session` ended, waiting a bounded time for
    /// queue space (a slow consumer would otherwise miss the notification).
    pub async fn end_session(&mut self, session: u64) {
        for s in &self.subscribers {
            let item = StreamItem::SessionEnd { session };
            if tokio::time::timeout(SESSION_END_TIMEOUT, s.tx.send(item))
                .await
                .is_err()
            {
                tracing::warn!(subscriber = %s.name, "could not deliver end of session");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use rustnvr_core::{VideoCodec, VideoParams};

    fn frame(seq: u64) -> VideoFrame {
        VideoFrame {
            session: 1,
            seq,
            params: Arc::new(VideoParams {
                codec: VideoCodec::H264,
                rfc6381_codec: "avc1.42C01E".into(),
                width: 320,
                height: 240,
                clock_rate: 90_000,
                sample_entry: Bytes::new(),
            }),
            pts: seq as i64 * 9000,
            is_key: seq == 0,
            wall_time: std::time::SystemTime::now(),
            loss: 0,
            audio: None,
            data: Bytes::from_static(b"x"),
        }
    }

    #[tokio::test]
    async fn slow_subscriber_does_not_affect_others() {
        let mut fanout = FrameFanout::new();
        let (mut fast, _) = fanout.subscribe("fast", 100);
        let (_slow, slow_stats) = fanout.subscribe("slow", 2);
        for seq in 0..10 {
            fanout.send(&frame(seq));
            assert!(matches!(fast.recv().await, Some(StreamItem::Frame(f)) if f.seq == seq));
        }
        assert_eq!(slow_stats.delivered.load(Ordering::Relaxed), 2);
        assert_eq!(slow_stats.dropped.load(Ordering::Relaxed), 8);
    }

    #[tokio::test]
    async fn closed_subscribers_are_removed() {
        let mut fanout = FrameFanout::new();
        let (rx, _) = fanout.subscribe("gone", 4);
        drop(rx);
        fanout.send(&frame(0));
        assert!(fanout.is_empty());
    }
}
