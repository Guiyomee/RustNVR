//! On-demand H.264 conversion of a live stream, for browsers that cannot
//! decode the camera codec (typically H.265 in Firefox).
//!
//! One FFmpeg process per camera stream, shared by all its viewers, started
//! on the first request and stopped once nobody watches. Recording is never
//! involved: it keeps the original stream.
//!
//! FFmpeg reads fragmented MP4 (with the original timestamps) on stdin and
//! writes fragmented MP4 with one H.264 frame per fragment on stdout. The
//! output is parsed back into frames published on a [`LiveTap`], so converted
//! viewers use exactly the same delivery path as direct ones.

use bytes::Bytes;
use rustnvr_core::config::LiveConfig;
use rustnvr_core::{AudioParams, AudioSample, FrameAudio, VideoCodec, VideoFrame, VideoParams};
use rustnvr_recorder::fmp4;
use rustnvr_recorder::live_mux::{LiveMuxer, Push};
use rustnvr_recorder::scan::find_box;
use rustnvr_stream::LiveTap;
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
};
use tokio::process::{ChildStdin, Command};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// A conversion without viewers for this long is stopped.
const IDLE_STOP: Duration = Duration::from_secs(10);
/// Largest box accepted from FFmpeg (a single frame).
const MAX_BOX: u64 = 64 << 20;

/// Live view settings, from `[live]` in the configuration.
#[derive(Clone, Debug)]
pub struct LiveSettings {
    /// Whether H.264 conversion may be requested.
    pub transcode: bool,
    /// FFmpeg executable.
    pub ffmpeg: String,
    /// Bit rate cap of the converted stream.
    pub max_bitrate_kbps: u32,
    /// Graphics card for the conversions: filled by the server from the test
    /// of `live.hwaccel` (a value given here is ignored).
    pub hw: Option<crate::hwaccel::HwEncoder>,
}

impl From<&LiveConfig> for LiveSettings {
    fn from(c: &LiveConfig) -> Self {
        Self {
            transcode: c.transcode,
            ffmpeg: c.ffmpeg.clone(),
            max_bitrate_kbps: c.max_bitrate_kbps,
            hw: None,
        }
    }
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self::from(&LiveConfig::default())
    }
}

/// What a viewer of a conversion needs.
pub(crate) struct TranscodeHandle {
    /// Converted frames.
    pub tap: LiveTap,
    /// Becomes `Some(reason)` if the conversion fails.
    pub failure: watch::Receiver<Option<String>>,
}

/// Output tap and failure signal of each running conversion, by `camera/stream`.
type Running = HashMap<String, (LiveTap, watch::Receiver<Option<String>>)>;

/// Running conversions.
#[derive(Default)]
pub(crate) struct Transcoders {
    running: Mutex<Running>,
}

impl Transcoders {
    fn running(&self) -> std::sync::MutexGuard<'_, Running> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns the conversion of `source`, starting it if needed.
    pub(crate) fn get_or_start(
        self: &Arc<Self>,
        key: String,
        source: LiveTap,
        settings: LiveSettings,
        shutdown: CancellationToken,
    ) -> TranscodeHandle {
        let mut running = self.running();
        if let Some((tap, failure)) = running.get(&key) {
            return TranscodeHandle {
                tap: tap.clone(),
                failure: failure.clone(),
            };
        }
        let tap = LiveTap::new();
        let (failure_tx, failure) = watch::channel(None);
        running.insert(key.clone(), (tap.clone(), failure.clone()));
        drop(running);

        let this = self.clone();
        let out = tap.clone();
        tokio::spawn(async move {
            tracing::info!(stream = %key, "starting live H.264 conversion");
            match supervise(&source, &out, &settings, &shutdown).await {
                Ok(()) => {
                    tracing::info!(stream = %key, "live H.264 conversion stopped (no viewer)")
                }
                Err(reason) => {
                    tracing::warn!(stream = %key, %reason, "live H.264 conversion failed");
                    let _ = failure_tx.send(Some(reason));
                }
            }
            let mut running = this.running();
            if running.get(&key).is_some_and(|(t, _)| t.same_as(&out)) {
                running.remove(&key);
            }
        });
        TranscodeHandle { tap, failure }
    }
}

enum Exit {
    Idle,
    Shutdown,
    SourceChanged,
}

/// Runs FFmpeg, restarting it when the source stream changes, until nobody watches.
async fn supervise(
    source: &LiveTap,
    out: &LiveTap,
    settings: &LiveSettings,
    shutdown: &CancellationToken,
) -> Result<(), String> {
    let mut settings = settings.clone();
    let mut session = 0;
    loop {
        session += 1;
        match run_ffmpeg(source, out, &settings, session, shutdown).await {
            Ok(Exit::Idle | Exit::Shutdown) => return Ok(()),
            Ok(Exit::SourceChanged) => continue,
            // On the graphics card: once more on the processor.
            Err(reason) if settings.hw.is_some() => {
                tracing::warn!(%reason, "live conversion failed on the graphics card: redone on the processor");
                settings.hw = None;
            }
            Err(reason) => return Err(reason),
        }
    }
}

fn ffmpeg_args(s: &LiveSettings) -> Vec<String> {
    let rate = format!("{}k", s.max_bitrate_kbps);
    let buf = format!("{}k", 2 * s.max_bitrate_kbps);
    let hw = s.hw.as_ref();
    let encoder = crate::hwaccel::live_encoder_args(hw, &rate, &buf);
    let input = crate::hwaccel::input_args(hw);
    [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        // Start immediately: the input is MP4 whose parameters are in `moov`.
        "-fflags",
        "nobuffer",
        "-probesize",
        "32",
        "-analyzeduration",
        "0",
        // Frame threading would hold one frame per core before the first
        // output; slice threading keeps parallelism without that delay.
        "-thread_type",
        "slice",
    ]
    .into_iter()
    .chain(input.iter().map(String::as_str))
    .chain(["-f", "mp4", "-i", "pipe:0", "-map", "0:v:0", "-an"])
    .chain(encoder.iter().map(String::as_str))
    .chain([
        // A key frame every 2 s lets new viewers start quickly.
        "-force_key_frames",
        "expr:gte(t,n_forced*2)",
    ])
    .chain([
        // Keep the original timestamps, one output frame per input frame.
        "-fps_mode",
        "passthrough",
        "-f",
        "mp4",
        "-movflags",
        "empty_moov+default_base_moof+frag_every_frame",
        "-video_track_timescale",
        "90000",
        "-flush_packets",
        "1",
        "pipe:1",
    ])
    .map(str::to_owned)
    .collect()
}

async fn run_ffmpeg(
    source: &LiveTap,
    out: &LiveTap,
    settings: &LiveSettings,
    session: u64,
    shutdown: &CancellationToken,
) -> Result<Exit, String> {
    let mut command = Command::new(&settings.ffmpeg);
    if let Some(hw) = &settings.hw {
        hw.apply_env(&mut command);
    }
    let mut child = command
        .args(ffmpeg_args(settings))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            format!(
                "transcode: cannot start FFmpeg ({:?}): {e}",
                settings.ffmpeg
            )
        })?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err("transcode: FFmpeg pipes unavailable".to_owned());
    };

    // Keep FFmpeg's last error line for the failure message.
    let last_error = Arc::new(Mutex::new(String::new()));
    let stderr_lines = last_error.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "ffmpeg", "{line}");
            *stderr_lines.lock().unwrap_or_else(|e| e.into_inner()) = line;
        }
    });
    let failed = |what: &str| {
        let detail = last_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
        format!(
            "transcode: FFmpeg {what}{}{detail}",
            if detail.is_empty() { "" } else { ": " }
        )
    };

    let audio = SharedAudio::default();
    let feed = feed(source.clone(), stdin, audio.clone());
    let read = read_output(stdout, out.clone(), session, audio);
    tokio::pin!(feed, read);
    let mut idle_since: Option<Instant> = None;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(Exit::Shutdown),
            fed = &mut feed => {
                return match fed {
                    Ok(Exit::SourceChanged) => Ok(Exit::SourceChanged),
                    Ok(other) => Ok(other),
                    // Broken pipe: FFmpeg exited; report why.
                    Err(_) => {
                        let status = child.wait().await.map(|s| s.to_string()).unwrap_or_default();
                        Err(failed(&format!("stopped ({status})")))
                    }
                };
            }
            result = &mut read => {
                let status = child.wait().await.map(|s| s.to_string()).unwrap_or_default();
                return Err(failed(&format!("stopped ({status}, {})", result.err().unwrap_or_default())));
            }
            _ = ticker.tick() => {
                if out.viewers() > 0 {
                    idle_since = None;
                } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= IDLE_STOP {
                    return Ok(Exit::Idle);
                }
            }
        }
    }
}

/// Sound of the source, passed around FFmpeg (which only converts the
/// picture): written by [`feed`], attached to the converted pictures by
/// [`read_output`]. Timestamps are rebased on the first picture sent to
/// FFmpeg, whose output timeline starts there too (`-fps_mode passthrough`).
#[derive(Default)]
struct AudioBypass {
    params: Option<Arc<AudioParams>>,
    samples: VecDeque<AudioSample>,
}

/// Sound kept waiting for the converted pictures (≈ 30 s of AAC at 16 kHz).
const MAX_BYPASSED_AUDIO: usize = 512;

type SharedAudio = Arc<Mutex<AudioBypass>>;

/// Writes the source as fragmented MP4 to FFmpeg, from its last key frame.
async fn feed(source: LiveTap, mut stdin: ChildStdin, audio: SharedAudio) -> std::io::Result<Exit> {
    let (backlog, mut rx) = source.subscribe();
    let mut muxer: Option<LiveMuxer> = None;
    let mut first_pts: Option<(i64, u32)> = None;
    for frame in backlog {
        if let Some(exit) =
            write_frame(&mut stdin, &mut muxer, &mut first_pts, &audio, frame).await?
        {
            return Ok(exit);
        }
    }
    loop {
        match rx.recv().await {
            Ok(frame) => {
                if let Some(exit) =
                    write_frame(&mut stdin, &mut muxer, &mut first_pts, &audio, frame).await?
                {
                    return Ok(exit);
                }
            }
            // Restart cleanly from a key frame.
            Err(RecvError::Lagged(_)) => return Ok(Exit::SourceChanged),
            Err(RecvError::Closed) => return Ok(Exit::Idle),
        }
    }
}

async fn write_frame(
    stdin: &mut ChildStdin,
    muxer: &mut Option<LiveMuxer>,
    first_pts: &mut Option<(i64, u32)>,
    audio: &SharedAudio,
    frame: VideoFrame,
) -> std::io::Result<Option<Exit>> {
    let m = match muxer {
        Some(m) => m,
        None => {
            if !frame.is_key {
                return Ok(None);
            }
            stdin.write_all(&fmp4::init_segment(&frame.params)).await?;
            *first_pts = Some((frame.pts, frame.params.clock_rate.max(1)));
            muxer.insert(LiveMuxer::tolerant(&frame))
        }
    };
    if let (Some(fa), Some((pts, rate))) = (&frame.audio, *first_pts) {
        let start =
            i64::try_from(i128::from(pts) * i128::from(fa.params.clock_rate) / i128::from(rate))
                .unwrap_or(0);
        let mut a = audio.lock().unwrap_or_else(|e| e.into_inner());
        if a.params.as_deref() != Some(&*fa.params) {
            a.params = Some(fa.params.clone());
            a.samples.clear();
        }
        for s in &fa.samples {
            if s.pts >= start {
                if a.samples.len() >= MAX_BYPASSED_AUDIO {
                    a.samples.pop_front();
                }
                a.samples.push_back(AudioSample {
                    pts: s.pts - start,
                    ..s.clone()
                });
            }
        }
    }
    match m.push(frame) {
        Push::Nothing => Ok(None),
        Push::Fragment(bytes) => {
            stdin.write_all(&bytes).await?;
            Ok(None)
        }
        Push::Reset(_) => Ok(Some(Exit::SourceChanged)),
    }
}

/// The bypassed sound up to `pts` (90 kHz), for a converted picture.
fn audio_until(audio: &SharedAudio, pts: i64) -> Option<Arc<FrameAudio>> {
    let mut a = audio.lock().unwrap_or_else(|e| e.into_inner());
    let params = a.params.clone()?;
    let limit =
        i64::try_from(i128::from(pts) * i128::from(params.clock_rate) / 90_000).unwrap_or(0);
    let mut samples = Vec::new();
    while a.samples.front().is_some_and(|s| s.pts <= limit) {
        samples.extend(a.samples.pop_front());
    }
    Some(Arc::new(FrameAudio { params, samples }))
}

/// Reads FFmpeg's fragmented MP4 output and publishes each frame on `out`.
async fn read_output(
    mut stdout: impl AsyncRead + Unpin,
    out: LiveTap,
    session: u64,
    audio: SharedAudio,
) -> Result<(), String> {
    let mut params: Option<Arc<VideoParams>> = None;
    let mut moof: Option<Vec<u8>> = None;
    let mut seq = 0u64;
    loop {
        let Some((fourcc, body)) = read_box(&mut stdout).await? else {
            return Err("output ended".to_owned());
        };
        match &fourcc {
            b"moov" => {
                params = Some(Arc::new(
                    parse_params(&body).ok_or("unexpected FFmpeg output (moov)")?,
                ))
            }
            b"moof" => moof = Some(body),
            b"mdat" => {
                let (Some(p), Some(m)) = (&params, moof.take()) else {
                    continue;
                };
                for (pts, data) in samples(&m, body).ok_or("unexpected FFmpeg output (moof)")? {
                    out.publish(&VideoFrame {
                        session,
                        seq,
                        params: p.clone(),
                        pts,
                        is_key: h264_has_idr(&data),
                        wall_time: SystemTime::now(),
                        loss: 0,
                        audio: audio_until(&audio, pts),
                        data,
                    });
                    seq += 1;
                }
            }
            _ => {}
        }
    }
}

async fn read_box(r: &mut (impl AsyncRead + Unpin)) -> Result<Option<([u8; 4], Vec<u8>)>, String> {
    let mut header = [0u8; 8];
    match r.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    let size = u64::from(u32::from_be_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let fourcc = [header[4], header[5], header[6], header[7]];
    let body_len = match size {
        1 => {
            let mut large = [0u8; 8];
            r.read_exact(&mut large).await.map_err(|e| e.to_string())?;
            u64::from_be_bytes(large)
                .checked_sub(16)
                .ok_or("invalid box size")?
        }
        0..8 => return Err("invalid box size".to_owned()),
        n => n - 8,
    };
    if body_len > MAX_BOX {
        return Err("box too large".to_owned());
    }
    let mut body = vec![0; usize::try_from(body_len).map_err(|e| e.to_string())?];
    r.read_exact(&mut body).await.map_err(|e| e.to_string())?;
    Ok(Some((fourcc, body)))
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Video parameters from an H.264 `moov` written by FFmpeg.
fn parse_params(moov: &[u8]) -> Option<VideoParams> {
    let mdhd = find_box(moov, &[b"trak", b"mdia", b"mdhd"])?;
    let clock_rate = if *mdhd.first()? == 1 {
        be32(mdhd, 20)?
    } else {
        be32(mdhd, 12)?
    };
    let stsd = find_box(moov, &[b"trak", b"mdia", b"minf", b"stbl", b"stsd"])?;
    let entry = stsd.get(8..)?;
    let entry = entry.get(..usize::try_from(be32(entry, 0)?).ok()?)?;
    if !matches!(entry.get(4..8)?, b"avc1" | b"avc3") {
        return None;
    }
    // VisualSampleEntry: 8-byte header, then width at 24 and height at 26 of
    // the body; child boxes (avcC) follow the 78-byte fixed part.
    let width = u32::from(be16(entry, 32)?);
    let height = u32::from(be16(entry, 34)?);
    let avcc = find_box(entry.get(86..)?, &[b"avcC"])?;
    let rfc6381_codec = format!(
        "avc1.{:02X}{:02X}{:02X}",
        avcc.get(1)?,
        avcc.get(2)?,
        avcc.get(3)?
    );
    Some(VideoParams {
        codec: VideoCodec::H264,
        rfc6381_codec,
        width,
        height,
        clock_rate,
        sample_entry: Bytes::copy_from_slice(entry),
    })
}

/// `(pts, data)` of each sample of a `moof` + `mdat` pair.
fn samples(moof: &[u8], mdat: Vec<u8>) -> Option<Vec<(i64, Bytes)>> {
    let traf = find_box(moof, &[b"traf"])?;
    let tfhd = find_box(traf, &[b"tfhd"])?;
    let tfhd_flags = be32(tfhd, 0)? & 0x00ff_ffff;
    // tfhd optional fields after track_ID, in order.
    let mut pos = 8;
    if tfhd_flags & 0x01 != 0 {
        pos += 8;
    }
    if tfhd_flags & 0x02 != 0 {
        pos += 4;
    }
    let default_duration = if tfhd_flags & 0x08 != 0 {
        let d = be32(tfhd, pos)?;
        pos += 4;
        d
    } else {
        0
    };
    let default_size = if tfhd_flags & 0x10 != 0 {
        Some(be32(tfhd, pos)?)
    } else {
        None
    };

    let tfdt = find_box(traf, &[b"tfdt"])?;
    let base = if *tfdt.first()? == 1 {
        u64::from_be_bytes(tfdt.get(4..12)?.try_into().ok()?)
    } else {
        u64::from(be32(tfdt, 4)?)
    };
    let trun = find_box(traf, &[b"trun"])?;
    let flags = be32(trun, 0)? & 0x00ff_ffff;
    let count = be32(trun, 4)?;
    let mut pos = 8;
    if flags & 0x001 != 0 {
        pos += 4; // data_offset: samples are contiguous at the start of mdat
    }
    if flags & 0x004 != 0 {
        pos += 4; // first_sample_flags
    }
    let mdat = Bytes::from(mdat);
    let (mut out, mut t, mut offset) = (Vec::new(), base, 0usize);
    for _ in 0..count {
        let mut duration = default_duration;
        let mut size = default_size;
        if flags & 0x100 != 0 {
            duration = be32(trun, pos)?;
            pos += 4;
        }
        if flags & 0x200 != 0 {
            size = Some(be32(trun, pos)?);
            pos += 4;
        }
        if flags & 0x400 != 0 {
            pos += 4;
        }
        if flags & 0x800 != 0 {
            pos += 4;
        }
        let size = match size {
            Some(s) => usize::try_from(s).ok()?,
            None if count == 1 => mdat.len(),
            None => return None,
        };
        let data = mdat.get(offset..offset.checked_add(size)?)?;
        out.push((i64::try_from(t).ok()?, mdat.slice_ref(data)));
        offset += size;
        t += u64::from(duration);
    }
    Some(out)
}

/// True if a 4-byte length-prefixed H.264 access unit contains an IDR slice.
fn h264_has_idr(mut data: &[u8]) -> bool {
    while data.len() > 4 {
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let Some(nal) = data.get(4..4 + len) else {
            return false;
        };
        if nal.first().is_some_and(|h| h & 0x1f == 5) {
            return true;
        }
        data = &data[4 + len..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_recorder::fmp4::SampleMeta;

    #[test]
    fn live_conversion_runs_on_the_processor_or_the_card() {
        let cpu = LiveSettings::default();
        let args = ffmpeg_args(&cpu).join(" ");
        assert!(
            args.contains("-c:v libx264 -preset ultrafast -tune zerolatency"),
            "{args}"
        );
        assert!(!args.contains("vaapi"), "{args}");

        let card = LiveSettings {
            hw: Some(crate::hwaccel::HwEncoder {
                api: crate::hwaccel::HwApi::Vaapi,
                device: Some("/dev/dri/renderD128".into()),
                driver: None,
                constant_qp: false,
                card: "Intel".into(),
            }),
            ..LiveSettings::default()
        };
        let args = ffmpeg_args(&card);
        let text = args.join(" ");
        // Hardware decoding is an input option: before `-i`.
        let hwaccel = args.iter().position(|a| a == "-hwaccel").expect("-hwaccel");
        let input = args.iter().position(|a| a == "-i").expect("-i");
        assert!(hwaccel < input, "{text}");
        assert!(
            text.contains("-vf format=nv12,hwupload -c:v h264_vaapi -bf 0"),
            "{text}"
        );
        assert!(
            text.contains("-b:v 2000k -maxrate 2000k -bufsize 4000k"),
            "{text}"
        );
        assert!(
            !text.contains("libx264") && !text.contains("-crf"),
            "{text}"
        );
        // Same output format as on the processor.
        assert!(text.ends_with("-flush_packets 1 pipe:1"), "{text}");
    }

    fn h264_params() -> VideoParams {
        // avc1 entry with a minimal avcC (version, profile 0x64, compat 0x00, level 0x1F).
        let mut entry = Vec::new();
        entry.extend_from_slice(&0u32.to_be_bytes());
        entry.extend_from_slice(b"avc1");
        entry.extend_from_slice(&[0; 6]);
        entry.extend_from_slice(&1u16.to_be_bytes());
        entry.extend_from_slice(&[0; 16]);
        entry.extend_from_slice(&768u16.to_be_bytes());
        entry.extend_from_slice(&432u16.to_be_bytes());
        entry.extend_from_slice(&[0; 50]);
        entry.extend_from_slice(&15u32.to_be_bytes());
        entry.extend_from_slice(b"avcC");
        entry.extend_from_slice(&[1, 0x64, 0x00, 0x1f, 0xff, 0xe0, 0x00]);
        let len = entry.len() as u32;
        entry[..4].copy_from_slice(&len.to_be_bytes());
        VideoParams {
            codec: VideoCodec::H264,
            rfc6381_codec: String::new(),
            width: 768,
            height: 432,
            clock_rate: 90_000,
            sample_entry: Bytes::from(entry),
        }
    }

    #[test]
    fn parses_params_and_samples_back() {
        let p = h264_params();
        let init = fmp4::init_segment(&p);
        let moov = find_box(&init, &[b"moov"]).expect("moov");
        let parsed = parse_params(moov).expect("params");
        assert_eq!(parsed.rfc6381_codec, "avc1.64001F");
        assert_eq!(
            (parsed.width, parsed.height, parsed.clock_rate),
            (768, 432, 90_000)
        );
        assert_eq!(parsed.sample_entry, p.sample_entry);

        let idr = [0, 0, 0, 2, 0x65, 0xaa];
        let header = fmp4::fragment_header(
            1,
            12_345,
            &[SampleMeta {
                duration: 6000,
                size: 6,
                is_key: true,
            }],
        );
        let moof = find_box(&header, &[b"moof"]).expect("moof");
        let s = samples(moof, idr.to_vec()).expect("samples");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, 12_345);
        assert!(h264_has_idr(&s[0].1));
        assert!(!h264_has_idr(&[0, 0, 0, 2, 0x41, 0xaa]));
    }
}
