//! One RTSP session, from DESCRIBE to the last frame.

use crate::error::StreamError;
use bytes::Bytes;
use futures::StreamExt as _;
use retina::client::{
    Demuxed, Described, InitialSequenceNumberPolicy, InitialTimestampPolicy, PlayOptions, Session,
    SessionGroup, SetupOptions, Transport,
};
use retina::codec::{CodecItem, FrameFormat, ParametersRef, VideoParametersCodec};
use rustnvr_core::config::{RtspTransport, StreamConfig};
use rustnvr_core::{
    AudioParams, AudioSample, FrameAudio, RtspTarget, VideoCodec, VideoFrame, VideoParams,
};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::Instant;

/// Connection options of a session.
#[derive(Clone, Debug)]
pub struct SessionOptions {
    /// RTP transport.
    pub transport: RtspTransport,
    /// Timeout of each of DESCRIBE, SETUP and PLAY.
    pub connect_timeout: Duration,
    /// Maximum time without a video frame once playing.
    pub stall_timeout: Duration,
    /// `User-Agent` header.
    pub user_agent: String,
    /// Also receive the camera's audio stream, if it is AAC.
    pub audio: bool,
}

impl SessionOptions {
    /// Options from a stream configuration.
    pub fn from_config(c: &StreamConfig) -> Self {
        Self {
            transport: c.transport,
            connect_timeout: c.connect_timeout(),
            stall_timeout: c.stall_timeout(),
            ..Self::default()
        }
    }
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            transport: RtspTransport::Tcp,
            connect_timeout: Duration::from_secs(10),
            stall_timeout: Duration::from_secs(10),
            user_agent: concat!("RustNVR/", env!("CARGO_PKG_VERSION")).to_owned(),
            audio: false,
        }
    }
}

async fn step<T>(
    stage: &'static str,
    after: Duration,
    fut: impl Future<Output = Result<T, retina::Error>>,
) -> Result<T, StreamError> {
    tokio::time::timeout(after, fut)
        .await
        .map_err(|_| StreamError::Timeout { stage, after })?
        .map_err(|error| StreamError::Rtsp { stage, error })
}

/// Sends DESCRIBE. The returned session lists the server's streams.
pub(crate) async fn describe(
    target: &RtspTarget,
    opts: &SessionOptions,
    group: Arc<SessionGroup>,
) -> Result<Session<Described>, StreamError> {
    let creds = target.credentials().map(|c| retina::client::Credentials {
        username: c.username.clone(),
        password: c.password.expose().to_owned(),
    });
    let session_opts = retina::client::SessionOptions::default()
        .creds(creds)
        .user_agent(opts.user_agent.clone())
        .session_group(group);
    step(
        "DESCRIBE",
        opts.connect_timeout,
        Session::describe(target.url().clone(), session_opts),
    )
    .await
}

/// Index of the first H.264/H.265 video stream.
pub(crate) fn select_video_stream(session: &Session<Described>) -> Result<usize, StreamError> {
    let streams = session.streams();
    streams
        .iter()
        .position(|s| s.media() == "video" && matches!(s.encoding_name(), "h264" | "h265"))
        .ok_or_else(|| StreamError::NoSupportedVideo {
            available: streams
                .iter()
                .map(|s| format!("{}/{}", s.media(), s.encoding_name()))
                .collect::<Vec<_>>()
                .join(", "),
        })
}

/// Index of the first AAC audio stream (the only audio codec recorded).
pub(crate) fn select_audio_stream(session: &Session<Described>) -> Option<usize> {
    session
        .streams()
        .iter()
        .position(|s| s.media() == "audio" && s.encoding_name() == "mpeg4-generic")
}

/// Most audio frames kept while waiting for the next video frame (≈ 16 s of
/// AAC at 16 kHz); older ones are dropped.
const MAX_PENDING_AUDIO: usize = 256;

/// A playing RTSP session yielding video frames.
pub struct RtspSession {
    demuxed: Demuxed,
    stream_index: usize,
    session_id: u64,
    next_seq: u64,
    params: Option<Arc<VideoParams>>,
    stall_timeout: Duration,
    last_frame_at: Instant,
    /// Audio stream, when set up and its parameters are usable.
    audio_index: Option<usize>,
    audio_params: Option<Arc<AudioParams>>,
    /// Audio frames received since the last video frame.
    pending_audio: Vec<AudioSample>,
    /// First video frame of the session: timestamp and arrival.
    first_video: Option<(i64, Instant)>,
    /// Subtracted from audio timestamps to put them on the video timeline.
    audio_offset: Option<i64>,
}

impl RtspSession {
    /// Connects and starts playing the first H.264/H.265 video stream.
    pub async fn connect(
        target: &RtspTarget,
        opts: &SessionOptions,
        session_id: u64,
    ) -> Result<Self, StreamError> {
        Self::connect_in_group(target, opts, Arc::new(SessionGroup::default()), session_id).await
    }

    /// Like [`Self::connect`], tracking stale sessions in `group` across reconnections.
    pub(crate) async fn connect_in_group(
        target: &RtspTarget,
        opts: &SessionOptions,
        group: Arc<SessionGroup>,
        session_id: u64,
    ) -> Result<Self, StreamError> {
        let session = describe(target, opts, group).await?;
        let index = select_video_stream(&session)?;
        let audio = if opts.audio {
            select_audio_stream(&session)
        } else {
            None
        };
        Self::start(session, index, audio, opts, session_id).await
    }

    /// Sends SETUP for `stream_index` (and `audio_index`, if any) then PLAY.
    pub(crate) async fn start(
        mut session: Session<Described>,
        stream_index: usize,
        mut audio_index: Option<usize>,
        opts: &SessionOptions,
        session_id: u64,
    ) -> Result<Self, StreamError> {
        let setup = || {
            let transport = match opts.transport {
                RtspTransport::Tcp => Transport::Tcp(Default::default()),
                RtspTransport::Udp => Transport::Udp(Default::default()),
            };
            SetupOptions::default()
                .transport(transport)
                .frame_format(FrameFormat::MP4)
        };
        step(
            "SETUP",
            opts.connect_timeout,
            session.setup(stream_index, setup()),
        )
        .await?;
        if let Some(a) = audio_index
            && let Err(e) = step("SETUP", opts.connect_timeout, session.setup(a, setup())).await
        {
            // The video alone is still worth recording.
            tracing::warn!(error = %e, "audio stream refused by the camera, recording without sound");
            audio_index = None;
        }
        let playing = step(
            "PLAY",
            opts.connect_timeout,
            // Some cameras announce an RTP-Info `seq` unrelated to the first
            // packet they send (seen on a Hikvision-style H.265 camera:
            // announced 34312, first packet 30930), which Retina treats as a
            // fatal reordering over TCP, or counts as thousands of lost
            // packets. With a single stream per session the announced value
            // is useless: start from the first packet actually received.
            //
            // With audio, both streams are timed from RTP-Info `rtptime` when
            // the camera gives it for both (synchronized sound), otherwise
            // each from its first packet (close: both start right after PLAY).
            session.play(
                PlayOptions::default()
                    .initial_seq(InitialSequenceNumberPolicy::Ignore)
                    .initial_timestamp(InitialTimestampPolicy::Permissive),
            ),
        )
        .await?;
        let demuxed = playing.demuxed().map_err(|error| StreamError::Rtsp {
            stage: "PLAY",
            error,
        })?;
        let mut s = Self {
            demuxed,
            stream_index,
            session_id,
            next_seq: 0,
            params: None,
            stall_timeout: opts.stall_timeout,
            last_frame_at: Instant::now(),
            audio_index,
            audio_params: None,
            pending_audio: Vec::new(),
            first_video: None,
            audio_offset: None,
        };
        if let Some(a) = audio_index {
            match audio_params(&s.demuxed, a) {
                Ok(p) => {
                    tracing::debug!(codec = %p.rfc6381_codec, rate = p.clock_rate, channels = p.channels, "audio parameters");
                    s.audio_params = Some(Arc::new(p));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "unusable audio stream, recording without sound");
                    s.audio_index = None;
                }
            }
        }
        // Parameters are often known from the SDP already; if not, they will
        // be after the first frame.
        s.params = s.current_params().ok();
        Ok(s)
    }

    /// Session identifier given at connection.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Parameters of the stream, if known yet.
    pub fn params(&self) -> Option<&Arc<VideoParams>> {
        self.params.as_ref()
    }

    fn current_params(&self) -> Result<Arc<VideoParams>, StreamError> {
        let stream = &self.demuxed.streams()[self.stream_index];
        let Some(ParametersRef::Video(vp)) = stream.parameters() else {
            return Err(StreamError::MissingParameters);
        };
        let codec = match vp.codec_params() {
            VideoParametersCodec::H264 { .. } => VideoCodec::H264,
            VideoParametersCodec::H265 { .. } => VideoCodec::H265,
            _ => return Err(StreamError::UnsupportedCodec),
        };
        let mut entry = vp.mp4_sample_entry();
        if let Some((h, v)) = vp.pixel_aspect_ratio()
            && (h, v) != (1, 1)
            && let (Ok(h), Ok(v)) = (u16::try_from(h), u16::try_from(v))
        {
            entry = entry.with_aspect_ratio((h, v));
        }
        let sample_entry = entry.build().map_err(StreamError::SampleEntry)?;
        let (width, height) = vp.pixel_dimensions();
        Ok(Arc::new(VideoParams {
            codec,
            rfc6381_codec: vp.rfc6381_codec().to_owned(),
            width,
            height,
            clock_rate: stream.clock_rate_hz(),
            sample_entry: Bytes::from(sample_entry),
        }))
    }

    /// Timestamp of an audio frame on the video timeline (in audio clock
    /// units), or `None` before the first video frame.
    ///
    /// Cameras do not reliably relate their audio and video RTP clocks (one
    /// announced an audio `rtptime` five hours away from the video one), so
    /// the first audio frame is anchored to the video timestamp of its
    /// arrival; later frames keep the camera's audio clock.
    fn audio_pts(&mut self, raw: i64) -> Option<i64> {
        if let Some(offset) = self.audio_offset {
            return Some(raw - offset);
        }
        let (video_pts, video_at) = self.first_video?;
        let video_rate = i128::from(self.params.as_ref()?.clock_rate.max(1));
        let audio_rate = i128::from(self.audio_params.as_ref()?.clock_rate);
        let elapsed = i128::try_from(video_at.elapsed().as_nanos()).ok()?;
        let video_now = i128::from(video_pts) + elapsed * video_rate / 1_000_000_000;
        let audio_now = i64::try_from(video_now * audio_rate / video_rate).ok()?;
        let offset = raw - audio_now;
        self.audio_offset = Some(offset);
        Some(raw - offset)
    }

    /// Waits for the next video frame.
    ///
    /// Fails with [`StreamError::Stalled`] if no video frame arrives within
    /// the stall timeout, even if other packets (RTCP, audio) keep arriving.
    pub async fn next_frame(&mut self) -> Result<VideoFrame, StreamError> {
        loop {
            let deadline = self.last_frame_at + self.stall_timeout;
            let item = tokio::time::timeout_at(deadline, self.demuxed.next())
                .await
                .map_err(|_| StreamError::Stalled(self.stall_timeout))?;
            let frame = match item {
                None => return Err(StreamError::Ended),
                Some(Err(error)) => {
                    return Err(StreamError::Rtsp {
                        stage: "stream",
                        error,
                    });
                }
                Some(Ok(CodecItem::VideoFrame(f))) if f.stream_id() == self.stream_index => f,
                Some(Ok(CodecItem::AudioFrame(a))) if Some(a.stream_id()) == self.audio_index => {
                    let Some(pts) = self.audio_pts(a.timestamp().elapsed()) else {
                        continue;
                    };
                    if self.pending_audio.len() >= MAX_PENDING_AUDIO {
                        self.pending_audio.remove(0);
                    }
                    self.pending_audio.push(AudioSample {
                        pts,
                        duration: a.frame_length().get(),
                        data: Bytes::copy_from_slice(a.data()),
                    });
                    continue;
                }
                Some(Ok(_)) => continue,
            };
            self.last_frame_at = Instant::now();
            if frame.has_new_parameters() || self.params.is_none() {
                let params = self.current_params()?;
                if self.params.as_deref() != Some(&*params) {
                    tracing::debug!(codec = %params.codec, width = params.width, height = params.height, "video parameters");
                    self.params = Some(params);
                }
            }
            let Some(params) = self.params.clone() else {
                return Err(StreamError::MissingParameters);
            };
            if self.first_video.is_none() {
                self.first_video = Some((frame.timestamp().elapsed(), Instant::now()));
            }
            let seq = self.next_seq;
            self.next_seq += 1;
            // Retina only flags IDR pictures as random access points for
            // H.265. CRA/BLA pictures are also valid decoding start points
            // (leading RASL pictures are then discarded by the decoder) and
            // valid MP4 sync samples (ISO/IEC 14496-15); encoders such as
            // x265 use CRA for every key frame after the first.
            let is_key = frame.is_random_access_point()
                || (params.codec == VideoCodec::H265 && h265_has_irap(frame.data()));
            return Ok(VideoFrame {
                session: self.session_id,
                seq,
                params,
                pts: frame.timestamp().elapsed(),
                is_key,
                wall_time: SystemTime::now(),
                loss: frame.loss(),
                data: Bytes::from(frame.into_data()),
                audio: self.audio_params.as_ref().map(|p| {
                    Arc::new(FrameAudio {
                        params: p.clone(),
                        samples: std::mem::take(&mut self.pending_audio),
                    })
                }),
            });
        }
    }
}

/// Parameters of the AAC stream `index`, from the SDP.
fn audio_params(demuxed: &Demuxed, index: usize) -> Result<AudioParams, String> {
    let stream = &demuxed.streams()[index];
    let Some(ParametersRef::Audio(ap)) = stream.parameters() else {
        return Err("no audio parameters".to_owned());
    };
    let rfc6381_codec = ap
        .rfc6381_codec()
        .ok_or("no RFC 6381 codec string")?
        .to_owned();
    let sample_entry = ap.mp4_sample_entry().build().map_err(|e| e.to_string())?;
    Ok(AudioParams {
        rfc6381_codec,
        clock_rate: ap.clock_rate(),
        channels: ap.channels().get(),
        sample_entry: Bytes::from(sample_entry),
    })
}

/// True if a length-prefixed H.265 access unit contains an IRAP picture
/// (BLA, IDR or CRA: NAL unit types 16 to 21).
fn h265_has_irap(mut data: &[u8]) -> bool {
    while data.len() > 4 {
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let Some(nal) = data.get(4..4 + len) else {
            return false;
        };
        if let Some(&header) = nal.first()
            && (16..=21).contains(&((header >> 1) & 0x3f))
        {
            return true;
        }
        data = &data[4 + len..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::h265_has_irap;

    fn au(nal_types: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &t in nal_types {
            out.extend_from_slice(&3u32.to_be_bytes());
            out.extend_from_slice(&[t << 1, 1, 0xaa]);
        }
        out
    }

    #[test]
    fn detects_h265_irap_pictures() {
        assert!(h265_has_irap(&au(&[39, 21]))); // SEI + CRA
        assert!(h265_has_irap(&au(&[19]))); // IDR_W_RADL
        assert!(h265_has_irap(&au(&[16]))); // BLA_W_LP
        assert!(!h265_has_irap(&au(&[1]))); // TRAIL_R
        assert!(!h265_has_irap(&au(&[9, 8]))); // RASL
        // Truncated length prefix.
        assert!(!h265_has_irap(&[0, 0, 0, 9, 21 << 1]));
    }
}
