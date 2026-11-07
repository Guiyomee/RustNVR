//! A camera read through FFmpeg instead of RTSP: a USB webcam (V4L2), its
//! pictures encoded to H.264 by FFmpeg (or copied when the webcam sends
//! H.264), written as an Annex B stream with access unit delimiters on
//! FFmpeg's standard output. The stream is split into
//! [`rustnvr_core::VideoFrame`]s like the RTSP ones, so recording, live view
//! and detection do not know the difference.
//!
//! Timestamps are the arrival times of the pictures (an Annex B stream has
//! none): a few milliseconds of jitter, never going backwards.

use crate::backoff::{Backoff, BackoffConfig};
use crate::fanout::FrameFanout;
use crate::metrics::StreamMetrics;
use crate::supervisor::StreamHandle;
use bytes::Bytes;
use rustnvr_core::{
    CameraId, EventBus, NvrEvent, StreamRole, StreamState, VideoCodec, VideoFrame, VideoParams,
};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument as _, info, info_span, warn};

/// 90 kHz, as RTP video.
const CLOCK_RATE: u32 = 90_000;
/// FFmpeg error lines kept to explain a failure.
const STDERR_LINES: usize = 8;

/// The FFmpeg command of one attempt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceCommand {
    /// Arguments: input, then an H.264 Annex B output with access unit
    /// delimiters on `pipe:1` (see [`annexb_output_args`]).
    pub args: Vec<String>,
    /// Extra environment (`LIBVA_DRIVER_NAME`).
    pub env: Vec<(String, String)>,
}

/// Builds the command of each attempt (it may change: the graphics card
/// found meanwhile, for instance).
pub type CommandSource =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = DeviceCommand> + Send>> + Send + Sync>;

/// What a device supervisor runs.
#[derive(Clone)]
pub struct DeviceConfig {
    /// Camera.
    pub camera: CameraId,
    /// Shown in the log and the state (`/dev/video0`).
    pub label: String,
    /// FFmpeg executable.
    pub ffmpeg: String,
    /// The command of each attempt.
    pub command: CommandSource,
    /// The device is considered lost after this long without a picture.
    pub stall_timeout: Duration,
    /// Restart policy.
    pub backoff: BackoffConfig,
}

/// Output arguments shared by every device command: the H.264 parameter
/// sets before each key frame, an access unit delimiter before each
/// picture, raw Annex B on standard output, written picture by picture.
pub fn annexb_output_args() -> Vec<String> {
    [
        "-bsf:v",
        "dump_extra=freq=keyframe,h264_metadata=aud=insert",
        "-flush_packets",
        "1",
        "-f",
        "h264",
        "pipe:1",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// Spawns a supervisor that keeps FFmpeg reading the device until `cancel`
/// fires, restarting it with backoff. Same handle as RTSP streams.
pub fn spawn_device_supervisor(
    config: DeviceConfig,
    fanout: FrameFanout,
    events: EventBus,
    cancel: CancellationToken,
) -> StreamHandle {
    let (state_tx, state) = watch::channel(StreamState::Idle);
    let metrics = Arc::new(StreamMetrics::default());
    let span = info_span!("stream", camera = %config.camera, role = %StreamRole::Main);
    let supervisor = Supervisor {
        config,
        fanout,
        events,
        state_tx,
        metrics: metrics.clone(),
    };
    let task = tokio::spawn(supervisor.run(cancel).instrument(span));
    StreamHandle {
        state,
        metrics,
        task,
    }
}

struct Supervisor {
    config: DeviceConfig,
    fanout: FrameFanout,
    events: EventBus,
    state_tx: watch::Sender<StreamState>,
    metrics: Arc<StreamMetrics>,
}

impl Supervisor {
    fn set_state(&self, state: StreamState) {
        self.state_tx.send_replace(state.clone());
        let _ = self.events.send(NvrEvent::StreamState {
            camera: self.config.camera.clone(),
            role: StreamRole::Main,
            state,
        });
    }

    async fn run(mut self, cancel: CancellationToken) {
        let mut backoff = Backoff::new(self.config.backoff);
        let mut attempt: u32 = 0;
        let mut session: u64 = 0;
        info!(device = %self.config.label, "starting device supervisor");
        loop {
            attempt = attempt.saturating_add(1);
            self.set_state(StreamState::Connecting { attempt });
            let command = tokio::select! {
                _ = cancel.cancelled() => break,
                c = (self.config.command)() => c,
            };
            session += 1;
            let started = Instant::now();
            let outcome = self.run_once(command, session, &cancel).await;
            self.fanout.end_session(session).await;
            let Some(error) = outcome else { break };
            if started.elapsed() >= self.config.backoff.stable_after {
                backoff.reset();
                attempt = 1;
            }
            self.metrics.record_failure();
            let retry_in = backoff.next_delay();
            warn!(attempt, retry_in_ms = retry_in.as_millis() as u64, %error, "device unavailable");
            self.set_state(StreamState::Retrying {
                attempt,
                retry_in,
                error,
            });
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(retry_in) => {}
            }
        }
        self.set_state(StreamState::Stopped);
        info!("device supervisor stopped");
    }

    /// Runs FFmpeg once; why it stopped, or `None` on cancellation.
    async fn run_once(
        &mut self,
        command: DeviceCommand,
        session: u64,
        cancel: &CancellationToken,
    ) -> Option<String> {
        let mut cmd = Command::new(&self.config.ffmpeg);
        cmd.args(&command.args)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Some(format!("FFmpeg ne démarre pas : {e}")),
        };
        let Some(mut stdout) = child.stdout.take() else {
            return Some("FFmpeg sans sortie".into());
        };
        // The last error lines, to tell why it stopped.
        let errors = Arc::new(std::sync::Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let errors = errors.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut e = errors.lock().unwrap_or_else(|e| e.into_inner());
                    if e.len() == STDERR_LINES {
                        e.pop_front();
                    }
                    e.push_back(line);
                }
            });
        }
        self.metrics.record_session();
        let mut reader = AnnexB::default();
        let mut frames = Framer::new(session);
        let mut buf = vec![0u8; 64 * 1024];
        let mut announced = false;
        let outcome = loop {
            let read = tokio::select! {
                biased;
                _ = cancel.cancelled() => break None,
                r = tokio::time::timeout(self.config.stall_timeout, stdout.read(&mut buf)) => r,
            };
            let n = match read {
                Err(_) => {
                    break Some(format!(
                        "aucune image depuis {} s",
                        self.config.stall_timeout.as_secs()
                    ));
                }
                Ok(Err(e)) => break Some(format!("lecture de FFmpeg : {e}")),
                Ok(Ok(0)) => break Some("FFmpeg s'est arrêté".into()),
                Ok(Ok(n)) => n,
            };
            for nal in reader.push(&buf[..n]) {
                let Some(frame) = frames.push(nal) else {
                    continue;
                };
                if !announced {
                    announced = true;
                    let p = &frame.params;
                    info!(session, codec = %p.codec, rfc6381 = %p.rfc6381_codec, width = p.width, height = p.height, "streaming");
                    self.set_state(StreamState::Streaming {
                        session,
                        codec: p.codec,
                        width: p.width,
                        height: p.height,
                        audio: None,
                        since: SystemTime::now(),
                    });
                }
                self.metrics.record_frame(&frame);
                let dropped = self.fanout.send(&frame);
                if dropped > 0 {
                    self.metrics.record_dropped(dropped);
                }
            }
        };
        let _ = child.kill().await;
        let _ = child.wait().await;
        outcome.map(|why| {
            if announced {
                return why;
            }
            // Before any picture: FFmpeg's own words (the cause comes first,
            // the summary last).
            let errors = errors.lock().unwrap_or_else(|e| e.into_inner());
            match ffmpeg_reason(errors.iter().map(String::as_str)) {
                Some(reason) => format!("{why} : {reason}"),
                None => why,
            }
        })
    }
}

/// The last error lines of FFmpeg (three at most, without their
/// `[in#0 @ 0x…]` prefix, each once), joined.
fn ffmpeg_reason<'a>(lines: impl DoubleEndedIterator<Item = &'a str>) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    for line in lines.rev() {
        let line = line.trim();
        let line = match line.strip_prefix('[').and_then(|l| l.split_once("] ")) {
            Some((context, rest)) if context.contains(" @ ") => rest.trim(),
            _ => line,
        };
        if !line.is_empty() && !kept.contains(&line) {
            kept.push(line);
        }
        if kept.len() == 3 {
            break;
        }
    }
    kept.reverse();
    (!kept.is_empty()).then(|| kept.join(" · "))
}

/// Splits an Annex B byte stream into NAL units (start codes removed).
#[derive(Default)]
pub(crate) struct AnnexB {
    buf: Vec<u8>,
}

/// Position of the next start code (`00 00 01`) at or after `from`.
fn start_code(data: &[u8], from: usize) -> Option<usize> {
    data.get(from..)?
        .windows(3)
        .position(|w| w == [0, 0, 1])
        .map(|p| p + from)
}

impl AnnexB {
    /// Adds `data`; the NAL units now complete (the last one is complete
    /// when the next start code arrives).
    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<Bytes> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        let Some(first) = start_code(&self.buf, 0) else {
            // No start code yet: keep at most two bytes (a split one).
            let keep = self.buf.len().min(2);
            self.buf.drain(..self.buf.len() - keep);
            return out;
        };
        let mut begin = first + 3;
        while let Some(next) = start_code(&self.buf, begin) {
            // `00 00 00 01`: the zero belongs to the start code.
            let mut end = next;
            while end > begin && self.buf[end - 1] == 0 {
                end -= 1;
            }
            if end > begin {
                out.push(Bytes::copy_from_slice(&self.buf[begin..end]));
            }
            begin = next + 3;
        }
        self.buf.drain(..begin - 3);
        out
    }
}

/// Groups NAL units into pictures (an access unit delimiter starts each),
/// with the parameters of the parameter sets seen.
pub(crate) struct Framer {
    session: u64,
    seq: u64,
    sps: Option<Bytes>,
    pps: Option<Bytes>,
    params: Option<Arc<VideoParams>>,
    /// NAL units of the picture being received.
    nals: Vec<Bytes>,
    started: Instant,
    last_pts: Option<i64>,
}

impl Framer {
    pub(crate) fn new(session: u64) -> Self {
        Self {
            session,
            seq: 0,
            sps: None,
            pps: None,
            params: None,
            nals: Vec::new(),
            started: Instant::now(),
            last_pts: None,
        }
    }

    /// Adds a NAL unit; the previous picture once complete.
    pub(crate) fn push(&mut self, nal: Bytes) -> Option<VideoFrame> {
        let kind = nal.first().map(|h| h & 0x1f)?;
        match kind {
            // Access unit delimiter: the previous picture is complete.
            9 => self.finish(),
            7 => {
                if self.sps.as_ref() != Some(&nal) {
                    self.sps = Some(nal);
                    self.params = None;
                }
                None
            }
            8 => {
                if self.pps.as_ref() != Some(&nal) {
                    self.pps = Some(nal);
                    self.params = None;
                }
                None
            }
            _ => {
                self.nals.push(nal);
                None
            }
        }
    }

    fn finish(&mut self) -> Option<VideoFrame> {
        if self.nals.is_empty() {
            return None;
        }
        let nals = std::mem::take(&mut self.nals);
        if self.params.is_none() {
            let (sps, pps) = (self.sps.as_ref()?, self.pps.as_ref()?);
            match h264_params(sps, pps) {
                Ok(p) => self.params = Some(Arc::new(p)),
                Err(e) => {
                    warn!(error = %e, "unreadable H.264 parameters: picture dropped");
                    return None;
                }
            }
        }
        let params = self.params.clone()?;
        let is_key = nals
            .iter()
            .any(|n| n.first().is_some_and(|h| h & 0x1f == 5));
        let mut data = Vec::with_capacity(nals.iter().map(|n| n.len() + 4).sum());
        for n in &nals {
            data.extend_from_slice(&(n.len() as u32).to_be_bytes());
            data.extend_from_slice(n);
        }
        // Arrival time on the 90 kHz clock, never going backwards.
        let elapsed = self.started.elapsed().as_micros() as i64;
        let mut pts = elapsed * i64::from(CLOCK_RATE) / 1_000_000;
        if let Some(last) = self.last_pts
            && pts <= last
        {
            pts = last + 1;
        }
        self.last_pts = Some(pts);
        let frame = VideoFrame {
            session: self.session,
            seq: self.seq,
            params,
            pts,
            is_key,
            wall_time: SystemTime::now(),
            loss: 0,
            data: Bytes::from(data),
            audio: None,
        };
        self.seq += 1;
        Some(frame)
    }
}

/// An ISO BMFF box.
fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(payload.len() + 8);
    b.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    b.extend_from_slice(kind);
    b.extend_from_slice(payload);
    b
}

/// Parameters of an H.264 stream from its parameter sets (NAL units with
/// their header): size, RFC 6381 codec and `avc1` sample entry.
pub(crate) fn h264_params(sps: &[u8], pps: &[u8]) -> Result<VideoParams, String> {
    use h264_reader::nal::sps::SeqParameterSet;
    use h264_reader::rbsp::{BitReader, decode_nal};
    if sps.len() < 4 || sps.len() > usize::from(u16::MAX) || pps.len() > usize::from(u16::MAX) {
        return Err("jeu de paramètres H.264 de taille invalide".into());
    }
    let rbsp = decode_nal(sps).map_err(|e| e.to_string())?;
    let parsed =
        SeqParameterSet::from_bits(BitReader::new(&rbsp[..])).map_err(|e| format!("{e:?}"))?;
    let (width, height) = parsed.pixel_dimensions().map_err(|e| format!("{e:?}"))?;
    let (w, h) = (
        u16::try_from(width).map_err(|_| "image trop large")?,
        u16::try_from(height).map_err(|_| "image trop haute")?,
    );
    // avcC: version, profile, constraints, level, 4-byte lengths, one SPS,
    // one PPS (and, for the High profiles, the chroma format and depths).
    let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(sps);
    avcc.push(1);
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(pps);
    if matches!(sps[1], 100 | 110 | 122 | 144) {
        use h264_reader::nal::sps::ChromaFormat;
        let c = &parsed.chroma_info;
        let format = match c.chroma_format {
            ChromaFormat::Monochrome => 0,
            ChromaFormat::YUV420 | ChromaFormat::Invalid(_) => 1,
            ChromaFormat::YUV422 => 2,
            ChromaFormat::YUV444 => 3,
        };
        avcc.extend_from_slice(&[
            0xfc | format,
            0xf8 | (c.bit_depth_luma_minus8 & 7),
            0xf8 | (c.bit_depth_chroma_minus8 & 7),
            0,
        ]);
    }
    // VisualSampleEntry (ISO/IEC 14496-12 and 14496-15).
    let mut entry = vec![0u8; 6];
    entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    entry.extend_from_slice(&[0u8; 16]); // pre_defined, reserved
    entry.extend_from_slice(&w.to_be_bytes());
    entry.extend_from_slice(&h.to_be_bytes());
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    entry.extend_from_slice(&0u32.to_be_bytes());
    entry.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    entry.extend_from_slice(&[0u8; 32]); // compressorname
    entry.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
    entry.extend_from_slice(&(-1i16).to_be_bytes());
    entry.extend_from_slice(&mp4_box(b"avcC", &avcc));
    Ok(VideoParams {
        codec: VideoCodec::H264,
        rfc6381_codec: format!("avc1.{:02X}{:02X}{:02X}", sps[1], sps[2], sps[3]),
        width,
        height,
        clock_rate: CLOCK_RATE,
        sample_entry: Bytes::from(mp4_box(b"avc1", &entry)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_failures_are_told_in_its_words() {
        // FFmpeg 7 on a missing device.
        let lines = [
            "[in#0 @ 0x5596] Error opening input: No such file or directory",
            "Error opening input file /dev/video9.",
            "Error opening input files: No such file or directory",
        ];
        assert_eq!(
            ffmpeg_reason(lines.iter().copied()).as_deref(),
            Some(
                "Error opening input: No such file or directory · Error opening input file /dev/video9. · Error opening input files: No such file or directory"
            )
        );
        assert_eq!(
            ffmpeg_reason(["a", "", "a", "[x] b"].iter().copied()).as_deref(),
            Some("a · [x] b")
        );
        assert_eq!(ffmpeg_reason([" "].iter().copied()), None);
    }

    #[test]
    fn annex_b_is_split_across_reads() {
        let stream = [
            &[0, 0, 0, 1, 9, 0xf0][..],
            &[0, 0, 0, 1, 0x67, 1, 2][..],
            &[0, 0, 1, 0x68, 3][..],
            &[0, 0, 1, 0x65, 0, 0, 3, 1][..],
            &[0, 0, 0, 1, 9, 0xf0][..],
        ]
        .concat();
        // Byte by byte, then all at once: the same units.
        let mut one = AnnexB::default();
        let mut got = Vec::new();
        for b in &stream {
            got.extend(one.push(&[*b]));
        }
        let mut all = AnnexB::default();
        assert_eq!(got, all.push(&stream));
        let nals: Vec<&[u8]> = got.iter().map(|n| &n[..]).collect();
        assert_eq!(
            nals,
            [
                &[9, 0xf0][..],
                &[0x67, 1, 2][..],
                &[0x68, 3][..],
                &[0x65, 0, 0, 3, 1][..]
            ]
        );
    }

    /// Parameter sets of a 1280x720 x264 stream (High profile, level 3.1),
    /// written by FFmpeg 7 (`testsrc2`, `-profile:v high -f h264`).
    const SPS: &[u8] = &[
        0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50, 0x05, 0xbb, 0x01, 0x10, 0x00, 0x00, 0x03,
        0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0x20, 0xf1, 0x83, 0x19, 0x60,
    ];
    const PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];

    #[test]
    fn parameters_of_an_h264_stream() {
        let p = h264_params(SPS, PPS).expect("params");
        assert_eq!((p.width, p.height), (1280, 720));
        assert_eq!(p.rfc6381_codec, "avc1.64001F");
        assert_eq!(&p.sample_entry[4..8], b"avc1");
        let len = u32::from_be_bytes([
            p.sample_entry[0],
            p.sample_entry[1],
            p.sample_entry[2],
            p.sample_entry[3],
        ]);
        assert_eq!(len as usize, p.sample_entry.len());
        let avcc = p
            .sample_entry
            .windows(4)
            .position(|w| w == b"avcC")
            .expect("avcC");
        // Version 1, then the profile, constraints and level of the SPS.
        assert_eq!(&p.sample_entry[avcc + 4..avcc + 8], &[1, 0x64, 0x00, 0x1f]);
        assert!(h264_params(&SPS[..3], PPS).is_err());
    }

    #[test]
    fn pictures_are_grouped_by_delimiter_with_their_parameters() {
        let mut f = Framer::new(7);
        let nal = |b: &[u8]| Bytes::copy_from_slice(b);
        assert!(f.push(nal(&[9, 0x10])).is_none());
        assert!(f.push(nal(SPS)).is_none());
        assert!(f.push(nal(PPS)).is_none());
        assert!(f.push(nal(&[6, 5, 1])).is_none()); // SEI
        assert!(f.push(nal(&[0x65, 0x88])).is_none()); // IDR slice
        let first = f.push(nal(&[9, 0x30])).expect("first picture");
        assert!(first.is_key);
        assert_eq!((first.session, first.seq), (7, 0));
        // Length-prefixed, without the delimiter nor the parameter sets.
        assert_eq!(
            &first.data[..],
            &[0, 0, 0, 3, 6, 5, 1, 0, 0, 0, 2, 0x65, 0x88]
        );
        assert!(f.push(nal(&[0x41, 0x9a])).is_none()); // P slice
        let second = f.push(nal(&[9, 0x30])).expect("second picture");
        assert!(!second.is_key);
        assert_eq!(second.seq, 1);
        assert!(second.pts > first.pts);
        assert!(Arc::ptr_eq(&first.params, &second.params));
        // Without parameter sets yet, nothing comes out.
        let mut g = Framer::new(1);
        assert!(g.push(nal(&[0x65, 0x88])).is_none());
        assert!(g.push(nal(&[9, 0x10])).is_none());
    }
}
