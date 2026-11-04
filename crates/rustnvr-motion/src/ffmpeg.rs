//! FFmpeg processes fed with fragmented MP4 built from live frames: grey
//! pictures for detection, JPEG snapshots.

use bytes::Bytes;
use rustnvr_core::{VideoFrame, VideoParams};
use rustnvr_recorder::fmp4;
use rustnvr_recorder::live_mux::{LiveMuxer, Push};
use rustnvr_stream::LiveTap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, Command};
use tokio::sync::Semaphore;
use tokio::sync::broadcast::error::RecvError;

/// Snapshots taken at the same time, at most (each runs one FFmpeg).
const MAX_CONCURRENT_SNAPSHOTS: usize = 2;
/// A snapshot taking longer is abandoned.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest FFmpeg output accepted for a snapshot (one JPEG per decoded picture).
const MAX_SNAPSHOT_OUTPUT: usize = 64 << 20;

static SNAPSHOTS: Semaphore = Semaphore::const_new(MAX_CONCURRENT_SNAPSHOTS);

/// Size of the analysed picture: `max_width` wide at most, same aspect
/// ratio, even dimensions (required by most pixel formats).
pub fn analysis_size(params: &VideoParams, max_width: u32) -> (u32, u32) {
    let even = |v: u32| (v / 2 * 2).max(2);
    let src_w = params.width.max(2);
    let w = even(max_width.min(src_w));
    let h = even(
        u32::try_from(u64::from(w) * u64::from(params.height.max(2)) / u64::from(src_w))
            .unwrap_or(2),
    );
    (w, h)
}

/// Common arguments. `live`: the input never ends, deliver pictures without
/// buffering (with a finite input, `nobuffer` makes FFmpeg output nothing).
fn base_args(live: bool) -> Vec<String> {
    let nobuffer: &[&str] = if live { &["-fflags", "nobuffer"] } else { &[] };
    ["-hide_banner", "-nostats", "-loglevel", "error"]
        .iter()
        .chain(nobuffer)
        .chain(&[
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
            "-threads",
            "1",
            "-f",
            "mp4",
            "-i",
            "pipe:0",
            "-map",
            "0:v:0",
            "-an",
        ])
        .map(|s| (*s).to_owned())
        .collect()
}

/// Arguments decoding to `w`×`h` grey pictures, `fps` per second, on stdout.
pub fn grey_args(fps: u32, w: u32, h: u32) -> Vec<String> {
    let mut a = base_args(true);
    a.extend([
        "-vf".to_owned(),
        format!("fps={fps},scale={w}:{h}:flags=area,format=gray"),
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-flush_packets".to_owned(),
        "1".to_owned(),
        "pipe:1".to_owned(),
    ]);
    a
}

/// Arguments writing every decoded picture as a JPEG on stdout.
///
/// The caller keeps the last one. Selecting it by number would be fragile:
/// the decoder may drop pictures (H.265 pictures following a CRA key frame
/// that reference the previous group of pictures). JPEG needs full-range
/// YUV, which recent FFmpeg versions no longer convert implicitly.
/// FFmpeg filters blacking out the privacy masks (`[x1, y1, x2, y2]`,
/// picture fractions), joined with commas; empty without masks.
pub fn mask_filters(masks: &[[f32; 4]]) -> String {
    masks
        .iter()
        .map(|m| {
            let [x1, y1, x2, y2] = m.map(|v| v.clamp(0.0, 1.0));
            format!(
                "drawbox=x=iw*{x1:.4}:y=ih*{y1:.4}:w=iw*{:.4}:h=ih*{:.4}:color=black:t=fill",
                (x2 - x1).max(0.0),
                (y2 - y1).max(0.0)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn snapshot_args(max_width: u32, masks: &[[f32; 4]]) -> Vec<String> {
    let mut a = base_args(false);
    let hide = mask_filters(masks);
    let hide = if hide.is_empty() {
        hide
    } else {
        format!("{hide},")
    };
    a.extend([
        "-vf".to_owned(),
        format!("{hide}scale='min({max_width},iw)':-2:out_range=full,format=yuvj420p"),
        "-f".to_owned(),
        "image2pipe".to_owned(),
        "-c:v".to_owned(),
        "mjpeg".to_owned(),
        "-q:v".to_owned(),
        "5".to_owned(),
        "pipe:1".to_owned(),
    ]);
    a
}

/// A running FFmpeg with its pipes.
pub struct Ffmpeg {
    /// The process (killed when dropped).
    pub child: Child,
    /// Input.
    pub stdin: ChildStdin,
    /// Output.
    pub stdout: tokio::process::ChildStdout,
    /// Last line FFmpeg wrote on stderr.
    pub last_error: Arc<Mutex<String>>,
}

impl Ffmpeg {
    /// Starts `ffmpeg` with `args`.
    pub fn spawn(ffmpeg: &str, args: &[String]) -> Result<Self, String> {
        let mut child = Command::new(ffmpeg)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("impossible de lancer FFmpeg ({ffmpeg:?}) : {e}"))?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("FFmpeg : tubes indisponibles".to_owned());
        };
        let last_error = Arc::new(Mutex::new(String::new()));
        tokio::spawn(keep_last_line(stderr, last_error.clone()));
        Ok(Self {
            child,
            stdin,
            stdout,
            last_error,
        })
    }

    /// FFmpeg's last error line, if any.
    pub fn last_error(&self) -> String {
        self.last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

async fn keep_last_line(stderr: ChildStderr, last: Arc<Mutex<String>>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "ffmpeg", "{line}");
        *last.lock().unwrap_or_else(|e| e.into_inner()) = line;
    }
}

/// Why feeding stopped.
pub enum FeedEnd {
    /// The stream changed (new session or parameters) or the consumer lagged:
    /// start again from a key frame.
    Restart(&'static str),
    /// The tap was closed.
    Closed,
}

/// Writes the frames of `source` as fragmented MP4, starting with the init
/// segment of `first` (a key frame, written too), until the stream changes.
pub async fn feed(
    first: VideoFrame,
    backlog: Vec<VideoFrame>,
    mut rx: tokio::sync::broadcast::Receiver<VideoFrame>,
    stdin: &mut ChildStdin,
) -> std::io::Result<FeedEnd> {
    stdin.write_all(&fmp4::init_segment(&first.params)).await?;
    let mut muxer = LiveMuxer::tolerant(&first);
    for f in std::iter::once(first).chain(backlog) {
        if !push(stdin, &mut muxer, f).await? {
            return Ok(FeedEnd::Restart("stream changed"));
        }
    }
    loop {
        match rx.recv().await {
            Ok(f) => {
                if !push(stdin, &mut muxer, f).await? {
                    return Ok(FeedEnd::Restart("stream changed"));
                }
            }
            Err(RecvError::Lagged(_)) => return Ok(FeedEnd::Restart("detection lagging")),
            Err(RecvError::Closed) => return Ok(FeedEnd::Closed),
        }
    }
}

/// Muxes and writes one frame; false if the stream changed.
async fn push(
    stdin: &mut ChildStdin,
    muxer: &mut LiveMuxer,
    f: VideoFrame,
) -> std::io::Result<bool> {
    match muxer.push(f) {
        Push::Nothing => Ok(true),
        Push::Fragment(b) => stdin.write_all(&b).await.map(|()| true),
        Push::Reset(reason) => {
            tracing::debug!(reason, "detection input restarts");
            Ok(false)
        }
    }
}

/// Reads exactly one `len`-byte picture; `None` at the end of the output.
pub async fn read_picture(
    stdout: &mut tokio::process::ChildStdout,
    buf: &mut [u8],
) -> std::io::Result<Option<()>> {
    match stdout.read_exact(buf).await {
        Ok(_) => Ok(Some(())),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e),
    }
}

/// The current picture of a live stream as a JPEG, at most `max_width` wide.
///
/// Decodes the frames since the last key frame (kept by the tap) and encodes
/// the last one. Fails when the stream has no recent key frame (camera
/// offline, or group of pictures too large to be kept). The privacy `masks`
/// (picture fractions) are blacked out.
pub async fn snapshot_jpeg(
    ffmpeg: &str,
    source: &LiveTap,
    max_width: u32,
    masks: &[[f32; 4]],
) -> Result<Bytes, String> {
    let (frames, rx) = source.subscribe();
    drop(rx);
    let Some(first) = frames.first() else {
        return Err("aucune image récente (caméra hors ligne ?)".to_owned());
    };
    let mut input = fmp4::init_segment(&first.params);
    let mut muxer = LiveMuxer::tolerant(first);
    for f in frames {
        match muxer.push(f) {
            Push::Fragment(b) => input.extend_from_slice(&b),
            Push::Nothing => {}
            Push::Reset(reason) => return Err(format!("flux instable ({reason})")),
        }
    }
    if let Some(b) = muxer.finish() {
        input.extend_from_slice(&b);
    }
    jpeg_from(ffmpeg, &snapshot_args(max_width, masks), input).await
}

/// `jpeg` with the boxes `[x1, y1, x2, y2]` (fractions of the picture)
/// drawn in blue, as the interface shows them.
pub async fn boxed_jpeg(ffmpeg: &str, jpeg: Bytes, boxes: &[[f32; 4]]) -> Result<Bytes, String> {
    if boxes.is_empty() {
        return Ok(jpeg);
    }
    jpeg_from(ffmpeg, &boxed_args(boxes), jpeg.to_vec()).await
}

/// FFmpeg arguments drawing `boxes` on a JPEG read from standard input.
fn boxed_args(boxes: &[[f32; 4]]) -> Vec<String> {
    let mut filters: Vec<String> = boxes
        .iter()
        .map(|b| {
            let [x1, y1, x2, y2] = b.map(|v| v.clamp(0.0, 1.0));
            format!(
                "drawbox=x=iw*{x1:.4}:y=ih*{y1:.4}:w=iw*{:.4}:h=ih*{:.4}:color=0x3B82F6:t=max(2\\,iw/240)",
                (x2 - x1).max(0.0),
                (y2 - y1).max(0.0)
            )
        })
        .collect();
    filters.push("format=yuvj420p".to_owned());
    [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-threads",
        "1",
        "-f",
        "jpeg_pipe",
        "-i",
        "pipe:0",
        "-vf",
        &filters.join(","),
        "-frames:v",
        "1",
        "-f",
        "image2pipe",
        "-c:v",
        "mjpeg",
        "-q:v",
        "3",
        "pipe:1",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Runs FFmpeg with `args` on `input` and returns the last JPEG it wrote.
async fn jpeg_from(ffmpeg: &str, args: &[String], input: Vec<u8>) -> Result<Bytes, String> {
    let _permit = SNAPSHOTS
        .acquire()
        .await
        .map_err(|_| "captures indisponibles".to_owned())?;
    let run = async {
        let mut p = Ffmpeg::spawn(ffmpeg, args)?;
        let mut stdin = p.stdin;
        // Write while reading, so that neither pipe can fill up and block.
        let writer = tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.shutdown().await;
        });
        let mut output = Vec::new();
        (&mut p.stdout)
            .take(MAX_SNAPSHOT_OUTPUT as u64 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|e| e.to_string())?;
        let _ = writer.await;
        let status = p.child.wait().await.map_err(|e| e.to_string())?;
        if output.len() > MAX_SNAPSHOT_OUTPUT {
            return Err("capture trop volumineuse".to_owned());
        }
        match last_jpeg(&output) {
            Some(jpeg) => Ok(Bytes::copy_from_slice(jpeg)),
            None => {
                let detail = p
                    .last_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                Err(format!(
                    "FFmpeg n'a pas produit d'image ({status}) {detail}"
                ))
            }
        }
    };
    tokio::time::timeout(SNAPSHOT_TIMEOUT, run)
        .await
        .map_err(|_| "capture trop longue (délai dépassé)".to_owned())?
}

/// The last complete JPEG of concatenated JPEGs. Entropy-coded data escapes
/// 0xFF bytes, so an end-of-image marker followed by a start-of-image marker
/// only occurs between pictures.
fn last_jpeg(data: &[u8]) -> Option<&[u8]> {
    if !data.ends_with(&[0xff, 0xd9]) {
        return None;
    }
    let start = data
        .windows(4)
        .rposition(|w| w == [0xff, 0xd9, 0xff, 0xd8])
        .map_or(0, |i| i + 2);
    data[start..]
        .starts_with(&[0xff, 0xd8])
        .then(|| &data[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::VideoCodec;

    #[test]
    fn draws_one_box_per_object() {
        let args = boxed_args(&[[0.25, 0.5, 0.75, 1.2]]);
        let vf = &args[args.iter().position(|a| a == "-vf").expect("-vf") + 1];
        assert_eq!(
            vf,
            "drawbox=x=iw*0.2500:y=ih*0.5000:w=iw*0.5000:h=ih*0.5000:color=0x3B82F6:t=max(2\\,iw/240),format=yuvj420p"
        );
    }

    #[test]
    fn masks_are_filled_before_scaling() {
        assert_eq!(mask_filters(&[]), "");
        let args = snapshot_args(640, &[[0.5, 0.0, 1.0, 0.25]]);
        let vf = &args[args.iter().position(|a| a == "-vf").expect("-vf") + 1];
        assert!(
            vf.starts_with(
                "drawbox=x=iw*0.5000:y=ih*0.0000:w=iw*0.5000:h=ih*0.2500:color=black:t=fill,scale="
            ),
            "{vf}"
        );
    }

    #[test]
    fn keeps_the_last_jpeg() {
        let one = [0xff, 0xd8, 1, 0xff, 0x00, 0xff, 0xd9];
        let two = [0xff, 0xd8, 2, 0xff, 0xd9];
        let both: Vec<u8> = one.iter().chain(&two).copied().collect();
        assert_eq!(last_jpeg(&both), Some(&two[..]));
        assert_eq!(last_jpeg(&one), Some(&one[..]));
        assert_eq!(last_jpeg(&both[..both.len() - 1]), None, "truncated");
        assert_eq!(last_jpeg(b""), None);
    }

    #[test]
    fn analysis_keeps_aspect_ratio_with_even_sizes() {
        let p = |w, h| VideoParams {
            codec: VideoCodec::H265,
            rfc6381_codec: String::new(),
            width: w,
            height: h,
            clock_rate: 90_000,
            sample_entry: Bytes::new(),
        };
        assert_eq!(analysis_size(&p(768, 432), 320), (320, 180));
        assert_eq!(analysis_size(&p(1920, 1080), 320), (320, 180));
        assert_eq!(analysis_size(&p(640, 480), 320), (320, 240));
        assert_eq!(analysis_size(&p(200, 101), 320), (200, 100));
        assert!(
            grey_args(5, 320, 180)
                .iter()
                .any(|a| a == "fps=5,scale=320:180:flags=area,format=gray")
        );
    }
}
