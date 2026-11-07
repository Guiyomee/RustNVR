//! A USB camera's path, without a webcam: FFmpeg encodes a test pattern
//! (standing for the webcam) to H.264 Annex B, the device supervisor turns
//! it into frames, the recorder writes MP4 segments; FFmpeg must decode
//! every one (when ffprobe and libx264 are installed, skipped otherwise).

use rustnvr_core::config::Config;
use rustnvr_core::events::event_bus;
use rustnvr_core::{CameraId, NvrEvent, SegmentInfo};
use rustnvr_recorder::{FsStore, RecorderCore, RecorderSettings, spawn_recorder};
use rustnvr_stream::{
    BackoffConfig, DeviceCommand, DeviceConfig, FrameFanout, annexb_output_args,
    spawn_device_supervisor,
};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn tools_available() -> bool {
    let x264 = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("libx264"));
    x264 && Command::new("ffprobe").arg("-version").output().is_ok()
}

/// Codec, size and decoded pictures of a segment.
fn decode(path: &std::path::Path) -> (String, String, u64) {
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
        .expect("ffprobe");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stderr.trim().is_empty(),
        "ffprobe errors for {}: {stderr}",
        path.display()
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let field = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or_default()
            .to_owned()
    };
    (
        field("codec_name"),
        format!("{}x{}", field("width"), field("height")),
        field("nb_read_frames").parse().unwrap_or(0),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_webcam_stream_is_recorded_in_playable_segments() {
    if !tools_available() {
        eprintln!("FFmpeg with libx264 or ffprobe not found: test skipped");
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    // The recording settings of the application, with short segments.
    let config = Config::from_toml_with_env(
        "[recording]\nsegment_seconds = 5\nfragment_millis = 500\n",
        |_| None,
    )
    .expect("config");
    let camera = CameraId::new("webcam").expect("id");
    // What the webcam command gives, from a test pattern at 15 pictures per
    // second (key frame every 2 s, as for webcams).
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-re",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=640x360:rate=15",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-tune",
        "zerolatency",
        "-pix_fmt",
        "yuv420p",
        "-bf",
        "0",
        "-g",
        "30",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.extend(annexb_output_args());
    let events = event_bus(256);
    let mut rx = events.subscribe();
    let mut fanout = FrameFanout::new();
    let (frames, _) = fanout.subscribe("recorder", 256);
    let recorder = spawn_recorder(
        RecorderCore::new(
            camera.clone(),
            RecorderSettings::from(&config.recording),
            Box::new(FsStore::new(dir.path())),
        ),
        frames,
        events.clone(),
        None,
    )
    .expect("recorder");
    let cancel = CancellationToken::new();
    let stream = spawn_device_supervisor(
        DeviceConfig {
            camera,
            label: "test pattern".into(),
            ffmpeg: "ffmpeg".into(),
            command: Arc::new(move || {
                let args = args.clone();
                Box::pin(async move {
                    DeviceCommand {
                        args,
                        env: Vec::new(),
                    }
                })
            }),
            stall_timeout: Duration::from_secs(5),
            backoff: BackoffConfig {
                initial: Duration::from_millis(200),
                max: Duration::from_secs(1),
                stable_after: Duration::from_secs(30),
            },
        },
        fanout,
        events.clone(),
        cancel.clone(),
    );
    let mut segments: Vec<SegmentInfo> = Vec::new();
    tokio::time::timeout(Duration::from_secs(40), async {
        while segments.len() < 2 {
            if let Ok(NvrEvent::SegmentFinished(s)) = rx.recv().await {
                segments.push(s);
            }
        }
    })
    .await
    .expect("segments in time");
    cancel.cancel();
    stream.task.await.expect("stream stopped");
    assert!(
        recorder.wait(Duration::from_secs(10)).await,
        "recorder stopped"
    );

    for s in &segments {
        let (codec, size, pictures) = decode(&s.path);
        assert_eq!((codec.as_str(), size.as_str()), ("h264", "640x360"));
        // About 5 s at 15 pictures per second.
        assert!(
            (60..=90).contains(&pictures),
            "{pictures} pictures in {}",
            s.path.display()
        );
    }
}
