//! Short video clips of events, for the notifications: a few seconds from
//! just before the start of an event, cut from the recordings once their
//! segment is finished, converted to a small H.264 file (privacy masks
//! burned in) that phones and chat applications play.

use super::Shared;
use bytes::Bytes;
use rustnvr_api::journeys::clock;
use rustnvr_core::DetectionEvent;
use rustnvr_storage::SegmentRecord;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::debug;

/// The clip starts this long before the event.
const BEFORE_MS: i64 = 2_000;
/// Shortest and longest clip after the start of the event.
const MIN_AFTER_MS: i64 = 5_000;
const MAX_AFTER_MS: i64 = 13_000;
/// How often the finished segments are looked for, and for how long.
const POLL: Duration = Duration::from_secs(10);
const GIVE_UP: Duration = Duration::from_secs(5 * 60);
/// A conversion taking longer is abandoned.
const CONVERT_TIMEOUT: Duration = Duration::from_secs(120);
/// Largest clip sent (Discord accepts 10 MB without a subscription).
pub(super) const MAX_CLIP_BYTES: usize = 9 * 1024 * 1024;
/// Height of the clip, at most.
const HEIGHT: u32 = 480;

/// One conversion at a time: clips must not slow the recording down.
static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// The period of the clip of `event`: from just before its start to its end,
/// between 5 and 13 s after the start.
pub(super) fn clip_period(event: &DetectionEvent) -> (i64, i64) {
    let start = event.start_ms;
    let end = event
        .end_ms
        .unwrap_or(start)
        .clamp(start + MIN_AFTER_MS, start + MAX_AFTER_MS);
    (start - BEFORE_MS, end)
}

/// Segments to join, back to back: one destination per moment.
pub(super) fn chain(list: Vec<SegmentRecord>) -> Vec<SegmentRecord> {
    let mut out: Vec<SegmentRecord> = Vec::new();
    for s in list {
        if out
            .last()
            .is_none_or(|last| s.start_ms >= last.end_ms - 500)
        {
            out.push(s);
        }
    }
    out
}

/// The clip of `event`, waiting for its recordings to be finished.
pub(super) async fn event_clip(shared: &Shared, event: &DetectionEvent) -> Result<Bytes, String> {
    if !shared.ffmpeg_ok {
        return Err("FFmpeg indisponible".to_owned());
    }
    let db = shared
        .database
        .as_ref()
        .ok_or("base de données indisponible")?;
    let (from, to) = clip_period(event);
    let camera = event.camera.as_str();
    let deadline = tokio::time::Instant::now() + GIVE_UP;
    let Cut {
        files,
        offset_ms,
        length_ms,
    } = cut(shared, db, camera, from, to, deadline).await?;
    let masks = super::detection::camera_settings(shared, &event.camera)
        .await
        .mask_rects();
    let _permit = ONE_AT_A_TIME.acquire().await.map_err(|e| e.to_string())?;
    let dir = shared.data_dir.join("clips");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let work = dir.join(format!("{}-{}", std::process::id(), event.id));
    let list_path = work.with_extension("txt");
    let out_path = work.with_extension("mp4");
    let result = convert(
        &shared.ffmpeg,
        &files,
        &list_path,
        &out_path,
        offset_ms,
        length_ms,
        &rustnvr_motion::mask_filters(&masks),
    )
    .await;
    let _ = tokio::fs::remove_file(&list_path).await;
    let read = match result {
        Ok(()) => tokio::fs::read(&out_path).await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    let _ = tokio::fs::remove_file(&out_path).await;
    let mp4 = read?;
    if mp4.len() > MAX_CLIP_BYTES {
        return Err(format!("clip trop lourd ({} Mo)", mp4.len() / 1_000_000));
    }
    debug!(camera, id = event.id, bytes = mp4.len(), "event clip made");
    Ok(Bytes::from(mp4))
}

/// The recordings of a camera for a period: files, then where the period
/// starts in them and how long it lasts.
struct Cut {
    files: Vec<PathBuf>,
    offset_ms: i64,
    length_ms: i64,
}

/// The recordings of `camera` from `from` to `to`. The segment holding the
/// end is written until it is cut (a minute by default): it is waited for
/// until `deadline`, then what is there is used.
async fn cut(
    shared: &Shared,
    db: &rustnvr_storage::Database,
    camera: &str,
    from: i64,
    to: i64,
    deadline: tokio::time::Instant,
) -> Result<Cut, String> {
    let segments = loop {
        let list = chain(
            db.segments(camera, from, to)
                .await
                .map_err(|e| e.to_string())?,
        );
        let covered = list.last().is_some_and(|s| s.end_ms >= to);
        if covered || tokio::time::Instant::now() >= deadline {
            break list;
        }
        tokio::time::sleep(POLL).await;
    };
    let first = segments
        .first()
        .ok_or("aucun enregistrement pour cet événement")?;
    let roots: Vec<(String, PathBuf)> = shared
        .api
        .get()
        .map(|a| {
            a.destinations()
                .into_iter()
                .map(|d| (d.id, d.root))
                .collect()
        })
        .unwrap_or_default();
    let files: Vec<PathBuf> = segments
        .iter()
        .filter_map(|s| {
            let root = &roots.iter().find(|(d, _)| *d == s.destination)?.1;
            Some(root.join(&s.camera).join(&s.day).join(&s.file))
        })
        .collect();
    if files.is_empty() {
        return Err("enregistrements introuvables".to_owned());
    }
    Ok(Cut {
        files,
        offset_ms: (from - first.start_ms).max(0),
        length_ms: to - from.max(first.start_ms),
    })
}

/// One camera of a journey video: what it shows, and its banner.
pub(super) struct JourneyPart {
    /// Camera.
    pub camera: rustnvr_core::CameraId,
    /// Name shown in the banner.
    pub name: String,
    /// Period shown, ms since the Unix epoch.
    pub from_ms: i64,
    pub to_ms: i64,
}

/// Longest journey video, and longest part of it.
const JOURNEY_MAX_MS: i64 = 60_000;
const JOURNEY_PART_MAX_MS: i64 = 20_000;
/// Size of a journey video (every camera brought to it).
const JOURNEY_SIZE: (u32, u32) = (960, 540);

/// The parts of the video of a journey: the moments each camera sees it,
/// one after the other — 2 s before the first, the next camera as soon as
/// it sees it, 2 s after the last — 20 s at most each, 60 s in all.
pub(super) fn journey_parts(
    steps: &[(rustnvr_core::journey::JourneyStep, String)],
) -> Vec<JourneyPart> {
    let times: Vec<(i64, Option<i64>)> = steps
        .iter()
        .map(|(s, _)| {
            (
                s.start_ms,
                Some(s.end_ms.unwrap_or(s.start_ms + MIN_AFTER_MS)),
            )
        })
        .collect();
    rustnvr_api::journeys::video_periods(
        &times,
        BEFORE_MS,
        BEFORE_MS,
        JOURNEY_PART_MAX_MS,
        JOURNEY_MAX_MS,
    )
    .into_iter()
    .map(|(i, from_ms, to_ms)| JourneyPart {
        camera: steps[i].0.camera.clone(),
        name: steps[i].1.clone(),
        from_ms,
        to_ms,
    })
    .collect()
}

/// The video of a journey: its cameras one after the other, each with a
/// banner « camera · time », masks burned in, once the recordings are there.
pub(super) async fn journey_clip(
    shared: &Shared,
    id: i64,
    parts: &[JourneyPart],
) -> Result<Bytes, String> {
    if !shared.ffmpeg_ok {
        return Err("FFmpeg indisponible".to_owned());
    }
    let db = shared
        .database
        .as_ref()
        .ok_or("base de données indisponible")?;
    if parts.is_empty() {
        return Err("trajet vide".to_owned());
    }
    let deadline = tokio::time::Instant::now() + GIVE_UP;
    let mut inputs = Vec::new();
    for p in parts {
        let cut = cut(shared, db, p.camera.as_str(), p.from_ms, p.to_ms, deadline).await?;
        let masks = super::detection::camera_settings(shared, &p.camera)
            .await
            .mask_rects();
        inputs.push(JourneyInput {
            cut,
            masks: rustnvr_motion::mask_filters(&masks),
            banner: format!("{} · {}", p.name, clock(p.from_ms)),
        });
    }
    let _permit = ONE_AT_A_TIME.acquire().await.map_err(|e| e.to_string())?;
    let work = shared
        .data_dir
        .join("clips")
        .join(format!("{}-journey-{id}", std::process::id()));
    tokio::fs::create_dir_all(&work)
        .await
        .map_err(|e| e.to_string())?;
    let out = work.join("journey.mp4");
    // With the banners, then without if FFmpeg has no font to write them.
    let mut result = convert_journey(&shared.ffmpeg, &work, &inputs, &out, true).await;
    if let Err(e) = &result {
        debug!(journey = id, error = %e, "journey video made again without banners");
        result = convert_journey(&shared.ffmpeg, &work, &inputs, &out, false).await;
    }
    let read = match result {
        Ok(()) => tokio::fs::read(&out).await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    let _ = tokio::fs::remove_dir_all(&work).await;
    let mp4 = read?;
    if mp4.len() > MAX_CLIP_BYTES {
        return Err(format!("vidéo trop lourde ({} Mo)", mp4.len() / 1_000_000));
    }
    debug!(journey = id, bytes = mp4.len(), "journey video made");
    Ok(Bytes::from(mp4))
}

/// One camera of a journey video, ready to convert.
struct JourneyInput {
    cut: Cut,
    masks: String,
    banner: String,
}

/// The filter graph of a journey video of notifications.
fn journey_graph(inputs: &[(&str, i64, i64, bool)], banners: bool) -> String {
    rustnvr_api::journeys::video_graph(inputs, JOURNEY_SIZE, 15, banners)
}

async fn convert_journey(
    ffmpeg: &str,
    work: &Path,
    inputs: &[JourneyInput],
    out: &Path,
    banners: bool,
) -> Result<(), String> {
    let mut cmd = Command::new(ffmpeg);
    // In the work folder: the banners are read from `t<i>.txt` there.
    cmd.current_dir(work)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]);
    for (i, input) in inputs.iter().enumerate() {
        let list: String = std::iter::once("ffconcat version 1.0\n".to_owned())
            .chain(
                input
                    .cut
                    .files
                    .iter()
                    .map(|f| format!("file {}\n", quote(f))),
            )
            .collect();
        let list_path = work.join(format!("l{i}.txt"));
        tokio::fs::write(&list_path, list)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(work.join(format!("t{i}.txt")), &input.banner)
            .await
            .map_err(|e| e.to_string())?;
        cmd.args(["-f", "concat", "-safe", "0", "-i"])
            .arg(&list_path);
    }
    // The concat demuxer takes its streams from the first file of a list.
    let firsts: Vec<Option<PathBuf>> = inputs
        .iter()
        .map(|i| i.cut.files.first().cloned())
        .collect();
    let audio = tokio::task::spawn_blocking(move || {
        firsts
            .iter()
            .map(|f| f.as_deref().is_some_and(rustnvr_recorder::scan::has_audio))
            .collect::<Vec<bool>>()
    })
    .await
    .map_err(|e| e.to_string())?;
    let parts: Vec<(&str, i64, i64, bool)> = inputs
        .iter()
        .zip(&audio)
        .map(|(i, a)| (i.masks.as_str(), i.cut.offset_ms, i.cut.length_ms, *a))
        .collect();
    cmd.args(["-filter_complex", &journey_graph(&parts, banners)])
        .args(["-map", "[out]"]);
    if audio.contains(&true) {
        cmd.args(["-map", "[aout]", "-c:a", "aac", "-b:a", "64k"]);
    } else {
        cmd.arg("-an");
    }
    cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "30"])
        .args(["-movflags", "+faststart", "-f", "mp4"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(CONVERT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "conversion trop longue".to_owned())?
        .map_err(|e| format!("FFmpeg n'a pas pu être lancé : {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(detail
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("échec de FFmpeg")
        .chars()
        .take(160)
        .collect())
}

/// `'…'` quoting for an FFmpeg concat list.
pub(super) fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// The video filters of a clip: masks, then the height.
fn filters(masks: &str) -> String {
    let scale = format!("scale=-2:'min({HEIGHT},ih)'");
    if masks.is_empty() {
        scale
    } else {
        format!("{masks},{scale}")
    }
}

async fn convert(
    ffmpeg: &str,
    files: &[PathBuf],
    list_path: &Path,
    out_path: &Path,
    offset_ms: i64,
    length_ms: i64,
    masks: &str,
) -> Result<(), String> {
    let list: String = std::iter::once("ffconcat version 1.0\n".to_owned())
        .chain(files.iter().map(|f| format!("file {}\n", quote(f))))
        .collect();
    tokio::fs::write(list_path, list)
        .await
        .map_err(|e| e.to_string())?;
    let seconds = |ms: i64| format!("{}.{:03}", ms / 1000, ms % 1000);
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
        .args(["-f", "concat", "-safe", "0", "-i"])
        .arg(list_path)
        .args(["-ss", &seconds(offset_ms), "-t", &seconds(length_ms)])
        .args(["-map", "0:v:0", "-map", "0:a?", "-vf", &filters(masks)])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "28"])
        .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "64k"])
        .args(["-movflags", "+faststart", "-f", "mp4"])
        .arg(out_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(CONVERT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "conversion trop longue".to_owned())?
        .map_err(|e| format!("FFmpeg n'a pas pu être lancé : {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(detail
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("échec de FFmpeg")
        .chars()
        .take(160)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(start_ms: i64, end_ms: Option<i64>) -> DetectionEvent {
        DetectionEvent {
            id: 1,
            camera: rustnvr_core::CameraId::new("jardin").expect("id"),
            kind: rustnvr_core::EventKind::Motion,
            start_ms,
            end_ms,
            score: 0.1,
            zones: Vec::new(),
            has_thumbnail: true,
            objects: Vec::new(),
            journey: None,
        }
    }

    #[test]
    fn the_clip_covers_the_start_of_the_event() {
        assert_eq!(
            clip_period(&event(100_000, Some(108_000))),
            (98_000, 108_000)
        );
        // A short event: 5 s at least; a long one: 13 s at most.
        assert_eq!(
            clip_period(&event(100_000, Some(101_000))),
            (98_000, 105_000)
        );
        assert_eq!(
            clip_period(&event(100_000, Some(400_000))),
            (98_000, 113_000)
        );
        assert_eq!(clip_period(&event(100_000, None)), (98_000, 105_000));
        assert_eq!(filters(""), "scale=-2:'min(480,ih)'");
        assert!(filters("drawbox=x=0").starts_with("drawbox=x=0,scale"));
    }

    fn segment(destination: &str, start_ms: i64, end_ms: i64) -> SegmentRecord {
        SegmentRecord {
            destination: destination.into(),
            camera: "jardin".into(),
            day: "2026-11-13".into(),
            file: format!("{start_ms}.mp4"),
            start_ms,
            end_ms,
            bytes: 1,
        }
    }

    /// Two short H.264 segments made by FFmpeg (skipped without it), cut
    /// across them into a masked H.264 clip of 480 lines at most.
    #[tokio::test]
    async fn converts_segments_into_a_small_h264_clip() {
        let dir = tempfile::tempdir().expect("tmp");
        let mut segments = Vec::new();
        for i in 0..2 {
            let path = dir.path().join(format!("seg{i}.mp4"));
            let made = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
                .args(["-i", "testsrc=size=1280x720:rate=10", "-t", "3"])
                .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
                .arg(&path)
                .status();
            if !made.is_ok_and(|s| s.success()) {
                eprintln!("FFmpeg with libx264 missing: skipped");
                return;
            }
            segments.push(path);
        }
        let out = dir.path().join("clip.mp4");
        convert(
            "ffmpeg",
            &segments,
            &dir.path().join("list.txt"),
            &out,
            1_500,
            3_000,
            &rustnvr_motion::mask_filters(&[[0.0, 0.0, 0.5, 0.5]]),
        )
        .await
        .expect("clip");
        let size = std::fs::metadata(&out).expect("clip file").len();
        assert!(size > 1_000, "{size} bytes");
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0"])
            .args(["-show_entries", "stream=codec_name,height:format=duration"])
            .args(["-of", "default=noprint_wrappers=1"])
            .arg(&out)
            .output();
        if let Some(probe) = probe.ok().filter(|p| p.status.success()) {
            let text = String::from_utf8_lossy(&probe.stdout);
            assert!(text.contains("codec_name=h264"), "{text}");
            assert!(text.contains("height=480"), "{text}");
            let duration: f64 = text
                .lines()
                .find_map(|l| l.strip_prefix("duration="))
                .and_then(|d| d.parse().ok())
                .expect("duration");
            assert!((2.5..=3.5).contains(&duration), "{duration}");
        }
    }

    fn step(
        camera: &str,
        start_ms: i64,
        end_ms: Option<i64>,
    ) -> (rustnvr_core::journey::JourneyStep, String) {
        (
            rustnvr_core::journey::JourneyStep {
                event: start_ms,
                camera: rustnvr_core::CameraId::new(camera).expect("id"),
                start_ms,
                end_ms,
            },
            camera.to_uppercase(),
        )
    }

    #[test]
    fn a_journey_video_follows_its_cameras() {
        let parts = journey_parts(&[
            step("sonnette", 10_000, Some(14_000)),
            step("entree", 18_000, Some(30_000)),
            step("salon", 26_000, Some(31_000)),
        ]);
        let periods: Vec<(&str, i64, i64)> = parts
            .iter()
            .map(|p| (p.camera.as_str(), p.from_ms, p.to_ms))
            .collect();
        // 2 s before; the next camera as soon as it sees it; 2 s after.
        assert_eq!(
            periods,
            [
                ("sonnette", 8_000, 18_000),
                ("entree", 18_000, 26_000),
                ("salon", 26_000, 33_000)
            ]
        );
        assert_eq!(parts[0].name, "SONNETTE");
        // 20 s at most per camera, 60 s in all.
        let long: Vec<_> = (0..5)
            .map(|i| step("cam", i * 100_000, Some(i * 100_000 + 90_000)))
            .collect();
        let parts = journey_parts(&long);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.to_ms - p.from_ms == 20_000));
        let g = journey_graph(
            &[("drawbox=x=0", 1_500, 8_000, false), ("", 0, 2_000, false)],
            true,
        );
        assert!(
            g.starts_with(
                "[0:v:0]trim=start=1.500:duration=8.000,setpts=PTS-STARTPTS,drawbox=x=0,scale=960:540"
            ),
            "{g}"
        );
        assert!(g.contains("textfile=t1.txt"), "{g}");
        assert!(g.ends_with("[v0][v1]concat=n=2:v=1:a=0[out]"), "{g}");
        assert!(!journey_graph(&[("", 0, 1_000, false)], false).contains("drawtext"));
    }

    /// Two cameras of different sizes made by FFmpeg (skipped without it),
    /// joined into one 960×540 video with their banners; the first one has
    /// sound, the second none: the video has sound all along.
    #[tokio::test]
    async fn joins_two_cameras_into_a_journey_video() {
        let dir = tempfile::tempdir().expect("tmp");
        let mut inputs = Vec::new();
        for (i, size) in ["1280x720", "640x480"].iter().enumerate() {
            let path = dir.path().join(format!("cam{i}.mp4"));
            let mut make = std::process::Command::new("ffmpeg");
            make.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
                .args(["-i", &format!("testsrc=size={size}:rate=10")]);
            if i == 0 {
                make.args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=16000"])
                    .args(["-c:a", "aac"]);
            }
            let made = make
                .args(["-t", "4", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
                .args(["-movflags", "frag_keyframe+empty_moov"])
                .arg(&path)
                .status();
            if !made.is_ok_and(|s| s.success()) {
                eprintln!("FFmpeg with libx264 missing: skipped");
                return;
            }
            inputs.push(JourneyInput {
                cut: Cut {
                    files: vec![path],
                    offset_ms: 500,
                    length_ms: 2_000,
                },
                masks: if i == 0 {
                    rustnvr_motion::mask_filters(&[[0.0, 0.0, 0.5, 0.5]])
                } else {
                    String::new()
                },
                banner: format!("Caméra {i} · 12:00:0{i}"),
            });
        }
        let out = dir.path().join("journey.mp4");
        let with = convert_journey("ffmpeg", dir.path(), &inputs, &out, true).await;
        if let Err(e) = &with {
            eprintln!("no banners ({e}), made without them");
            convert_journey("ffmpeg", dir.path(), &inputs, &out, false)
                .await
                .expect("journey video");
        }
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0"])
            .args([
                "-show_entries",
                "stream=codec_name,width,height:format=duration",
            ])
            .args(["-of", "default=noprint_wrappers=1"])
            .arg(&out)
            .output();
        if let Some(probe) = probe.ok().filter(|p| p.status.success()) {
            let text = String::from_utf8_lossy(&probe.stdout);
            assert!(text.contains("codec_name=h264"), "{text}");
            assert!(
                text.contains("width=960") && text.contains("height=540"),
                "{text}"
            );
            let duration: f64 = text
                .lines()
                .find_map(|l| l.strip_prefix("duration="))
                .and_then(|d| d.parse().ok())
                .expect("duration");
            assert!((3.5..=4.5).contains(&duration), "{duration}");
            let sound = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-select_streams", "a:0"])
                .args(["-show_entries", "stream=codec_name,duration"])
                .args(["-of", "default=noprint_wrappers=1"])
                .arg(&out)
                .output()
                .expect("ffprobe");
            let text = String::from_utf8_lossy(&sound.stdout);
            assert!(text.contains("codec_name=aac"), "{text}");
            let duration: f64 = text
                .lines()
                .find_map(|l| l.strip_prefix("duration="))
                .and_then(|d| d.parse().ok())
                .expect("sound duration");
            assert!((3.5..=4.5).contains(&duration), "sound {duration}");
        }
        assert!(with.is_ok(), "banners: {with:?}");
    }

    #[test]
    fn one_destination_per_moment() {
        let list = chain(vec![
            segment("local", 0, 60_000),
            segment("nas", 0, 60_000),
            segment("local", 60_000, 120_000),
        ]);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].start_ms, 60_000);
    }
}
