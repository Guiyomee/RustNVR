//! Motion-triggered recording: decides which frames reach the recorder.
//!
//! Outside events, the gate keeps the most recent frames in memory, from the
//! last key frame at or before the pre-roll start (a segment must start on a
//! key frame), within [`MAX_BUFFER_BYTES`]. When an event opens the gate, the
//! buffered frames are written first, then the live ones, until the event
//! ends plus the post-roll; then the segment is closed.
//!
//! Times are the wall-clock reception times of the frames, compared with the
//! deadline set by detection ([`RecordTrigger`]).

use rustnvr_core::VideoFrame;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

/// Memory bound of the pre-roll buffer, per recorder.
pub const MAX_BUFFER_BYTES: usize = 32 << 20;

/// Shared between detection and the recorders of a camera: frames received
/// up to the deadline are recorded.
#[derive(Debug)]
pub struct RecordTrigger {
    until_ms: AtomicI64,
}

impl Default for RecordTrigger {
    fn default() -> Self {
        Self {
            until_ms: AtomicI64::new(i64::MIN),
        }
    }
}

impl RecordTrigger {
    /// Records until further notice (an event started).
    pub fn hold(&self) {
        self.until_ms.store(i64::MAX, Ordering::Relaxed);
    }

    /// Records frames received up to `until_ms` (event end plus post-roll).
    pub fn release_at(&self, until_ms: i64) {
        self.until_ms.store(until_ms, Ordering::Relaxed);
    }

    /// Current deadline in milliseconds since the Unix epoch.
    pub fn deadline_ms(&self) -> i64 {
        self.until_ms.load(Ordering::Relaxed)
    }
}

/// What the recorder must do with a frame.
#[derive(Debug)]
pub enum GateAction {
    /// Write these frames, in order.
    Write(Vec<VideoFrame>),
    /// Close the open segment (the frame was kept for a later pre-roll).
    Close,
    /// Nothing (the frame was kept for a later pre-roll).
    Hold,
}

/// Frame filter of one motion-triggered recorder.
pub struct MotionGate {
    trigger: std::sync::Arc<RecordTrigger>,
    pre_roll_ms: i64,
    buffer: VecDeque<VideoFrame>,
    bytes: usize,
    open: bool,
}

fn wall_ms(f: &VideoFrame) -> i64 {
    f.wall_time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl MotionGate {
    /// Gate following `trigger`, writing `pre_roll` of video before each opening.
    pub fn new(trigger: std::sync::Arc<RecordTrigger>, pre_roll: Duration) -> Self {
        Self {
            trigger,
            pre_roll_ms: i64::try_from(pre_roll.as_millis()).unwrap_or(i64::MAX),
            buffer: VecDeque::new(),
            bytes: 0,
            open: false,
        }
    }

    fn drop_front(&mut self) {
        if let Some(f) = self.buffer.pop_front() {
            self.bytes -= f.data.len();
        }
    }

    /// Drops frames up to (excluding) the next key frame.
    fn drop_until_next_key(&mut self) {
        self.drop_front();
        while self.buffer.front().is_some_and(|f| !f.is_key) {
            self.drop_front();
        }
    }

    fn keep(&mut self, frame: VideoFrame) {
        let now = wall_ms(&frame);
        if self
            .buffer
            .front()
            .is_some_and(|f| f.session != frame.session)
        {
            self.buffer.clear();
            self.bytes = 0;
        }
        if self.buffer.is_empty() && !frame.is_key {
            return; // a segment can only start on a key frame
        }
        self.bytes += frame.data.len();
        self.buffer.push_back(frame);
        // Keep the newest key frame that is at or before the pre-roll start.
        let start = now.saturating_sub(self.pre_roll_ms);
        loop {
            let next_key = self
                .buffer
                .iter()
                .skip(1)
                .position(|f| f.is_key)
                .map(|i| i + 1);
            match next_key {
                Some(i) if wall_ms(&self.buffer[i]) <= start => {
                    for _ in 0..i {
                        self.drop_front();
                    }
                }
                _ => break,
            }
        }
        while self.bytes > MAX_BUFFER_BYTES && !self.buffer.is_empty() {
            self.drop_until_next_key();
        }
    }

    /// Takes the next frame of the stream.
    pub fn push(&mut self, frame: VideoFrame) -> GateAction {
        let recording = wall_ms(&frame) <= self.trigger.deadline_ms();
        match (recording, self.open) {
            (true, true) => GateAction::Write(vec![frame]),
            (true, false) => {
                self.open = true;
                self.bytes = 0;
                let mut frames: Vec<VideoFrame> = self.buffer.drain(..).collect();
                frames.push(frame);
                GateAction::Write(frames)
            }
            (false, open) => {
                self.open = false;
                self.keep(frame);
                if open {
                    GateAction::Close
                } else {
                    GateAction::Hold
                }
            }
        }
    }

    /// Forgets buffered frames (the session ended).
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use rustnvr_core::{VideoCodec, VideoParams};
    use std::sync::Arc;

    fn frame(seq: u64, ms: i64, is_key: bool) -> VideoFrame {
        VideoFrame {
            session: 1,
            seq,
            params: Arc::new(VideoParams {
                codec: VideoCodec::H264,
                rfc6381_codec: String::new(),
                width: 320,
                height: 240,
                clock_rate: 90_000,
                sample_entry: Bytes::new(),
            }),
            pts: ms * 90,
            is_key,
            wall_time: UNIX_EPOCH + Duration::from_millis(ms as u64),
            loss: 0,
            audio: None,
            data: Bytes::from(vec![0; 100]),
        }
    }

    fn seqs(action: &GateAction) -> Vec<u64> {
        match action {
            GateAction::Write(f) => f.iter().map(|f| f.seq).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn writes_pre_roll_from_a_key_frame_then_closes_after_deadline() {
        let trigger = Arc::new(RecordTrigger::default());
        let mut gate = MotionGate::new(trigger.clone(), Duration::from_secs(3));
        // Key frame every 2 s, 1 frame per second.
        let mut seq = 0;
        let mut feed = |gate: &mut MotionGate, ms: i64| {
            seq += 1;
            gate.push(frame(seq, ms, ms % 2000 == 0))
        };
        for ms in (0..=10_000).step_by(1000) {
            assert!(matches!(feed(&mut gate, ms), GateAction::Hold));
        }
        // At 10 s the pre-roll starts at 7 s: kept from the key frame at 6 s.
        assert_eq!(gate.buffer.front().map(wall_ms), Some(6000));
        trigger.hold();
        let a = feed(&mut gate, 11_000);
        assert_eq!(seqs(&a), vec![7, 8, 9, 10, 11, 12]); // 6 s .. 11 s
        assert_eq!(seqs(&feed(&mut gate, 12_000)), vec![13]);
        trigger.release_at(13_000);
        assert_eq!(seqs(&feed(&mut gate, 13_000)), vec![14]);
        assert!(matches!(feed(&mut gate, 14_000), GateAction::Close));
        assert!(matches!(feed(&mut gate, 15_000), GateAction::Hold));
    }

    #[test]
    fn never_buffers_without_a_key_frame_and_stays_bounded() {
        let trigger = Arc::new(RecordTrigger::default());
        let mut gate = MotionGate::new(trigger, Duration::from_secs(5));
        gate.push(frame(1, 0, false));
        assert!(gate.buffer.is_empty());
        let mut big = frame(2, 1, true);
        big.data = Bytes::from(vec![0; MAX_BUFFER_BYTES + 1]);
        gate.push(big);
        assert!(gate.bytes <= MAX_BUFFER_BYTES);
    }
}
