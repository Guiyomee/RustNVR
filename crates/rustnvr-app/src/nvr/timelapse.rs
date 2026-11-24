//! Daily time-lapses: every night, for the cameras whose detection settings
//! ask for it, a short video of the previous day made from the recordings —
//! the key frames only (little decoding), one picture a minute, privacy
//! masks burned in, 720p H.264 — and stored next to the recordings in
//! `<root>/.timelapse/<camera>/<day>.mp4` (see `rustnvr_api::timelapse`).
//! One camera at a time, at a low priority: recording comes first.

use super::Shared;
use rustnvr_api::timelapse::{DIR, Job};
use rustnvr_core::CameraId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// First look after startup, then every…
const FIRST_LOOK: Duration = Duration::from_secs(10 * 60);
const LOOK_EVERY: Duration = Duration::from_secs(30 * 60);
/// One picture kept per this many seconds of recordings.
const PICTURE_EVERY_S: u32 = 60;
/// Pictures per second of the video: a day lasts about 48 s.
const FPS: u32 = 30;
/// Height of the video, at most.
const HEIGHT: u32 = 720;
/// Time-lapses are kept this many days.
const KEEP_DAYS: i64 = 30;
/// A day that failed is tried again this many times at most.
const ATTEMPTS: u32 = 2;
/// A conversion taking longer is abandoned.
const CONVERT_TIMEOUT: Duration = Duration::from_secs(3 * 3600);

/// One time-lapse at a time (the nightly ones and those asked from the API).
static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Start and end (ms since the Unix epoch) of a local day.
fn day_bounds(day: jiff::civil::Date) -> Option<(i64, i64)> {
    let tz = jiff::tz::TimeZone::system();
    let start = day.to_zoned(tz.clone()).ok()?.timestamp().as_millisecond();
    let end = day
        .tomorrow()
        .ok()?
        .to_zoned(tz)
        .ok()?
        .timestamp()
        .as_millisecond();
    Some((start, end))
}

/// The local day before today.
fn yesterday() -> Option<jiff::civil::Date> {
    jiff::Zoned::now().date().yesterday().ok()
}

/// Storage destinations with a folder of their own: `(id, root)`.
fn roots(shared: &Shared) -> Vec<(String, PathBuf)> {
    shared
        .api
        .get()
        .map(|a| {
            a.destinations()
                .into_iter()
                .filter(|d| d.remote.is_none())
                .map(|d| (d.id, d.root))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a time-lapse of `camera` for `day` exists on some destination.
fn exists(roots: &[(String, PathBuf)], camera: &str, day: &str) -> bool {
    roots.iter().any(|(_, root)| {
        root.join(DIR)
            .join(camera)
            .join(format!("{day}.mp4"))
            .is_file()
    })
}

/// Tells the API where the time-lapse of `camera` for `day` is (`None`:
/// done), for the list of the interface.
pub(super) fn report(shared: &Shared, camera: &CameraId, day: &str, job: Option<Job>) {
    if let Some(api) = shared.api.get() {
        api.set_timelapse_job(camera.as_str(), day, job);
    }
}

/// Makes the time-lapse of `camera` for the local `day`, replacing an
/// existing one; its path. Where it is goes to the API while it is made, and
/// why it failed if it does.
pub(super) async fn make(
    shared: &Shared,
    camera: &CameraId,
    day: jiff::civil::Date,
) -> Result<PathBuf, String> {
    let name = day.to_string();
    report(shared, camera, &name, Some(Job::Waiting));
    let key = (camera.to_string(), name.clone());
    let token = CancellationToken::new();
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    running().insert(key.clone(), (id, token.clone()));
    // Stopped: the future of `make_it` is dropped, and FFmpeg killed with it.
    let result = tokio::select! {
        r = make_it(shared, camera, day) => r,
        () = token.cancelled() => Err(STOPPED.to_owned()),
    };
    {
        let mut running = running();
        if running.get(&key).is_some_and(|(i, _)| *i == id) {
            running.remove(&key);
        }
    }
    match &result {
        Ok(_) => report(shared, camera, &name, None),
        Err(e) if e == STOPPED => {
            report(shared, camera, &name, None);
            // What it left behind, wherever the day was being made.
            for (_, root) in roots(shared) {
                let dir = root.join(DIR).join(camera.as_str());
                let _ = tokio::fs::remove_file(dir.join(format!("{name}.mp4.part"))).await;
                let _ = tokio::fs::remove_file(dir.join(format!("{name}.txt"))).await;
            }
            info!(camera = %camera, day = %name, "time-lapse stopped");
        }
        Err(e) => report(shared, camera, &name, Some(Job::Failed(e.clone()))),
    }
    result
}

/// Error of a time-lapse stopped from the interface.
pub(super) const STOPPED: &str = "arrêté";

/// Time-lapses waiting or being made: `(camera, day)` → their stop.
type Running = HashMap<(String, String), (u64, CancellationToken)>;
static RUNNING: std::sync::LazyLock<std::sync::Mutex<Running>> =
    std::sync::LazyLock::new(Default::default);
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

fn running() -> std::sync::MutexGuard<'static, Running> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stops the time-lapse of `camera` for `day` if it is waiting or being
/// made; whether one was.
pub(super) fn stop(camera: &str, day: &str) -> bool {
    match running().remove(&(camera.to_owned(), day.to_owned())) {
        Some((_, token)) => {
            token.cancel();
            true
        }
        None => false,
    }
}

async fn make_it(
    shared: &Shared,
    camera: &CameraId,
    day: jiff::civil::Date,
) -> Result<PathBuf, String> {
    if !shared.ffmpeg_ok {
        return Err("FFmpeg indisponible".to_owned());
    }
    let db = shared
        .database
        .as_ref()
        .ok_or("base de données indisponible")?;
    let (from, to) = day_bounds(day).ok_or("jour invalide")?;
    let segments = super::clips::chain(
        db.segments(camera.as_str(), from, to)
            .await
            .map_err(|e| e.to_string())?,
    );
    let roots = roots(shared);
    let files: Vec<(PathBuf, PathBuf)> = segments
        .iter()
        .filter_map(|s| {
            let root = &roots.iter().find(|(d, _)| *d == s.destination)?.1;
            Some((
                root.clone(),
                root.join(&s.camera).join(&s.day).join(&s.file),
            ))
        })
        .collect();
    // Stored where the day was recorded (its first segment).
    let Some((root, _)) = files.first() else {
        return Err("aucun enregistrement ce jour-là".to_owned());
    };
    let dir = root.join(DIR).join(camera.as_str());
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let name = day.to_string();
    let list_path = dir.join(format!("{name}.txt"));
    let part = dir.join(format!("{name}.mp4.part"));
    let out = dir.join(format!("{name}.mp4"));
    let masks = super::detection::camera_settings(shared, camera)
        .await
        .mask_rects();
    // FFmpeg reads every file of the day: how far it is, from what it read.
    let total_bytes: u64 = segments.iter().map(|s| s.bytes).sum();
    let _permit = ONE_AT_A_TIME.acquire().await.map_err(|e| e.to_string())?;
    report(shared, camera, &name, Some(Job::Making(0)));
    let list: String = std::iter::once("ffconcat version 1.0\n".to_owned())
        .chain(
            files
                .iter()
                .map(|(_, f)| format!("file {}\n", super::clips::quote(f))),
        )
        .collect();
    tokio::fs::write(&list_path, list)
        .await
        .map_err(|e| e.to_string())?;
    let mut last = 0;
    let result = convert(
        &shared.ffmpeg,
        &list_path,
        &part,
        &rustnvr_motion::mask_filters(&masks),
        |read| {
            let percent = percent(read, total_bytes);
            if percent != last {
                last = percent;
                report(shared, camera, &name, Some(Job::Making(percent)));
            }
        },
    )
    .await;
    let _ = tokio::fs::remove_file(&list_path).await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, &out)
        .await
        .map_err(|e| e.to_string())?;
    info!(camera = %camera, day = %name, segments = files.len(), "time-lapse made");
    Ok(out)
}

/// The video filters: one picture a minute of recordings (key frames only
/// are decoded), the masks, the height, then those pictures at `FPS`.
fn filters(masks: &str) -> String {
    let mut f = vec![format!("fps=1/{PICTURE_EVERY_S}")];
    if !masks.is_empty() {
        f.push(masks.to_owned());
    }
    f.push(format!("scale=-2:'min({HEIGHT},ih)'"));
    // After `fps`, the time base is a minute: the pictures are numbered in
    // a time base of 1/`FPS` first (`N/FPS/TB` in a one-minute time base
    // rounds every picture to 0, and only 2 or 3 of them were left). The
    // final `fps` states the rate in the graph itself: with `-r` alone,
    // FFmpeg 6.1 (Ubuntu 24.04) kept the pictures a second apart and
    // repeated each one 30 times (1859 pictures instead of 60); 5.1 and 7.1
    // give 60 either way.
    f.push(format!("settb=1/{FPS},setpts=N,fps={FPS}"));
    f.join(",")
}

/// How far the time-lapse is when FFmpeg has read `read` of the `total`
/// bytes of the day: 0 to 99 (100 once it is renamed into place). FFmpeg's
/// own `-progress` report is no help here: the encoder keeps the pictures
/// until the end, so its time stays at 0.
fn percent(read: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    (read.saturating_mul(100) / total).min(99) as u8
}

/// Bytes a process has read (`rchar` of `/proc/<pid>/io`).
fn read_chars(io: &str) -> Option<u64> {
    io.lines()
        .find_map(|l| l.strip_prefix("rchar:"))
        .and_then(|v| v.trim().parse().ok())
}

/// How often the progress of FFmpeg is read.
const PROGRESS_EVERY: Duration = Duration::from_secs(2);

async fn convert(
    ffmpeg: &str,
    list: &Path,
    out: &Path,
    masks: &str,
    mut progress: impl FnMut(u64),
) -> Result<(), String> {
    use tokio::io::AsyncReadExt as _;
    // Below the recording: `nice` when the system has it.
    let mut cmd = if Path::new("/usr/bin/nice").exists() {
        let mut c = Command::new("/usr/bin/nice");
        c.args(["-n", "19"]).arg(ffmpeg);
        c
    } else {
        Command::new(ffmpeg)
    };
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
        .args(["-f", "concat", "-safe", "0", "-skip_frame", "nokey", "-i"])
        .arg(list)
        .args(["-map", "0:v:0", "-an", "-vf", &filters(masks)])
        .args(["-r", &FPS.to_string(), "-threads", "2"])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "28"])
        .args([
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("FFmpeg n'a pas pu être lancé : {e}"))?;
    // `nice` runs FFmpeg in its own process: the same pid.
    let io = child
        .id()
        .map(|pid| PathBuf::from(format!("/proc/{pid}/io")));
    let mut stderr = child.stderr.take();
    // The errors are read on the side, or a full pipe would stop FFmpeg.
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_string(&mut text).await;
        }
        text
    });
    let run = async {
        let mut tick = tokio::time::interval(PROGRESS_EVERY);
        loop {
            tokio::select! {
                status = child.wait() => break status,
                _ = tick.tick() => {
                    if let Some(read) = io
                        .as_ref()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .and_then(|t| read_chars(&t))
                    {
                        progress(read);
                    }
                }
            }
        }
    };
    let status = tokio::time::timeout(CONVERT_TIMEOUT, run)
        .await
        .map_err(|_| "conversion trop longue".to_owned())?
        .map_err(|e| format!("FFmpeg interrompu : {e}"))?;
    let detail = errors.await.unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    Err(detail
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("échec de FFmpeg")
        .chars()
        .take(160)
        .collect())
}

/// Removes the time-lapses older than `KEEP_DAYS`. Blocking.
fn purge(roots: &[(String, PathBuf)]) {
    let Some(limit) = jiff::Zoned::now()
        .date()
        .checked_sub(jiff::Span::new().days(KEEP_DAYS))
        .ok()
        .map(|d| d.to_string())
    else {
        return;
    };
    for (_, root) in roots {
        let Ok(cameras) = std::fs::read_dir(root.join(DIR)) else {
            continue;
        };
        for cam in cameras.flatten() {
            let Ok(files) = std::fs::read_dir(cam.path()) else {
                continue;
            };
            for f in files.flatten() {
                let name = f.file_name();
                let Some(day) = name.to_str().and_then(|n| n.strip_suffix(".mp4")) else {
                    continue;
                };
                // `YYYY-MM-DD` sorts like the dates.
                if rustnvr_api::timelapse::valid_day(day) && *day < *limit {
                    match std::fs::remove_file(f.path()) {
                        Ok(()) => debug!(path = %f.path().display(), "old time-lapse removed"),
                        Err(e) => {
                            warn!(path = %f.path().display(), error = %e, "old time-lapse not removed")
                        }
                    }
                }
            }
        }
    }
}

/// Every half hour: the missing time-lapses of yesterday, then the old ones
/// removed.
pub(super) async fn run(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut failed: HashMap<(CameraId, String), u32> = HashMap::new();
    let mut wait = FIRST_LOOK;
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
        wait = LOOK_EVERY;
        let Some(day) = yesterday() else { continue };
        let name = day.to_string();
        let cameras: Vec<CameraId> = {
            let state = shared.state.lock().await;
            state
                .configs
                .iter()
                .filter(|c| c.enabled && state.motion.get(&c.id).is_some_and(|m| m.timelapse))
                .map(|c| c.id.clone())
                .collect()
        };
        let roots = roots(&shared);
        for camera in cameras {
            if cancel.is_cancelled() {
                return;
            }
            let key = (camera.clone(), name.clone());
            if exists(&roots, camera.as_str(), &name)
                || failed.get(&key).copied().unwrap_or(0) >= ATTEMPTS
            {
                continue;
            }
            match make(&shared, &camera, day).await {
                // Stopped from the interface: not made again tonight.
                Err(e) if e == STOPPED => {
                    failed.insert(key, ATTEMPTS);
                }
                Err(e) => {
                    warn!(camera = %camera, day = %name, error = %e, "time-lapse not made");
                    *failed.entry(key).or_default() += 1;
                }
                Ok(_) => {}
            }
        }
        failed.retain(|(_, d), _| *d == name);
        let _ = tokio::task::spawn_blocking(move || purge(&roots)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_filters_and_the_day() {
        assert_eq!(
            filters(""),
            "fps=1/60,scale=-2:'min(720,ih)',settb=1/30,setpts=N,fps=30"
        );
        assert!(filters("drawbox=x=0").starts_with("fps=1/60,drawbox=x=0,scale"));
        let day = jiff::civil::date(2026, 11, 18);
        let (from, to) = day_bounds(day).expect("bounds");
        // 24 hours, or 23 / 25 on a change of time.
        assert!(
            (23 * 3_600_000..=25 * 3_600_000).contains(&(to - from)),
            "{}",
            to - from
        );
    }

    #[test]
    fn stops_only_what_is_on_its_way() {
        let token = CancellationToken::new();
        running().insert(("garage".into(), "2026-11-17".into()), (99, token.clone()));
        assert!(stop("garage", "2026-11-17"));
        assert!(token.is_cancelled());
        assert!(!stop("garage", "2026-11-17"), "already stopped");
        assert!(!stop("jardin", "2026-11-17"));
    }

    #[test]
    fn reads_how_far_ffmpeg_is() {
        let io = "rchar: 3145271188\nwchar: 1024\nsyscr: 9\nread_bytes: 3338240\n";
        assert_eq!(read_chars(io), Some(3_145_271_188));
        assert_eq!(read_chars("wchar: 1\n"), None);
        // 3 GB read of a day of 12 GB.
        assert_eq!(percent(3_000_000_000, 12_000_000_000), 25);
        assert_eq!(
            percent(13_000_000_000, 12_000_000_000),
            99,
            "100 only once renamed"
        );
        assert_eq!(percent(5, 0), 0);
    }

    /// A real conversion of an hour of recordings in three segments: one
    /// picture per minute, so about 60 pictures (2 s at 30 per second), and
    /// its progress reported (skipped without FFmpeg).
    #[tokio::test]
    async fn makes_one_picture_per_minute_with_its_progress() {
        let dir = tempfile::tempdir().expect("tmp");
        let mut list = String::from("ffconcat version 1.0\n");
        for i in 0..3 {
            let seg = dir.path().join(format!("seg{i}.mp4"));
            // 20 minutes at 1 picture per second, a key frame every 10 s.
            let made = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
                .args(["-i", "testsrc=size=320x240:rate=1", "-t", "1200"])
                .args(["-c:v", "libx264", "-g", "10", "-pix_fmt", "yuv420p"])
                .arg(&seg)
                .status();
            if !made.is_ok_and(|s| s.success()) {
                eprintln!("FFmpeg with libx264 missing: skipped");
                return;
            }
            list.push_str(&format!("file {}\n", super::super::clips::quote(&seg)));
        }
        let list_path = dir.path().join("list.txt");
        std::fs::write(&list_path, list).expect("list");
        let out = dir.path().join("out.mp4");
        let mut seen = Vec::new();
        convert("ffmpeg", &list_path, &out, "", |s| seen.push(s))
            .await
            .expect("time-lapse");
        assert!(!seen.is_empty(), "no progress read");
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-count_frames"])
            .args(["-show_entries", "stream=nb_read_frames:format=duration"])
            .args(["-of", "default=noprint_wrappers=1"])
            .arg(&out)
            .output();
        let Some(probe) = probe.ok().filter(|p| p.status.success()) else {
            eprintln!("ffprobe missing: pictures not counted");
            return;
        };
        let text = String::from_utf8_lossy(&probe.stdout);
        let value = |key: &str| -> f64 {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0.0)
        };
        let frames = value("nb_read_frames=");
        let duration = value("duration=");
        assert!((55.0..=62.0).contains(&frames), "{frames} pictures\n{text}");
        assert!((1.8..=2.2).contains(&duration), "{duration} s\n{text}");
    }

    #[test]
    fn removes_only_old_time_lapses() {
        let root = tempfile::tempdir().expect("tmp");
        let dir = root.path().join(DIR).join("garage");
        std::fs::create_dir_all(&dir).expect("dir");
        let today = jiff::Zoned::now().date();
        let old = today
            .checked_sub(jiff::Span::new().days(KEEP_DAYS + 1))
            .expect("old");
        let recent = today.yesterday().expect("recent");
        for name in [
            format!("{old}.mp4"),
            format!("{recent}.mp4"),
            "notes.txt".to_owned(),
        ] {
            std::fs::write(dir.join(name), b"x").expect("write");
        }
        let roots = vec![("nas".to_owned(), root.path().to_owned())];
        assert!(exists(&roots, "garage", &recent.to_string()));
        purge(&roots);
        assert!(!dir.join(format!("{old}.mp4")).exists());
        assert!(dir.join(format!("{recent}.mp4")).exists());
        assert!(dir.join("notes.txt").exists());
    }
}
