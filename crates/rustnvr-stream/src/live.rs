//! Live tap: lets viewers join a stream at any time, at no cost when nobody watches.
//!
//! Frames are published on a bounded broadcast channel only while there is
//! at least one viewer. A slow viewer lags (its receiver reports how many
//! frames it missed) without slowing ingestion or other consumers.
//!
//! To show a picture immediately instead of waiting for the next key frame
//! (cameras commonly use 2 to 10 s GOPs), the tap keeps the frames since the
//! last key frame. That cache is capped at [`GOP_CACHE_MAX_BYTES`]; beyond it
//! the cache is dropped and new viewers wait for the next key frame.

use rustnvr_core::VideoFrame;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Frames buffered per viewer before it is considered lagging.
pub const LIVE_CHANNEL_CAPACITY: usize = 128;
/// Memory bound of the cached group of pictures, per camera.
pub const GOP_CACHE_MAX_BYTES: usize = 4 << 20;

#[derive(Default)]
struct GopCache {
    frames: Vec<VideoFrame>,
    bytes: usize,
    /// False when the current GOP did not fit: wait for the next key frame.
    valid: bool,
}

struct Inner {
    tx: broadcast::Sender<VideoFrame>,
    gop: Mutex<GopCache>,
}

/// Shared handle to the live frames of one stream.
#[derive(Clone)]
pub struct LiveTap {
    inner: Arc<Inner>,
}

impl Default for LiveTap {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveTap {
    /// Creates a tap without viewers.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                tx: broadcast::channel(LIVE_CHANNEL_CAPACITY).0,
                gop: Mutex::new(GopCache::default()),
            }),
        }
    }

    fn gop(&self) -> std::sync::MutexGuard<'_, GopCache> {
        // The lock is never held across a panic-prone operation; recover anyway.
        self.inner.gop.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records `frame` in the GOP cache and sends it to current viewers.
    ///
    /// Called by the stream fan-out for camera frames, and by producers of
    /// derived streams (e.g. an H.264 conversion).
    pub fn publish(&self, frame: &VideoFrame) {
        let mut gop = self.gop();
        let other_session = gop
            .frames
            .first()
            .is_some_and(|f| f.session != frame.session);
        if frame.is_key || other_session {
            gop.frames.clear();
            gop.bytes = 0;
            gop.valid = frame.is_key;
        }
        if gop.valid {
            if gop.bytes + frame.data.len() > GOP_CACHE_MAX_BYTES {
                gop.frames.clear();
                gop.bytes = 0;
                gop.valid = false;
            } else {
                gop.bytes += frame.data.len();
                gop.frames.push(frame.clone());
            }
        }
        // Sending under the lock makes `subscribe` atomic with respect to
        // publishing: a viewer gets each frame exactly once.
        if self.inner.tx.receiver_count() > 0 {
            let _ = self.inner.tx.send(frame.clone());
        }
    }

    /// Joins the stream: returns the frames since the last key frame (possibly
    /// empty) and a receiver for the following ones.
    pub fn subscribe(&self) -> (Vec<VideoFrame>, broadcast::Receiver<VideoFrame>) {
        let gop = self.gop();
        let backlog = if gop.valid {
            gop.frames.clone()
        } else {
            Vec::new()
        };
        (backlog, self.inner.tx.subscribe())
    }

    /// Whether both handles refer to the same tap.
    pub fn same_as(&self, other: &LiveTap) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Current number of viewers.
    pub fn viewers(&self) -> usize {
        self.inner.tx.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use rustnvr_core::{VideoCodec, VideoParams};
    use std::time::SystemTime;

    fn frame(session: u64, seq: u64, is_key: bool, size: usize) -> VideoFrame {
        VideoFrame {
            session,
            seq,
            params: Arc::new(VideoParams {
                codec: VideoCodec::H264,
                rfc6381_codec: "avc1.42C01E".into(),
                width: 320,
                height: 240,
                clock_rate: 90_000,
                sample_entry: Bytes::new(),
            }),
            pts: seq as i64 * 3600,
            is_key,
            wall_time: SystemTime::now(),
            loss: 0,
            audio: None,
            data: Bytes::from(vec![0; size]),
        }
    }

    #[tokio::test]
    async fn late_viewer_starts_at_last_key_frame() {
        let tap = LiveTap::new();
        for seq in 0..25 {
            tap.publish(&frame(1, seq, seq % 10 == 0, 100));
        }
        let (backlog, mut rx) = tap.subscribe();
        // Key frame at 20, then 21..24.
        assert_eq!(
            backlog.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![20, 21, 22, 23, 24]
        );
        assert!(backlog[0].is_key);
        tap.publish(&frame(1, 25, false, 100));
        assert_eq!(rx.recv().await.map(|f| f.seq).ok(), Some(25));
        assert_eq!(tap.viewers(), 1);
    }

    #[tokio::test]
    async fn oversized_gop_and_new_sessions_reset_the_cache() {
        let tap = LiveTap::new();
        tap.publish(&frame(1, 0, true, GOP_CACHE_MAX_BYTES));
        tap.publish(&frame(1, 1, false, 1));
        assert!(
            tap.subscribe().0.is_empty(),
            "over the cap: wait for next key"
        );
        tap.publish(&frame(1, 2, true, 10));
        assert_eq!(tap.subscribe().0.len(), 1);
        // A new session starting without a key frame invalidates the cache.
        tap.publish(&frame(2, 0, false, 10));
        assert!(tap.subscribe().0.is_empty());
    }

    #[test]
    fn no_cost_without_viewers() {
        let tap = LiveTap::new();
        tap.publish(&frame(1, 0, true, 10));
        assert_eq!(tap.viewers(), 0);
    }
}
