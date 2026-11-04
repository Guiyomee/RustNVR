//! Compressed media exchanged between ingestion and its consumers.
//!
//! Frames carry the encoded bitstream exactly as received (no decoding, no
//! re-encoding). Cloning a [`VideoFrame`] is cheap: the payload is a
//! reference-counted [`Bytes`] and parameters are shared through an [`Arc`].

use bytes::Bytes;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

/// Video codecs supported by the recording path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    H265,
}

impl fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "h265",
        })
    }
}

/// Decoder configuration of a video stream.
///
/// A change of any field (new SPS/PPS/VPS, resolution, codec) means a new
/// initialization segment is required, so consumers compare parameters with
/// `==` to detect it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoParams {
    /// Codec.
    pub codec: VideoCodec,
    /// RFC 6381 codec string, e.g. `avc1.42C01E`; used for browser playback (MSE/HLS).
    pub rfc6381_codec: String,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Timestamp clock rate in Hz (90 000 for RTP video).
    pub clock_rate: u32,
    /// Complete ISO BMFF `VisualSampleEntry` box (`avc1` or `hvc1`, including
    /// `avcC`/`hvcC`), ready to be placed in an `stsd` box.
    pub sample_entry: Bytes,
}

/// Decoder configuration of an audio stream (AAC only for now).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioParams {
    /// RFC 6381 codec string, e.g. `mp4a.40.2`.
    pub rfc6381_codec: String,
    /// Timestamp clock rate in Hz (the sample rate for AAC).
    pub clock_rate: u32,
    /// Number of channels.
    pub channels: u16,
    /// Complete ISO BMFF `AudioSampleEntry` box (`mp4a`, including `esds`).
    pub sample_entry: Bytes,
}

/// One encoded audio frame (an AAC access unit).
#[derive(Clone, Debug)]
pub struct AudioSample {
    /// Presentation timestamp in the audio `clock_rate` units, relative to the
    /// start of the session (same origin as the video timestamps when the
    /// camera announces it, see `rustnvr_stream`).
    pub pts: i64,
    /// Duration in `clock_rate` units (1024 for AAC-LC).
    pub duration: u32,
    /// Encoded data, without ADTS header.
    pub data: Bytes,
}

/// Audio received with a video frame: the frames that arrived since the
/// previous video frame, and the parameters of the session's audio stream.
#[derive(Clone, Debug)]
pub struct FrameAudio {
    /// Parameters of the audio stream (shared by every frame of the session).
    pub params: Arc<AudioParams>,
    /// Audio frames received since the previous video frame (often empty).
    pub samples: Vec<AudioSample>,
}

/// One encoded picture (access unit).
#[derive(Clone, Debug)]
pub struct VideoFrame {
    /// Identifier of the RTSP session the frame belongs to. Changes on every
    /// reconnection; timestamps are only comparable within one session.
    pub session: u64,
    /// Position of the frame within its session, starting at 0 and
    /// incremented by one per frame. A consumer that sees a gap knows frames
    /// were dropped before reaching it.
    pub seq: u64,
    /// Decoder configuration in effect for this frame.
    pub params: Arc<VideoParams>,
    /// Presentation timestamp in `params.clock_rate` units, relative to the
    /// start of the session.
    pub pts: i64,
    /// True for random access points (IDR / IRAP pictures).
    pub is_key: bool,
    /// Wall-clock time at which the frame was received.
    pub wall_time: SystemTime,
    /// Number of RTP packets lost immediately before this frame.
    pub loss: u16,
    /// NAL units, each prefixed by its length as a 4-byte big-endian integer
    /// (ISO BMFF sample format). Parameter sets are carried in `params`.
    pub data: Bytes,
    /// Audio of the session, when the camera sends a supported audio stream
    /// (AAC): the audio frames received just before this picture. Carrying
    /// audio with video keeps it in the same queues, buffers and drop
    /// detection.
    pub audio: Option<Arc<FrameAudio>>,
}

/// Item delivered to stream consumers.
#[derive(Clone, Debug)]
pub enum StreamItem {
    /// A video frame.
    Frame(VideoFrame),
    /// The given session ended (disconnection, stall, shutdown). Delivered on
    /// a best-effort basis: consumers must also treat a change of
    /// [`VideoFrame::session`] as the end of the previous session.
    SessionEnd {
        /// Identifier of the session that ended.
        session: u64,
    },
}
