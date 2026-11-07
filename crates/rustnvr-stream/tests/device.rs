//! The device supervisor against a real FFmpeg (a test pattern encoded by
//! libx264 stands for the webcam). Skipped when FFmpeg or libx264 is missing.

use rustnvr_core::events::event_bus;
use rustnvr_core::{CameraId, StreamItem, StreamState, VideoCodec};
use rustnvr_stream::{
    BackoffConfig, DeviceCommand, DeviceConfig, FrameFanout, annexb_output_args,
    spawn_device_supervisor,
};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn ffmpeg_with_x264() -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("libx264"))
}

fn config(args: Vec<String>) -> DeviceConfig {
    DeviceConfig {
        camera: CameraId::new("webcam").expect("id"),
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
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pictures_of_an_ffmpeg_source_become_frames() {
    if !ffmpeg_with_x264() {
        eprintln!("FFmpeg with libx264 not found: test skipped");
        return;
    }
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-re",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=25",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-tune",
        "zerolatency",
        "-pix_fmt",
        "yuv420p",
        "-g",
        "25",
        "-bf",
        "0",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.extend(annexb_output_args());
    let mut fanout = FrameFanout::new();
    let (mut rx, _) = fanout.subscribe("test", 256);
    let cancel = CancellationToken::new();
    let handle = spawn_device_supervisor(config(args), fanout, event_bus(16), cancel.clone());

    let mut frames = Vec::new();
    while frames.len() < 60 {
        let item = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("frames in time")
            .expect("open");
        if let StreamItem::Frame(f) = item {
            frames.push(f);
        }
    }
    assert!(matches!(
        *handle.state.borrow(),
        StreamState::Streaming {
            codec: VideoCodec::H264,
            width: 320,
            height: 240,
            ..
        }
    ));
    let first = &frames[0];
    assert!(first.is_key, "starts on a key frame");
    assert_eq!((first.params.width, first.params.height), (320, 240));
    assert!(first.params.rfc6381_codec.starts_with("avc1."));
    // One key frame per second (-g 25), consecutive numbers, rising times
    // about 40 ms apart.
    let keys = frames.iter().filter(|f| f.is_key).count();
    assert!((2..=4).contains(&keys), "{keys} key frames");
    for w in frames.windows(2) {
        assert_eq!(w[1].seq, w[0].seq + 1);
        assert!(w[1].pts > w[0].pts);
    }
    let span = (frames[59].pts - frames[0].pts) as f64 / 90_000.0;
    assert!((1.5..3.5).contains(&span), "60 pictures in {span} s");
    // Length-prefixed NAL units, as from RTSP.
    let len = u32::from_be_bytes([first.data[0], first.data[1], first.data[2], first.data[3]]);
    assert!(len as usize <= first.data.len() - 4);
    assert!(handle.metrics.snapshot().frames >= 60);
    cancel.cancel();
    handle.task.await.expect("stopped");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_fails_is_retried_with_the_reason() {
    if !ffmpeg_with_x264() {
        eprintln!("FFmpeg with libx264 not found: test skipped");
        return;
    }
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "v4l2",
        "-i",
        "/dev/nonexistent-video",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.extend(annexb_output_args());
    let fanout = FrameFanout::new();
    let cancel = CancellationToken::new();
    let mut handle = spawn_device_supervisor(config(args), fanout, event_bus(16), cancel.clone());
    let error = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            handle.state.changed().await.expect("state");
            if let StreamState::Retrying { error, .. } = &*handle.state.borrow() {
                return error.clone();
            }
        }
    })
    .await
    .expect("retrying");
    assert!(error.contains("FFmpeg"), "{error}");
    assert!(error.contains("nonexistent-video"), "{error}");
    cancel.cancel();
    handle.task.await.expect("stopped");
}
