//! Live WebSocket over a real TCP connection.
#![allow(clippy::unwrap_used, missing_docs)]

use bytes::Bytes;
use futures::StreamExt as _;
use rustnvr_api::{ApiServer, ApiState, CameraSource, LiveSettings};
use rustnvr_core::events::event_bus;
use rustnvr_core::{CameraId, VideoCodec, VideoFrame, VideoParams};
use rustnvr_stream::FrameFanout;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

fn params() -> Arc<VideoParams> {
    let mut entry = vec![0, 0, 0, 16];
    entry.extend_from_slice(b"avc1");
    entry.extend_from_slice(&[0; 8]);
    Arc::new(VideoParams {
        codec: VideoCodec::H264,
        rfc6381_codec: "avc1.42C01E".into(),
        width: 320,
        height: 240,
        clock_rate: 90_000,
        sample_entry: Bytes::from(entry),
    })
}

fn frame(session: u64, seq: u64, params: &Arc<VideoParams>) -> VideoFrame {
    VideoFrame {
        session,
        seq,
        params: params.clone(),
        pts: seq as i64 * 9000,
        is_key: seq.is_multiple_of(10),
        wall_time: SystemTime::now(),
        loss: 0,
        audio: None,
        data: Bytes::from(vec![0xab; 50]),
    }
}

async fn start(live: bool) -> (ApiServer, FrameFanout) {
    let mut fanout = FrameFanout::new();
    let tap = fanout.live_tap();
    let cameras = vec![CameraSource {
        id: CameraId::new("cam").unwrap(),
        name: None,
        enabled: true,
        paused: false,
        night: None,
        recording_enabled: false,
        has_sub_stream: false,
        endpoint: "rtsp://x:554".into(),
        stream: None,
        recorders: Vec::new(),
        record_destinations: Vec::new(),
        live: live.then_some(tap),
        sub_stream: None,
        live_sub: None,
        edit: None,
        motion_enabled: false,
        record_audio: true,
        privacy_masks: Vec::new(),
        ptz: false,
        record_mode: Default::default(),
        retention: Default::default(),
        motion: None,
        motion_off_schedule: false,
    }];
    let state = ApiState::new(
        cameras,
        std::env::temp_dir(),
        event_bus(8),
        None,
        LiveSettings::default(),
    );
    let server = ApiServer::start("127.0.0.1:0".parse().unwrap(), state, "/nonexistent".into())
        .await
        .unwrap();
    (server, fanout)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(ws: &mut Ws) -> Message {
    timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

fn fourcc(m: &Message) -> [u8; 4] {
    let Message::Binary(b) = m else {
        panic!("expected binary, got {m:?}");
    };
    b[4..8].try_into().unwrap()
}

#[tokio::test]
async fn streams_fmp4_from_the_last_key_frame() {
    let (server, mut fanout) = start(true).await;
    let p = params();
    // Frames published before the viewer joins: it must start at key frame 10.
    for seq in 0..15 {
        fanout.send(&frame(1, seq, &p));
    }
    let url = format!("ws://{}/api/v1/cameras/cam/live", server.local_addr());
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    let Message::Text(init) = next(&mut ws).await else {
        panic!("expected init message");
    };
    let init: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(init["type"], "init");
    assert_eq!(init["codec"], "avc1.42C01E");
    assert_eq!(fourcc(&next(&mut ws).await), *b"ftyp");
    // Frames 10..=13 are sent immediately (14 waits for its successor).
    for _ in 10..14 {
        assert_eq!(fourcc(&next(&mut ws).await), *b"moof");
    }
    fanout.send(&frame(1, 15, &p));
    assert_eq!(fourcc(&next(&mut ws).await), *b"moof");

    // A new session (camera reconnected) closes the socket for a fresh start.
    fanout.send(&frame(2, 0, &p));
    let Message::Close(Some(close)) = next(&mut ws).await else {
        panic!("expected close");
    };
    assert_eq!(close.reason.as_str(), "new session");
    server.shutdown().await;
}

#[tokio::test]
async fn rejects_cameras_without_live_frames() {
    let (server, _fanout) = start(false).await;
    for path in ["cam", "unknown"] {
        let url = format!("ws://{}/api/v1/cameras/{path}/live", server.local_addr());
        let err = tokio_tungstenite::connect_async(url).await.unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }
    server.shutdown().await;
}

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Serves a real H.265 stream (mock RTSP camera) through a live tap.
async fn h265_source(settings: LiveSettings) -> (ApiServer, tokio::task::JoinHandle<()>) {
    use rustnvr_stream::{RtspSession, SessionOptions};
    use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig};
    let mut cfg = MockServerConfig::new(Fixture::h265());
    cfg.frame_interval = Duration::from_millis(100); // real time, 10 fps
    let camera = MockRtspServer::start(cfg).await.unwrap();
    let mut fanout = FrameFanout::new();
    let tap = fanout.live_tap();
    let target = rustnvr_core::RtspTarget::parse(&camera.url(), None, None).unwrap();
    let feeder = tokio::spawn(async move {
        let _camera = camera;
        let mut session = RtspSession::connect(&target, &SessionOptions::default(), 1)
            .await
            .unwrap();
        while let Ok(frame) = session.next_frame().await {
            fanout.send(&frame);
        }
    });
    let cameras = vec![CameraSource {
        id: CameraId::new("cam").unwrap(),
        name: None,
        enabled: true,
        paused: false,
        night: None,
        recording_enabled: false,
        has_sub_stream: false,
        endpoint: "rtsp://x:554".into(),
        stream: None,
        recorders: Vec::new(),
        record_destinations: Vec::new(),
        live: Some(tap),
        sub_stream: None,
        live_sub: None,
        edit: None,
        motion_enabled: false,
        record_audio: true,
        privacy_masks: Vec::new(),
        ptz: false,
        record_mode: Default::default(),
        retention: Default::default(),
        motion: None,
        motion_off_schedule: false,
    }];
    let state = ApiState::new(cameras, std::env::temp_dir(), event_bus(8), None, settings);
    let server = ApiServer::start("127.0.0.1:0".parse().unwrap(), state, "/nonexistent".into())
        .await
        .unwrap();
    (server, feeder)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn converts_h265_to_h264_on_request() {
    if !ffmpeg_available() {
        eprintln!("ffmpeg not found: skipping conversion test");
        return;
    }
    let (server, feeder) = h265_source(LiveSettings::default()).await;
    // Let the source start so that the conversion has a key frame to begin with.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let url = format!(
        "ws://{}/api/v1/cameras/cam/live?transcode=h264",
        server.local_addr()
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let started = std::time::Instant::now();
    let Message::Text(init) = next(&mut ws).await else {
        panic!("expected init message");
    };
    let first_frame_ms = started.elapsed().as_millis();
    let init: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert!(
        init["codec"].as_str().unwrap().starts_with("avc1."),
        "{init}"
    );
    assert_eq!(
        (init["width"].as_u64(), init["height"].as_u64()),
        (Some(320), Some(240))
    );

    // Collect the converted stream and check that FFmpeg decodes it.
    let mut file = Vec::new();
    let Message::Binary(b) = next(&mut ws).await else {
        panic!("expected init segment")
    };
    assert_eq!(&b[4..8], b"ftyp");
    file.extend_from_slice(&b);
    for _ in 0..20 {
        let m = next(&mut ws).await;
        assert_eq!(fourcc(&m), *b"moof");
        let Message::Binary(b) = m else {
            unreachable!()
        };
        file.extend_from_slice(&b);
    }
    let path = std::env::temp_dir().join(format!("rustnvr-transcode-{}.mp4", std::process::id()));
    std::fs::write(&path, &file).unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_entries",
            "stream=codec_name,nb_read_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(&path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(
        probe.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "h264,20");
    eprintln!("first converted frame after {first_frame_ms} ms");
    feeder.abort();
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_missing_ffmpeg_to_the_viewer() {
    let settings = LiveSettings {
        ffmpeg: "/nonexistent/ffmpeg".into(),
        ..LiveSettings::default()
    };
    let (server, feeder) = h265_source(settings).await;
    let url = format!(
        "ws://{}/api/v1/cameras/cam/live?transcode=h264",
        server.local_addr()
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let Message::Close(Some(close)) = next(&mut ws).await else {
        panic!("expected close");
    };
    assert!(
        close
            .reason
            .as_str()
            .starts_with("transcode: cannot start FFmpeg"),
        "{}",
        close.reason
    );
    feeder.abort();
    server.shutdown().await;
}
