//! Lock-free counters of one stream.

use rustnvr_core::VideoFrame;
use std::sync::atomic::{AtomicU64, Ordering};

/// Counters updated by the supervisor and read by metrics reporters.
#[derive(Debug, Default)]
pub struct StreamMetrics {
    frames: AtomicU64,
    key_frames: AtomicU64,
    bytes: AtomicU64,
    lost_packets: AtomicU64,
    sessions: AtomicU64,
    failures: AtomicU64,
    dropped_frames: AtomicU64,
}

/// Point-in-time copy of [`StreamMetrics`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamMetricsSnapshot {
    /// Frames received.
    pub frames: u64,
    /// Key frames received.
    pub key_frames: u64,
    /// Payload bytes received.
    pub bytes: u64,
    /// RTP packets reported lost.
    pub lost_packets: u64,
    /// Sessions established.
    pub sessions: u64,
    /// Failed attempts and lost sessions.
    pub failures: u64,
    /// Frame deliveries dropped because a consumer queue was full.
    pub dropped_frames: u64,
}

impl StreamMetrics {
    pub(crate) fn record_frame(&self, frame: &VideoFrame) {
        self.frames.fetch_add(1, Ordering::Relaxed);
        if frame.is_key {
            self.key_frames.fetch_add(1, Ordering::Relaxed);
        }
        self.bytes
            .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
        self.lost_packets
            .fetch_add(frame.loss.into(), Ordering::Relaxed);
    }

    pub(crate) fn record_session(&self) {
        self.sessions.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_dropped(&self, n: usize) {
        self.dropped_frames.fetch_add(n as u64, Ordering::Relaxed);
    }

    /// Reads all counters.
    pub fn snapshot(&self) -> StreamMetricsSnapshot {
        StreamMetricsSnapshot {
            frames: self.frames.load(Ordering::Relaxed),
            key_frames: self.key_frames.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            lost_packets: self.lost_packets.load(Ordering::Relaxed),
            sessions: self.sessions.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
        }
    }
}
