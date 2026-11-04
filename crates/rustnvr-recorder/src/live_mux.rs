//! Fragmented MP4 for live consumers (browser viewers, FFmpeg pipes): one
//! fragment per frame, built from the frames of a single session.

use crate::fmp4::{self, SampleMeta};
use bytes::Bytes;
use rustnvr_core::{AudioParams, VideoFrame, VideoParams};
use std::sync::Arc;

/// Audio track of a live stream.
struct LiveAudio {
    params: Arc<AudioParams>,
    /// Start of the stream in audio timestamp units.
    start: i64,
    /// Decode time of the last sample sent.
    last: Option<u64>,
}

/// Longest gap between two frames kept as is by a tolerant muxer.
const TOLERATED_GAP_SECONDS: u32 = 2;

/// Turns frames of one session into fMP4 fragments. The latest frame is held
/// until the next one arrives, since its duration is the timestamp difference.
pub struct LiveMuxer {
    session: u64,
    params: Arc<VideoParams>,
    held: Option<VideoFrame>,
    sequence: u32,
    /// Decode time of the next fragment.
    decode_time: u64,
    /// Duration of the last fragment, used for the final one.
    last_duration: u32,
    /// Replace timestamp jumps instead of resetting.
    tolerant: bool,
    /// Sound, for consumers that asked for it.
    audio: Option<LiveAudio>,
}

/// Result of [`LiveMuxer::push`].
pub enum Push {
    /// Nothing to send yet.
    Nothing,
    /// A fragment to send.
    Fragment(Bytes),
    /// The stream changed: the client must start over.
    Reset(&'static str),
}

impl LiveMuxer {
    /// Starts a muxer for the session of `first` (pushed afterwards like any frame).
    pub fn new(first: &VideoFrame) -> Self {
        Self {
            session: first.session,
            params: first.params.clone(),
            held: None,
            sequence: 0,
            decode_time: 0,
            // Assume 15 frames/s until a real duration is known.
            last_duration: first.params.clock_rate / 15,
            tolerant: false,
            audio: None,
        }
    }

    /// Like [`Self::new`], with the sound of the session as a second track
    /// when the camera sends one (browser viewers).
    pub fn with_audio(first: &VideoFrame) -> Self {
        let audio = first.audio.as_ref().map(|a| LiveAudio {
            params: a.params.clone(),
            start: i64::try_from(
                i128::from(first.pts) * i128::from(a.params.clock_rate)
                    / i128::from(first.params.clock_rate.max(1)),
            )
            .unwrap_or(0),
            last: None,
        });
        Self {
            audio,
            ..Self::new(first)
        }
    }

    /// Initialization segment of the stream (video, and sound if any).
    pub fn init_segment(&self) -> Vec<u8> {
        fmp4::init_segment_av(&self.params, self.audio.as_ref().map(|a| &*a.params))
    }

    /// RFC 6381 codecs of the stream, e.g. `avc1.640033,mp4a.40.2`.
    pub fn codecs(&self) -> String {
        match &self.audio {
            Some(a) => format!("{},{}", self.params.rfc6381_codec, a.params.rfc6381_codec),
            None => self.params.rfc6381_codec.clone(),
        }
    }

    /// Audio codec, when the stream has sound.
    pub fn audio_codec(&self) -> Option<&str> {
        self.audio.as_ref().map(|a| a.params.rfc6381_codec.as_str())
    }

    /// Like [`Self::new`], but timestamp discontinuities (backwards, or
    /// forward jumps over 2 s) are replaced by the previous frame duration
    /// instead of resetting: for consumers that only need a plausible,
    /// increasing timeline (FFmpeg), not the exact timing.
    pub fn tolerant(first: &VideoFrame) -> Self {
        Self {
            tolerant: true,
            ..Self::new(first)
        }
    }

    /// Adds a frame; returns the fragment of the previous one, if any.
    pub fn push(&mut self, frame: VideoFrame) -> Push {
        if frame.session != self.session {
            return Push::Reset("new session");
        }
        if !Arc::ptr_eq(&frame.params, &self.params) && *frame.params != *self.params {
            return Push::Reset("new parameters");
        }
        if let (Some(a), Some(fa)) = (&self.audio, &frame.audio)
            && !Arc::ptr_eq(&a.params, &fa.params)
            && *a.params != *fa.params
        {
            return Push::Reset("new audio parameters");
        }
        let Some(held) = self.held.replace(frame) else {
            return Push::Nothing;
        };
        let next_pts = self.held.as_ref().map_or(held.pts, |f| f.pts);
        let duration = match u32::try_from(next_pts - held.pts) {
            Ok(d)
                if d > 0
                    && (!self.tolerant || d <= self.params.clock_rate * TOLERATED_GAP_SECONDS) =>
            {
                d
            }
            _ if self.tolerant => self.last_duration,
            _ => return Push::Reset("timestamp discontinuity"),
        };
        self.last_duration = duration;
        Push::Fragment(self.fragment(&held, duration))
    }

    /// Fragment of the held frame, when no frame follows (end of input). Its
    /// duration is assumed equal to the previous one.
    pub fn finish(&mut self) -> Option<Bytes> {
        let held = self.held.take()?;
        Some(self.fragment(&held, self.last_duration))
    }

    fn fragment(&mut self, frame: &VideoFrame, duration: u32) -> Bytes {
        let decode_time = self.decode_time;
        self.decode_time += u64::from(duration);
        self.sequence = self.sequence.wrapping_add(1);
        let meta = SampleMeta {
            duration,
            size: u32::try_from(frame.data.len()).unwrap_or(u32::MAX),
            is_key: frame.is_key,
        };
        // Sound received just before this picture, placed by its own
        // timestamps (dropped if before the start or not after the last one).
        let mut audio: Vec<(u64, u32, &Bytes)> = Vec::new();
        if let (Some(a), Some(fa)) = (&mut self.audio, &frame.audio) {
            for s in &fa.samples {
                let Ok(t) = u64::try_from(s.pts - a.start) else {
                    continue;
                };
                if a.last.is_some_and(|last| t <= last) {
                    continue;
                }
                a.last = Some(t);
                audio.push((t, s.duration, &s.data));
            }
        }
        let audio_metas: Vec<SampleMeta> = audio
            .iter()
            .enumerate()
            .map(|(i, (t, d, data))| SampleMeta {
                duration: audio
                    .get(i + 1)
                    .and_then(|(next, _, _)| u32::try_from(next - t).ok())
                    .unwrap_or(*d),
                size: u32::try_from(data.len()).unwrap_or(u32::MAX),
                is_key: true,
            })
            .collect();
        let mut out = fmp4::fragment_header_av(
            self.sequence,
            decode_time,
            &[meta],
            audio.first().map(|(t, _, _)| (*t, audio_metas.as_slice())),
        );
        out.extend_from_slice(&frame.data);
        for (_, _, data) in &audio {
            out.extend_from_slice(data);
        }
        Bytes::from(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn frame(
        session: u64,
        seq: u64,
        pts: i64,
        is_key: bool,
        params: &Arc<VideoParams>,
    ) -> VideoFrame {
        VideoFrame {
            session,
            seq,
            params: params.clone(),
            pts,
            is_key,
            wall_time: SystemTime::now(),
            loss: 0,
            audio: None,
            data: Bytes::from(vec![seq as u8; 10]),
        }
    }

    fn params() -> Arc<VideoParams> {
        Arc::new(VideoParams {
            codec: rustnvr_core::VideoCodec::H264,
            rfc6381_codec: "avc1.42C01E".into(),
            width: 320,
            height: 240,
            clock_rate: 90_000,
            sample_entry: Bytes::from_static(&[0, 0, 0, 8, b'a', b'v', b'c', b'1']),
        })
    }

    #[test]
    fn fragments_carry_durations_and_decode_times() {
        let p = params();
        let first = frame(1, 0, 1000, true, &p);
        let mut m = LiveMuxer::new(&first);
        assert!(matches!(m.push(first), Push::Nothing));
        let Push::Fragment(f) = m.push(frame(1, 1, 4600, false, &p)) else {
            panic!("expected a fragment");
        };
        // Parse it back with the recorder's scanner (after a synthetic init segment).
        let mut file = fmp4::init_segment(&p);
        file.extend_from_slice(&f);
        let report = crate::scan::scan(&mut std::io::Cursor::new(file)).expect("scan");
        let frag = &report.fragments[0];
        assert_eq!(
            (frag.base_decode_time, frag.duration, frag.samples),
            (0, 3600, 1)
        );
        assert!(frag.starts_with_key);
    }

    #[test]
    fn carries_sound_when_asked() {
        let p = params();
        let a = Arc::new(AudioParams {
            rfc6381_codec: "mp4a.40.2".into(),
            clock_rate: 16_000,
            channels: 1,
            sample_entry: Bytes::from_static(&[0, 0, 0, 8, b'm', b'p', b'4', b'a']),
        });
        // Picture at 0.1 s carrying AAC frames from before it (dropped) and
        // after the start; then a picture at 0.14 s.
        let with_audio = |seq, pts, audio_pts: &[i64]| VideoFrame {
            audio: Some(Arc::new(rustnvr_core::FrameAudio {
                params: a.clone(),
                samples: audio_pts
                    .iter()
                    .map(|&pts| rustnvr_core::AudioSample {
                        pts,
                        duration: 1024,
                        data: Bytes::from_static(&[0x21; 7]),
                    })
                    .collect(),
            })),
            ..frame(1, seq, pts, seq == 0, &p)
        };
        let first = with_audio(0, 9000, &[0, 1600]);
        assert!(LiveMuxer::new(&first).audio_codec().is_none(), "opt-in");
        let mut m = LiveMuxer::with_audio(&first);
        assert_eq!(m.codecs(), "avc1.42C01E,mp4a.40.2");
        m.push(first);
        let Push::Fragment(f) = m.push(with_audio(1, 12_600, &[2624])) else {
            panic!("expected a fragment");
        };
        let mut file = m.init_segment();
        file.extend_from_slice(&f);
        let report = crate::scan::scan(&mut std::io::Cursor::new(file)).expect("scan");
        assert!(report.has_audio);
        assert_eq!(report.valid_len, report.file_len);
        // The sample at 0 s is before the stream start (0.1 s = 1600): only
        // the one at 1600 goes with the first picture.
        assert_eq!(report.fragments[0].audio_samples, 1);
        assert_eq!(report.fragments[0].samples, 1);
    }

    #[test]
    fn resets_on_stream_changes() {
        let p = params();
        let first = frame(1, 0, 0, true, &p);
        let mut m = LiveMuxer::new(&first);
        m.push(first);
        assert!(matches!(m.push(frame(2, 0, 0, true, &p)), Push::Reset(_)));
        let mut other = (*p).clone();
        other.width = 640;
        assert!(matches!(
            m.push(frame(1, 1, 3600, false, &Arc::new(other))),
            Push::Reset(_)
        ));
        assert!(matches!(m.push(frame(1, 1, -5, false, &p)), Push::Reset(_)));
    }

    #[test]
    fn tolerant_muxer_repairs_timestamp_jumps() {
        let p = params();
        let first = frame(1, 0, 1000, true, &p);
        let mut m = LiveMuxer::tolerant(&first);
        m.push(first);
        // Normal, backwards, then far forward: durations 3600, 3600, 3600.
        let mut file = fmp4::init_segment(&p);
        for (seq, pts) in [(1, 4600), (2, 100), (3, 900_000), (4, 903_600)] {
            match m.push(frame(1, seq, pts, false, &p)) {
                Push::Fragment(f) => file.extend_from_slice(&f),
                _ => panic!("expected a fragment"),
            }
        }
        file.extend_from_slice(&m.finish().expect("last"));
        let report = crate::scan::scan(&mut std::io::Cursor::new(file)).expect("scan");
        let times: Vec<_> = report
            .fragments
            .iter()
            .map(|f| (f.base_decode_time, f.duration))
            .collect();
        assert_eq!(
            times,
            vec![
                (0, 3600),
                (3600, 3600),
                (7200, 3600),
                (10_800, 3600),
                (14_400, 3600)
            ]
        );
        // Changes of session still reset.
        assert!(matches!(m.push(frame(2, 0, 0, true, &p)), Push::Reset(_)));
    }
}
