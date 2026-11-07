//! Connection test: what does a camera offer, and does video actually flow?

use crate::error::StreamError;
use crate::session::{self, RtspSession, SessionOptions};
use retina::client::SessionGroup;
use rustnvr_core::{RtspTarget, VideoCodec};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// One stream announced in the SDP.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbedStream {
    /// `video`, `audio` or `application`.
    pub media: String,
    /// Lower-case encoding name (`h264`, `h265`, `mpeg4-generic`, …).
    pub encoding: String,
    /// RTP clock rate.
    pub clock_rate: u32,
    /// Frame rate announced in the SDP, if any.
    pub framerate: Option<f32>,
}

/// First key frame observed while probing.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbedVideo {
    /// Codec.
    pub codec: VideoCodec,
    /// RFC 6381 codec string.
    pub rfc6381_codec: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Time from PLAY to the first key frame.
    pub time_to_key_frame: Duration,
}

/// Result of [`probe`].
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeReport {
    /// Streams announced by the server.
    pub streams: Vec<ProbedStream>,
    /// Server tool (SDP `a=tool`), if announced.
    pub tool: Option<String>,
    /// Index of the stream RustNVR would record, if any.
    pub selected: Option<usize>,
    /// Present when frames were requested and a key frame arrived.
    pub video: Option<ProbedVideo>,
}

/// Plays the stream RustNVR would record from `target` (connection test
/// preview); frames come from [`RtspSession::next_frame`] until it is dropped.
pub async fn play(target: &RtspTarget, opts: &SessionOptions) -> Result<RtspSession, StreamError> {
    let group = Arc::new(SessionGroup::default());
    let described = session::describe(target, opts, group).await?;
    let index = session::select_video_stream(&described)?;
    RtspSession::start(described, index, None, opts, 0).await
}

/// Connects to `target`, lists its streams and, if `wait_for_key_frame`,
/// plays the selected video stream until the first key frame (bounded by the
/// stall timeout).
pub async fn probe(
    target: &RtspTarget,
    opts: &SessionOptions,
    wait_for_key_frame: bool,
) -> Result<ProbeReport, StreamError> {
    let group = Arc::new(SessionGroup::default());
    let described = session::describe(target, opts, group).await?;
    let streams = described
        .streams()
        .iter()
        .map(|s| ProbedStream {
            media: s.media().to_owned(),
            encoding: s.encoding_name().to_owned(),
            clock_rate: s.clock_rate_hz(),
            framerate: s.framerate(),
        })
        .collect();
    let tool = described.tool().map(|t| format!("{t:?}"));
    let selected = session::select_video_stream(&described).ok();
    let mut report = ProbeReport {
        streams,
        tool,
        selected,
        video: None,
    };
    let Some(index) = selected.filter(|_| wait_for_key_frame) else {
        return Ok(report);
    };
    let mut playing = RtspSession::start(described, index, None, opts, 0).await?;
    let started = Instant::now();
    loop {
        let frame = playing.next_frame().await?;
        if frame.is_key {
            let p = &frame.params;
            report.video = Some(ProbedVideo {
                codec: p.codec,
                rfc6381_codec: p.rfc6381_codec.clone(),
                width: p.width,
                height: p.height,
                time_to_key_frame: started.elapsed(),
            });
            return Ok(report);
        }
        if started.elapsed() > opts.stall_timeout {
            return Err(StreamError::Stalled(opts.stall_timeout));
        }
    }
}
