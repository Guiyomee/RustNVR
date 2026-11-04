//! Segmentation state machine.
//!
//! [`RecorderCore`] is synchronous and performs blocking I/O: it runs on a
//! dedicated thread (see [`crate::driver`]), never on the async runtime.
//!
//! Rules:
//!
//! * a segment always starts with a key frame;
//! * a frame's duration is the difference with the next frame's timestamp, so
//!   the latest frame is held back until its successor arrives;
//! * a fragment is written when a key frame arrives after at least the target
//!   fragment duration, or unconditionally after twice that duration (long GOPs);
//! * a segment is closed on the first key frame after the segment duration,
//!   and immediately on any discontinuity: new session, parameter change,
//!   dropped frames, timestamp going backwards or jumping too far;
//! * after a write error the segment is abandoned (complete fragments are
//!   kept), and recording resumes on a key frame after a growing delay.

use crate::fmp4::{self, SampleMeta};
use crate::store::{SegmentFile, SegmentStore};
use bytes::Bytes;
use rustnvr_core::{AudioParams, CameraId, NvrEvent, SegmentInfo, VideoFrame, VideoParams};
use std::io::{self, Write as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// Recorder settings.
#[derive(Clone, Debug)]
pub struct RecorderSettings {
    /// Target segment duration.
    pub segment_duration: Duration,
    /// Target fragment duration.
    pub fragment_duration: Duration,
    /// Larger timestamp jumps start a new segment.
    pub max_timestamp_jump: Duration,
    /// Initial delay before retrying after a write error.
    pub retry_after_error: Duration,
}

impl From<&rustnvr_core::config::RecordingConfig> for RecorderSettings {
    fn from(c: &rustnvr_core::config::RecordingConfig) -> Self {
        Self {
            segment_duration: c.segment_duration(),
            fragment_duration: c.fragment_duration(),
            max_timestamp_jump: c.max_timestamp_jump(),
            retry_after_error: c.retry_after_error(),
        }
    }
}

/// Counters of one recorder.
#[derive(Debug, Default)]
pub struct RecorderMetrics {
    /// Segments completed.
    pub segments: AtomicU64,
    /// Frames written to disk.
    pub frames_written: AtomicU64,
    /// Bytes written to disk (including container overhead).
    pub bytes_written: AtomicU64,
    /// Frames discarded while waiting for a key frame or after an error.
    pub frames_skipped: AtomicU64,
    /// Write errors.
    pub write_errors: AtomicU64,
    /// Segments closed early because of a discontinuity.
    pub discontinuities: AtomicU64,
}

struct Sample {
    data: Bytes,
    duration: u32,
    is_key: bool,
}

/// Audio track of the open segment.
struct ActiveAudio {
    params: Arc<AudioParams>,
    /// Start of the segment in audio timestamp units (same origin as the
    /// video timestamps of the session).
    start: i64,
    /// Samples waiting for the next fragment: decode time relative to the
    /// segment, duration, data.
    pending: Vec<(u64, u32, Bytes)>,
    /// Decode time of the last sample taken.
    last: Option<u64>,
}

impl ActiveAudio {
    /// Takes the audio frames received with a picture; drops those from
    /// before the segment or not after the previous one.
    fn take(&mut self, frame: &VideoFrame) {
        let Some(audio) = &frame.audio else { return };
        if !Arc::ptr_eq(&audio.params, &self.params) && *audio.params != *self.params {
            return;
        }
        for s in &audio.samples {
            let Ok(t) = u64::try_from(s.pts - self.start) else {
                continue;
            };
            if self.last.is_some_and(|last| t <= last) {
                continue;
            }
            self.pending.push((t, s.duration, s.data.clone()));
            self.last = Some(t);
        }
    }

    /// Run of the pending samples: base decode time and sample metadata.
    /// Durations are the gaps between samples (the last one keeps its own),
    /// so the track stays aligned on the camera's timestamps.
    fn run(&self) -> Option<(u64, Vec<SampleMeta>)> {
        let base = self.pending.first()?.0;
        let metas = self
            .pending
            .iter()
            .enumerate()
            .map(|(i, (t, duration, data))| SampleMeta {
                duration: self
                    .pending
                    .get(i + 1)
                    .and_then(|(next, _, _)| u32::try_from(next - t).ok())
                    .unwrap_or(*duration),
                size: u32::try_from(data.len()).unwrap_or(u32::MAX),
                is_key: true,
            })
            .collect();
        Some((base, metas))
    }
}

struct Active {
    file: Box<dyn SegmentFile>,
    params: Arc<VideoParams>,
    /// Audio track, if the segment has one.
    audio: Option<ActiveAudio>,
    session: u64,
    start_wall: SystemTime,
    start_pts: i64,
    /// Frame whose duration is not known yet.
    held: VideoFrame,
    /// Samples of the fragment being built, with known durations.
    fragment: Vec<Sample>,
    /// Decode time (relative to the segment) of the first sample of `fragment`.
    fragment_start: u64,
    /// Decode time of the next sample to be appended.
    next_decode_time: u64,
    last_duration: u32,
    sequence: u32,
    bytes: u64,
    frames: u64,
    fragments: u32,
}

enum State {
    WaitingForKey,
    Recording(Box<Active>),
    Failed { until: Instant },
}

/// Why a segment is closed before reaching its target duration.
#[derive(Debug, Clone, Copy)]
enum Discontinuity {
    NewSession,
    NewParameters,
    FramesDropped,
    TimestampBackwards,
    TimestampJump,
}

/// Segmentation state machine for one camera.
pub struct RecorderCore {
    camera: CameraId,
    settings: RecorderSettings,
    store: Box<dyn SegmentStore>,
    state: State,
    consecutive_errors: u32,
    events: Vec<NvrEvent>,
    metrics: Arc<RecorderMetrics>,
    /// Record the sound (shared switch, read for every picture).
    record_audio: Arc<AtomicBool>,
}

fn ticks(d: Duration, clock_rate: u32) -> i64 {
    i64::try_from(d.as_nanos() * u128::from(clock_rate) / 1_000_000_000).unwrap_or(i64::MAX)
}

impl RecorderCore {
    /// Creates a recorder writing to `store`.
    pub fn new(camera: CameraId, settings: RecorderSettings, store: Box<dyn SegmentStore>) -> Self {
        Self {
            camera,
            settings,
            store,
            state: State::WaitingForKey,
            consecutive_errors: 0,
            events: Vec::new(),
            metrics: Arc::new(RecorderMetrics::default()),
            record_audio: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Uses `switch` to decide whether to record the sound (on by default).
    /// Turned off, sound stops at once; turned on, it starts with a new
    /// segment at the next key frame.
    pub fn with_audio_switch(mut self, switch: Arc<AtomicBool>) -> Self {
        self.record_audio = switch;
        self
    }

    /// Shared counters.
    pub fn metrics(&self) -> Arc<RecorderMetrics> {
        self.metrics.clone()
    }

    /// Camera.
    pub fn camera(&self) -> &CameraId {
        &self.camera
    }

    /// Events produced since the last call.
    pub fn drain_events(&mut self) -> Vec<NvrEvent> {
        std::mem::take(&mut self.events)
    }

    /// Whether a segment is open.
    pub fn is_recording(&self) -> bool {
        matches!(self.state, State::Recording(_))
    }

    /// Processes one frame.
    pub fn handle_frame(&mut self, frame: VideoFrame) {
        if let State::Failed { until } = self.state {
            if Instant::now() < until {
                self.metrics.frames_skipped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            self.state = State::WaitingForKey;
        }

        let audio_on = self.record_audio.load(Ordering::Relaxed);
        let frame = match &mut self.state {
            State::Recording(active) => {
                match Self::check_continuity(active, &frame, &self.settings) {
                    None => {
                        let result =
                            Self::append(active, frame, &self.settings, &self.metrics, audio_on);
                        // `append` flushes the fragment before a key frame that is
                        // due to start the next segment: rotate on it.
                        let rotate = result.is_ok()
                            && active.fragment.is_empty()
                            && active.held.is_key
                            && Self::rotation_due(active, &self.settings, audio_on);
                        match result {
                            Err(e) => self.fail(e),
                            Ok(()) if rotate => self.rotate(),
                            Ok(()) => {}
                        }
                        return;
                    }
                    Some(reason) => {
                        tracing::info!(camera = %self.camera, ?reason, "discontinuity, closing segment");
                        self.metrics.discontinuities.fetch_add(1, Ordering::Relaxed);
                        self.close();
                        frame
                    }
                }
            }
            _ => frame,
        };

        // Waiting for a key frame.
        if !frame.is_key {
            self.metrics.frames_skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if let Err(e) = self.open(frame) {
            self.fail(e);
        }
    }

    /// Ends the segment of `session`, if it is the one being recorded.
    pub fn handle_session_end(&mut self, session: u64) {
        if matches!(&self.state, State::Recording(a) if a.session == session) {
            self.close();
        }
    }

    /// Closes any open segment.
    pub fn shutdown(&mut self) {
        if self.is_recording() {
            self.close();
        }
    }

    fn check_continuity(
        active: &Active,
        frame: &VideoFrame,
        settings: &RecorderSettings,
    ) -> Option<Discontinuity> {
        if frame.session != active.session {
            return Some(Discontinuity::NewSession);
        }
        if !Arc::ptr_eq(&frame.params, &active.params) && *frame.params != *active.params {
            return Some(Discontinuity::NewParameters);
        }
        if let (Some(a), Some(fa)) = (&active.audio, &frame.audio)
            && !Arc::ptr_eq(&a.params, &fa.params)
            && *a.params != *fa.params
        {
            return Some(Discontinuity::NewParameters);
        }
        if frame.seq != active.held.seq + 1 {
            return Some(Discontinuity::FramesDropped);
        }
        let delta = frame.pts - active.held.pts;
        if delta <= 0 {
            return Some(Discontinuity::TimestampBackwards);
        }
        if delta > ticks(settings.max_timestamp_jump, active.params.clock_rate) {
            return Some(Discontinuity::TimestampJump);
        }
        None
    }

    /// Whether the held frame is past the target segment duration.
    fn segment_due(active: &Active, settings: &RecorderSettings) -> bool {
        active.held.pts - active.start_pts
            >= ticks(settings.segment_duration, active.params.clock_rate)
    }

    /// Whether the next key frame should start a new segment: past the target
    /// duration, or sound was switched on while this segment has none.
    fn rotation_due(active: &Active, settings: &RecorderSettings, audio_on: bool) -> bool {
        Self::segment_due(active, settings)
            || (audio_on && active.audio.is_none() && active.held.audio.is_some())
    }

    /// Appends the held frame (now that its duration is known) and holds `frame`.
    fn append(
        active: &mut Active,
        frame: VideoFrame,
        settings: &RecorderSettings,
        metrics: &RecorderMetrics,
        audio_on: bool,
    ) -> io::Result<()> {
        let rate = active.params.clock_rate;
        let duration = u32::try_from(frame.pts - active.held.pts).unwrap_or(u32::MAX);
        let held = std::mem::replace(&mut active.held, frame);
        if audio_on && let Some(audio) = &mut active.audio {
            audio.take(&held);
        }
        if active.fragment.is_empty() {
            active.fragment_start = active.next_decode_time;
        }
        active.fragment.push(Sample {
            data: held.data,
            duration,
            is_key: held.is_key,
        });
        active.next_decode_time += u64::from(duration);
        active.last_duration = duration;

        let buffered =
            i64::try_from(active.next_decode_time - active.fragment_start).unwrap_or(i64::MAX);
        let target = ticks(settings.fragment_duration, rate);
        let rotation_due = Self::rotation_due(active, settings, audio_on);
        if active.held.is_key && (buffered >= target || rotation_due) || buffered >= 2 * target {
            Self::write_fragment(active, metrics)?;
        }
        Ok(())
    }

    fn write_fragment(active: &mut Active, metrics: &RecorderMetrics) -> io::Result<()> {
        if active.fragment.is_empty() {
            return Ok(());
        }
        let metas: Vec<SampleMeta> = active
            .fragment
            .iter()
            .map(|s| SampleMeta {
                duration: s.duration,
                size: u32::try_from(s.data.len()).unwrap_or(u32::MAX),
                is_key: s.is_key,
            })
            .collect();
        let audio_run = active.audio.as_ref().and_then(ActiveAudio::run);
        active.sequence += 1;
        let header = fmp4::fragment_header_av(
            active.sequence,
            active.fragment_start,
            &metas,
            audio_run.as_ref().map(|(base, m)| (*base, m.as_slice())),
        );
        active.file.write_all(&header)?;
        let mut bytes = header.len() as u64;
        for s in &active.fragment {
            active.file.write_all(&s.data)?;
            bytes += s.data.len() as u64;
        }
        if let Some(audio) = &mut active.audio {
            for (_, _, data) in audio.pending.drain(..) {
                active.file.write_all(&data)?;
                bytes += data.len() as u64;
            }
        }
        // Hand the whole fragment to the OS: a process crash now loses nothing written.
        active.file.flush()?;
        active.bytes += bytes;
        active.frames += active.fragment.len() as u64;
        active.fragments += 1;
        metrics.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        metrics
            .frames_written
            .fetch_add(active.fragment.len() as u64, Ordering::Relaxed);
        active.fragment.clear();
        Ok(())
    }

    fn open(&mut self, frame: VideoFrame) -> io::Result<()> {
        debug_assert!(frame.is_key);
        let audio = frame
            .audio
            .as_ref()
            .filter(|_| self.record_audio.load(Ordering::Relaxed))
            .map(|a| ActiveAudio {
                params: a.params.clone(),
                start: i64::try_from(
                    i128::from(frame.pts) * i128::from(a.params.clock_rate)
                        / i128::from(frame.params.clock_rate.max(1)),
                )
                .unwrap_or(0),
                pending: Vec::new(),
                last: None,
            });
        let mut file = self.store.create(&self.camera, frame.wall_time)?;
        let init = fmp4::init_segment_av(&frame.params, audio.as_ref().map(|a| &*a.params));
        if let Err(e) = file.write_all(&init) {
            file.abandon();
            return Err(e);
        }
        tracing::debug!(camera = %self.camera, path = %file.path().display(), "segment opened");
        self.state = State::Recording(Box::new(Active {
            file,
            params: frame.params.clone(),
            audio,
            session: frame.session,
            start_wall: frame.wall_time,
            start_pts: frame.pts,
            held: frame,
            fragment: Vec::new(),
            fragment_start: 0,
            next_decode_time: 0,
            last_duration: 0,
            sequence: 0,
            bytes: init.len() as u64,
            frames: 0,
            fragments: 0,
        }));
        Ok(())
    }

    /// Closes the current segment and opens a new one on the held key frame.
    fn rotate(&mut self) {
        let State::Recording(active) = std::mem::replace(&mut self.state, State::WaitingForKey)
        else {
            return;
        };
        let next = active.held.clone();
        match self.finish(active, false) {
            Ok(()) => {
                if let Err(e) = self.open(next) {
                    self.fail(e);
                }
            }
            Err(e) => self.fail(e),
        }
    }

    /// Writes the held frame and finishes the segment.
    fn close(&mut self) {
        let State::Recording(active) = std::mem::replace(&mut self.state, State::WaitingForKey)
        else {
            return;
        };
        if let Err(e) = self.finish(active, true) {
            self.fail(e);
        }
    }

    fn finish(&mut self, mut active: Box<Active>, include_held: bool) -> io::Result<()> {
        if include_held {
            // The last frame's duration is unknown: reuse the previous one,
            // or 1/25 s for a single-frame segment.
            let duration = if active.last_duration > 0 {
                active.last_duration
            } else {
                active.params.clock_rate / 25
            };
            if active.fragment.is_empty() {
                active.fragment_start = active.next_decode_time;
            }
            if self.record_audio.load(Ordering::Relaxed)
                && let Some(audio) = &mut active.audio
            {
                audio.take(&active.held);
            }
            active.fragment.push(Sample {
                data: active.held.data.clone(),
                duration,
                is_key: active.held.is_key,
            });
            active.next_decode_time += u64::from(duration);
        }
        let result = Self::write_fragment(&mut active, &self.metrics);
        if let Err(e) = result {
            self.state = State::Recording(active);
            return Err(e);
        }
        let Active {
            file,
            params,
            start_wall,
            next_decode_time,
            bytes,
            frames,
            fragments,
            ..
        } = *active;
        let path = file.finish()?;
        self.consecutive_errors = 0;
        self.metrics.segments.fetch_add(1, Ordering::Relaxed);
        let info = SegmentInfo {
            camera: self.camera.clone(),
            path,
            start: start_wall,
            duration: Duration::from_nanos(
                u64::try_from(
                    u128::from(next_decode_time) * 1_000_000_000 / u128::from(params.clock_rate),
                )
                .unwrap_or(u64::MAX),
            ),
            bytes,
            frames,
            fragments,
            codec: params.codec,
            width: params.width,
            height: params.height,
        };
        tracing::debug!(camera = %self.camera, path = %info.path.display(), duration_ms = info.duration.as_millis() as u64, "segment finished");
        self.events.push(NvrEvent::SegmentFinished(info));
        Ok(())
    }

    fn fail(&mut self, error: io::Error) {
        self.metrics.write_errors.fetch_add(1, Ordering::Relaxed);
        let salvaged = match std::mem::replace(&mut self.state, State::WaitingForKey) {
            State::Recording(active) => active.file.abandon(),
            _ => None,
        };
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        let factor = 1u32 << (self.consecutive_errors - 1).min(4);
        let delay = self.settings.retry_after_error * factor;
        tracing::error!(camera = %self.camera, error = %error, retry_in_ms = delay.as_millis() as u64, salvaged = ?salvaged, "recording write failed");
        self.events.push(NvrEvent::RecorderError {
            camera: self.camera.clone(),
            message: error.to_string(),
            salvaged,
        });
        self.state = State::Failed {
            until: Instant::now() + delay,
        };
    }
}

#[cfg(test)]
mod tests;
