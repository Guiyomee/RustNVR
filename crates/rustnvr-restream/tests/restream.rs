//! Republication end to end: mock camera → supervisor → republication
//! server → RTSP clients (Retina, and FFmpeg when installed).
#![allow(clippy::unwrap_used, missing_docs)]

use futures::FutureExt as _;
use rustnvr_core::events::event_bus;
use rustnvr_core::{CameraId, RtspTarget, Secret, StreamRole, VideoCodec};
use rustnvr_restream::{Credentials, Lookup, Options, RestreamServer, Source};
use rustnvr_stream::{
    BackoffConfig, FrameFanout, LiveTap, RtspSession, SessionOptions, StreamHandle,
    SupervisorConfig, spawn_supervisor,
};
use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig, ServerStats};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

fn opts(audio: bool) -> SessionOptions {
    SessionOptions {
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(3),
        audio,
        ..SessionOptions::default()
    }
}

struct Rig {
    camera: MockRtspServer,
    server: RestreamServer,
    tap: LiveTap,
    cancel: CancellationToken,
    _handle: StreamHandle,
}

impl Rig {
    /// A mock camera read once by a supervisor and republished as `cam`.
    async fn start(config: MockServerConfig, audio: bool, max_clients: usize) -> Self {
        let options = Options {
            max_clients,
            max_clients_per_address: max_clients,
        };
        Self::with(config, audio, options, None).await
    }

    /// Like [`Self::start`]; with `credentials`, every stream (existing or
    /// not) requires them.
    async fn with(
        mut config: MockServerConfig,
        audio: bool,
        options: Options,
        credentials: Option<Credentials>,
    ) -> Self {
        // Real time: clients must not fall behind a camera faster than life.
        config.frame_interval = Duration::from_millis(40);
        let camera = MockRtspServer::start(config).await.unwrap();
        let mut fanout = FrameFanout::new();
        let tap = fanout.live_tap();
        let cancel = CancellationToken::new();
        let handle = spawn_supervisor(
            SupervisorConfig {
                camera: CameraId::new("cam").unwrap(),
                role: StreamRole::Main,
                target: RtspTarget::parse(&camera.url(), None, None).unwrap(),
                session: opts(audio),
                backoff: BackoffConfig {
                    initial: Duration::from_millis(20),
                    max: Duration::from_millis(100),
                    stable_after: Duration::from_secs(60),
                },
            },
            fanout,
            event_bus(64),
            cancel.clone(),
        );
        let source = tap.clone();
        let lookup: Lookup = Arc::new(move |camera: String, sub: bool| {
            let tap = (camera == "cam" && !sub).then(|| source.clone());
            let credentials = credentials.clone();
            async move { Source { tap, credentials } }.boxed()
        });
        let server = RestreamServer::start("127.0.0.1:0".parse().unwrap(), lookup, options)
            .await
            .unwrap();
        Self {
            camera,
            server,
            tap,
            cancel,
            _handle: handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("rtsp://{}/{path}", self.server.local_addr())
    }

    fn target(&self, path: &str) -> RtspTarget {
        RtspTarget::parse(&self.url(path), None, None).unwrap()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn republishes(fixture: Fixture, codec: VideoCodec) {
    let ticks = i64::from(fixture.ticks_per_frame());
    let rig = Rig::start(MockServerConfig::new(fixture), false, 8).await;
    let mut first = RtspSession::connect(&rig.target("cam"), &opts(false), 1)
        .await
        .unwrap();
    let mut second = RtspSession::connect(&rig.target("cam/"), &opts(false), 2)
        .await
        .unwrap();
    for session in [&mut first, &mut second] {
        let mut previous = None;
        let mut keys = 0;
        for i in 0..25 {
            let f = timeout(Duration::from_secs(5), session.next_frame())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(f.params.codec, codec);
            assert_eq!((f.params.width, f.params.height), (320, 240));
            assert_eq!(f.loss, 0);
            if i == 0 {
                assert!(f.is_key, "a client starts on a key frame");
            }
            if let Some(p) = previous {
                assert_eq!(f.pts - p, ticks, "timestamps of the camera are kept");
            }
            previous = Some(f.pts);
            keys += usize::from(f.is_key);
            let mut rest = &f.data[..];
            while !rest.is_empty() {
                let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
                assert!(len > 0 && 4 + len <= rest.len(), "bad NAL length");
                rest = &rest[4 + len..];
            }
        }
        assert!(keys >= 2, "one key frame per 10 frames");
    }
    // The point of it all: two clients, one connection to the camera.
    assert_eq!(ServerStats::get(&rig.camera.stats().connections), 1);
    assert_eq!(ServerStats::get(&rig.camera.stats().plays), 1);
    // The live tap is released when clients leave.
    drop((first, second));
    timeout(Duration::from_secs(5), async {
        while rig.tap.viewers() > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn republishes_h264_to_several_clients() {
    republishes(Fixture::h264(), VideoCodec::H264).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn republishes_h265() {
    republishes(Fixture::h265(), VideoCodec::H265).await;
}

/// Sends one request and returns the status line and headers.
async fn request(stream: &mut tokio::net::TcpStream, text: &str) -> String {
    stream.write_all(text.as_bytes()).await.unwrap();
    let mut got = Vec::new();
    timeout(Duration::from_secs(8), async {
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0, "connection closed");
            got.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .unwrap();
    String::from_utf8_lossy(&got).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_what_it_cannot_serve() {
    let rig = Rig::start(MockServerConfig::new(Fixture::h264()), false, 1).await;
    let addr = rig.server.local_addr();
    let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
    let base = format!("rtsp://{addr}");

    let r = request(
        &mut c,
        &format!("OPTIONS {base}/cam RTSP/1.0\r\nCSeq: 1\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 200 OK\r\nCSeq: 1\r\n"), "{r}");
    assert!(r.contains("Public: OPTIONS, DESCRIBE, SETUP, PLAY"), "{r}");
    // Unknown camera, absent sub stream.
    for path in ["nope", "cam/sub", ""] {
        let r = request(
            &mut c,
            &format!("DESCRIBE {base}/{path} RTSP/1.0\r\nCSeq: 2\r\n\r\n"),
        )
        .await;
        assert!(r.starts_with("RTSP/1.0 404 "), "{path}: {r}");
    }
    let r = request(
        &mut c,
        &format!("DESCRIBE {base}/cam RTSP/1.0\r\nCSeq: 3\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 200 OK"), "{r}");
    assert!(r.contains(&format!("Content-Base: {base}/cam/\r\n")), "{r}");
    // RTP over UDP is not offered.
    let r = request(
        &mut c,
        &format!("SETUP {base}/cam/trackID=0 RTSP/1.0\r\nCSeq: 4\r\nTransport: RTP/AVP;unicast;client_port=5000-5001\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 461 "), "{r}");
    // No sound on this camera.
    let r = request(
        &mut c,
        &format!("SETUP {base}/cam/trackID=1 RTSP/1.0\r\nCSeq: 5\r\nTransport: RTP/AVP/TCP;unicast;interleaved=2-3\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 404 "), "{r}");
    // Playing needs a track.
    let r = request(
        &mut c,
        &format!("PLAY {base}/cam RTSP/1.0\r\nCSeq: 6\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 455 "), "{r}");

    // One client at a time here: the second connection is closed at once.
    let mut second = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut buf = [0u8; 16];
    let n = timeout(Duration::from_secs(5), second.read(&mut buf))
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!(n, 0);
    // And accepted again once the first one left.
    let r = request(
        &mut c,
        &format!("TEARDOWN {base}/cam RTSP/1.0\r\nCSeq: 7\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 200 OK"), "{r}");
    drop(c);
    timeout(Duration::from_secs(5), async {
        loop {
            let mut again = tokio::net::TcpStream::connect(addr).await.unwrap();
            again
                .write_all(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n")
                .await
                .unwrap();
            let mut buf = [0u8; 16];
            if again.read(&mut buf).await.unwrap_or(0) > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

async fn ffprobe(url: &str) -> Option<String> {
    let run = tokio::process::Command::new("ffprobe")
        .args(["-v", "error", "-rtsp_transport", "tcp", "-show_entries"])
        .arg("stream=codec_type,codec_name,width,height,sample_rate")
        .args(["-of", "csv=p=0", url])
        .output();
    let out = timeout(Duration::from_secs(30), run).await.ok()?.ok()?;
    assert!(
        out.status.success(),
        "ffprobe: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ffprobe_available() -> bool {
    std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[tokio::test(flavor = "multi_thread")]
async fn ffmpeg_reads_picture_and_sound() {
    if !ffprobe_available() {
        eprintln!("ffprobe not found: skipping");
        return;
    }
    // G.711 from the camera, republished as the Opus it was converted to.
    let mut config = MockServerConfig::new(Fixture::h264());
    config.pcmu_audio = true;
    let rig = Rig::start(config, true, 8).await;
    let streams = ffprobe(&rig.url("cam")).await.unwrap();
    assert!(streams.contains("h264,video,320,240"), "{streams}");
    assert!(streams.contains("opus,audio,48000"), "{streams}");

    let rig = Rig::start(MockServerConfig::new(Fixture::h265()), false, 8).await;
    let streams = ffprobe(&rig.url("cam")).await.unwrap();
    assert!(streams.contains("hevc,video,320,240"), "{streams}");
    assert!(!streams.contains("audio"), "{streams}");
}

fn credentials() -> Credentials {
    Credentials {
        username: "nvr".to_owned(),
        password: Secret::new("s3cret pass"),
    }
}

async fn protected(max_clients_per_address: usize) -> Rig {
    let options = Options {
        max_clients: 8,
        max_clients_per_address,
    };
    Rig::with(
        MockServerConfig::new(Fixture::h264()),
        false,
        options,
        Some(credentials()),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn protected_streams_need_the_right_password() {
    let rig = protected(8).await;
    let with = |user: Option<&str>, password: Option<&str>| {
        RtspTarget::parse(&rig.url("cam"), user, password.map(Secret::new)).unwrap()
    };
    // The right account: Retina answers the Digest challenge and plays.
    let mut session =
        RtspSession::connect(&with(Some("nvr"), Some("s3cret pass")), &opts(false), 1)
            .await
            .unwrap();
    let f = timeout(Duration::from_secs(5), session.next_frame())
        .await
        .unwrap()
        .unwrap();
    assert!(f.is_key);
    drop(session);
    // No account, a wrong password.
    assert!(
        RtspSession::connect(&with(None, None), &opts(false), 2)
            .await
            .is_err()
    );
    assert!(
        RtspSession::connect(&with(Some("nvr"), Some("nope")), &opts(false), 3)
            .await
            .is_err()
    );

    let addr = rig.server.local_addr();
    let base = format!("rtsp://{addr}");
    let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Both challenges, Digest first; nothing said about which cameras exist.
    for path in ["cam", "nope"] {
        let r = request(
            &mut c,
            &format!("DESCRIBE {base}/{path} RTSP/1.0\r\nCSeq: 1\r\n\r\n"),
        )
        .await;
        assert!(r.starts_with("RTSP/1.0 401 "), "{path}: {r}");
        let digest = r
            .find("WWW-Authenticate: Digest realm=\"RustNVR\", nonce=\"")
            .unwrap();
        let basic = r.find("WWW-Authenticate: Basic realm=\"RustNVR\"").unwrap();
        assert!(digest < basic, "{r}");
    }
    // Basic works too; an unknown camera is then said so.
    let basic = "Authorization: Basic bnZyOnMzY3JldCBwYXNz"; // nvr:s3cret pass
    let r = request(
        &mut c,
        &format!("DESCRIBE {base}/nope RTSP/1.0\r\nCSeq: 2\r\n{basic}\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 404 "), "{r}");
    let r = request(
        &mut c,
        &format!("DESCRIBE {base}/cam RTSP/1.0\r\nCSeq: 3\r\n{basic}\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("RTSP/1.0 200 OK"), "{r}");

    if ffprobe_available() {
        let url = format!("rtsp://nvr:s3cret%20pass@{addr}/cam");
        let streams = ffprobe(&url).await.unwrap();
        assert!(streams.contains("h264,video,320,240"), "{streams}");
    }
}

/// True when the server closes `stream` without answering.
async fn closed(stream: &mut tokio::net::TcpStream) -> bool {
    let mut buf = [0u8; 16];
    timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap_or(0)
        == 0
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_passwords_get_the_address_refused() {
    let rig = protected(8).await;
    let addr = rig.server.local_addr();
    let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
    let wrong = "Authorization: Basic bnZyOm5vcGU="; // nvr:nope
    for i in 1..=5 {
        let r = request(
            &mut c,
            &format!("DESCRIBE rtsp://{addr}/cam RTSP/1.0\r\nCSeq: {i}\r\n{wrong}\r\n\r\n"),
        )
        .await;
        assert!(r.starts_with("RTSP/1.0 401 "), "{r}");
    }
    // The connection is closed, and the address no longer answered, even
    // with the right password.
    assert!(closed(&mut c).await);
    let mut again = tokio::net::TcpStream::connect(addr).await.unwrap();
    let _ = again
        .write_all(
            format!("DESCRIBE rtsp://{addr}/cam RTSP/1.0\r\nCSeq: 1\r\nAuthorization: Basic bnZyOnMzY3JldCBwYXNz\r\n\r\n")
                .as_bytes(),
        )
        .await;
    assert!(closed(&mut again).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_address_cannot_take_every_place() {
    let rig = protected(2).await;
    let addr = rig.server.local_addr();
    let options = "OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n";
    let mut first = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut second = tokio::net::TcpStream::connect(addr).await.unwrap();
    assert!(
        request(&mut first, options)
            .await
            .starts_with("RTSP/1.0 200")
    );
    assert!(
        request(&mut second, options)
            .await
            .starts_with("RTSP/1.0 200")
    );
    // The third connection of this address is closed, with places left
    // (8) for other addresses.
    let mut third = tokio::net::TcpStream::connect(addr).await.unwrap();
    assert!(closed(&mut third).await);
    drop(first);
    timeout(Duration::from_secs(5), async {
        loop {
            let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
            c.write_all(options.as_bytes()).await.unwrap();
            let mut buf = [0u8; 16];
            if c.read(&mut buf).await.unwrap_or(0) > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
