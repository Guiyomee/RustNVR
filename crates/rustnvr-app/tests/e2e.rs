//! End-to-end: mock camera → RTSP ingestion → recorder → files on disk.
//!
//! Files are checked structurally with the crate's own scanner and, when
//! `ffprobe` is installed, decoded by FFmpeg to prove they are playable.
#![allow(clippy::unwrap_used, missing_docs)]

use rustnvr_app::Nvr;
use rustnvr_core::config::Config;
use rustnvr_core::events::event_bus;
use rustnvr_core::{NvrEvent, SegmentInfo, VideoCodec};
use rustnvr_recorder::scan::scan;
use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn config(url: &str, dir: &Path) -> Config {
    Config::from_toml_with_env(
        &format!(
            r#"
            [storage]
            recordings_dir = "{}"

            [recording]
            segment_seconds = 5
            fragment_millis = 1000

            [reconnect]
            initial_millis = 20
            max_seconds = 1

            [metrics]
            log_interval_seconds = 0

            [web]
            listen = "127.0.0.1:0"
            static_dir = "/nonexistent-rustnvr-web"

            [[cameras]]
            id = "mock"
            main = {{ url = "{url}", stall_timeout_seconds = 2 }}
            "#,
            dir.display()
        ),
        |_| None,
    )
    .unwrap()
}

/// Runs the NVR until `count` segments were finished, then shuts it down and
/// returns every finished segment (including those closed by the shutdown).
async fn record(server_cfg: MockServerConfig, count: usize, dir: &Path) -> Vec<SegmentInfo> {
    let server = MockRtspServer::start(server_cfg).await.unwrap();
    let events = event_bus(1024);
    let mut rx = events.subscribe();
    let nvr = Nvr::start(&config(&server.url(), dir), events)
        .await
        .unwrap();
    let mut segments = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        while segments.len() < count {
            if let NvrEvent::SegmentFinished(s) = rx.recv().await.unwrap() {
                segments.push(s);
            }
        }
    })
    .await
    .expect("segments were not produced in time");
    assert!(nvr.shutdown().await, "unclean shutdown");
    while let Ok(event) = rx.try_recv() {
        if let NvrEvent::SegmentFinished(s) = event {
            segments.push(s);
        }
    }
    server.stop().await;
    segments
}

fn ffprobe_available() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Decodes every frame with FFmpeg; returns (codec name, decoded frame count).
fn ffprobe_decode(path: &Path) -> (String, u64) {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
        .args([
            "-show_entries",
            "stream=codec_name,width,height,nb_read_frames",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stderr.trim().is_empty(),
        "ffprobe errors for {}: {stderr}",
        path.display()
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let field = |k: &str| {
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(field("width"), "320");
    assert_eq!(field("height"), "240");
    (
        field("codec_name"),
        field("nb_read_frames").parse().unwrap(),
    )
}

fn check_segments(segments: &[SegmentInfo], codec: VideoCodec, dir: &Path) {
    let probe = ffprobe_available();
    if !probe {
        eprintln!("ffprobe not found: skipping decode validation");
    }
    for s in segments {
        assert_eq!(s.codec, codec);
        assert!(s.path.starts_with(dir.join("mock")), "{}", s.path.display());
        let report = scan(&mut std::fs::File::open(&s.path).unwrap()).unwrap();
        assert_eq!(report.valid_len, report.file_len);
        assert_eq!(report.samples(), s.frames);
        assert!(report.fragments[0].starts_with_key);
        let expected_entry = match codec {
            VideoCodec::H264 => *b"avc1",
            VideoCodec::H265 => *b"hvc1",
        };
        assert_eq!(report.sample_entry, Some(expected_entry));
        if probe {
            let (name, frames) = ffprobe_decode(&s.path);
            assert_eq!(
                name,
                if codec == VideoCodec::H264 {
                    "h264"
                } else {
                    "hevc"
                }
            );
            assert_eq!(frames, s.frames, "every recorded frame must decode");
        }
    }
    let leftovers: Vec<_> = walk(dir)
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

async fn continuous_recording(fixture: Fixture, codec: VideoCodec) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = MockServerConfig::new(fixture);
    cfg.frame_interval = Duration::from_millis(5); // 20x real time at 10 fps
    let segments = record(cfg, 3, dir.path()).await;
    // Key frames every second and 5 s segments: complete segments have 50 frames.
    for s in &segments[..3] {
        assert_eq!(s.frames, 50, "{s:?}");
        assert_eq!(s.duration, Duration::from_secs(5));
        assert_eq!(s.fragments, 5);
    }
    check_segments(&segments, codec, dir.path());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_h264_without_reencoding() {
    continuous_recording(Fixture::h264(), VideoCodec::H264).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_h265_without_reencoding() {
    continuous_recording(Fixture::h265(), VideoCodec::H265).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recording_survives_camera_disconnections() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.frame_interval = Duration::from_millis(5);
    // Each connection delivers 3.5 s of video and is then cut.
    cfg.disconnect_after_frames = Some(35);
    let segments = record(cfg, 4, dir.path()).await;
    // Every disconnection closes the segment early (35 frames) and recording
    // resumes on the first key frame of the next session.
    assert!(
        segments.iter().filter(|s| s.frames == 35).count() >= 3,
        "{segments:#?}"
    );
    check_segments(&segments, VideoCodec::H264, dir.path());
}

/// A camera sending G.711 µ-law sound (a 440 Hz tone): the sound is recorded
/// as Opus next to the untouched video, and FFmpeg decodes it back to the
/// tone, as long as the video.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_g711_sound_as_opus() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.frame_interval = Duration::from_millis(5);
    cfg.pcmu_audio = true;
    let segments = record(cfg, 2, dir.path()).await;
    check_segments(&segments, VideoCodec::H264, dir.path());
    if !ffprobe_available() {
        eprintln!("ffprobe not found: skipping the sound check");
        return;
    }
    let file = &segments[0].path;
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-count_packets"])
        .args([
            "-show_entries",
            "stream=codec_name,sample_rate,channels,nb_read_packets",
            "-of",
            "default=nw=1",
        ])
        .arg(file)
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(info.contains("codec_name=opus"), "{info}");
    assert!(info.contains("channels=1"), "{info}");
    let packets: u64 = info
        .lines()
        .find_map(|l| l.strip_prefix("nb_read_packets="))
        .unwrap()
        .parse()
        .unwrap();
    // 5 s of sound in 20 ms packets (the first ones may start just after the video).
    assert!((235..=251).contains(&packets), "{packets} packets: {info}");
    // Decoded by FFmpeg: the tone again.
    let pcm = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(file)
        .args(["-map", "0:a", "-f", "s16le", "-ac", "1", "-ar", "8000", "-"])
        .output()
        .unwrap();
    // FFmpeg 8 reports its own Opus MP4 files the same way at their last
    // packet (pre-skip): not a fault of the file.
    let stderr = String::from_utf8_lossy(&pcm.stderr);
    let problems: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.contains("non monotonically increasing dts"))
        .collect();
    assert!(pcm.status.success() && problems.is_empty(), "{stderr}");
    let samples: Vec<i16> = pcm
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    let steady = &samples[1_600..samples.len() - 800];
    let rms =
        (steady.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / steady.len() as f64).sqrt();
    let crossings = steady
        .windows(2)
        .filter(|w| (w[0] < 0) != (w[1] < 0))
        .count();
    let per_second = crossings as f64 * 8_000.0 / steady.len() as f64;
    assert!((4_000.0..7_500.0).contains(&rms), "rms {rms}");
    assert!(
        (860.0..900.0).contains(&per_second),
        "{per_second} crossings per second"
    );
}
