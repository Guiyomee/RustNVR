//! Ingestion tests against the mock RTSP server of `rustnvr-testkit`.
#![allow(clippy::unwrap_used, missing_docs)]

use rustnvr_core::events::event_bus;
use rustnvr_core::{CameraId, RtspTarget, Secret, StreamItem, StreamRole, StreamState, VideoCodec};
use rustnvr_stream::{
    BackoffConfig, FrameFanout, RtspSession, SessionOptions, SupervisorConfig, probe,
    spawn_supervisor,
};
use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig, ServerStats};
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

fn opts() -> SessionOptions {
    SessionOptions {
        connect_timeout: Duration::from_secs(3),
        stall_timeout: Duration::from_millis(800),
        ..SessionOptions::default()
    }
}

fn fast_backoff() -> BackoffConfig {
    BackoffConfig {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(100),
        stable_after: Duration::from_secs(60),
    }
}

fn target(server: &MockRtspServer) -> RtspTarget {
    RtspTarget::parse(&server.url(), None, None).unwrap()
}

async fn check_frames(fixture: Fixture, codec: VideoCodec, rfc6381_prefix: &str) {
    let ticks = i64::from(fixture.ticks_per_frame());
    let server = MockRtspServer::start(MockServerConfig::new(fixture))
        .await
        .unwrap();
    let mut session = RtspSession::connect(&target(&server), &opts(), 1)
        .await
        .unwrap();
    let mut prev_pts = None;
    let mut keys = 0;
    for expected_seq in 0..25 {
        let f = timeout(Duration::from_secs(5), session.next_frame())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.seq, expected_seq);
        assert_eq!(f.session, 1);
        assert_eq!(f.params.codec, codec);
        assert_eq!((f.params.width, f.params.height), (320, 240));
        assert!(
            f.params.rfc6381_codec.starts_with(rfc6381_prefix),
            "{}",
            f.params.rfc6381_codec
        );
        assert_eq!(f.params.clock_rate, 90_000);
        if expected_seq == 0 {
            assert!(f.is_key, "stream must start on a key frame");
        }
        if let Some(prev) = prev_pts {
            assert_eq!(f.pts - prev, ticks);
        }
        prev_pts = Some(f.pts);
        keys += usize::from(f.is_key);
        // MP4 framing: the payload is a sequence of 4-byte length-prefixed NAL units.
        let mut rest = &f.data[..];
        while !rest.is_empty() {
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            assert!(len > 0 && 4 + len <= rest.len(), "bad NAL length");
            rest = &rest[4 + len..];
        }
    }
    assert_eq!(keys, 3, "one key frame per 10 frames");
}

#[tokio::test]
async fn receives_h264_frames() {
    check_frames(Fixture::h264(), VideoCodec::H264, "avc1.").await;
}

#[tokio::test]
async fn receives_h265_frames() {
    check_frames(Fixture::h265(), VideoCodec::H265, "hvc1.").await;
}

#[tokio::test]
async fn basic_authentication() {
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.credentials = Some(("admin".into(), "s3cret".into()));
    let server = MockRtspServer::start(cfg).await.unwrap();

    let good =
        RtspTarget::parse(&server.url(), Some("admin"), Some(Secret::new("s3cret"))).unwrap();
    let mut s = RtspSession::connect(&good, &opts(), 1).await.unwrap();
    assert!(s.next_frame().await.unwrap().is_key);

    let bad = RtspTarget::parse(&server.url(), Some("admin"), Some(Secret::new("wrong"))).unwrap();
    let err = RtspSession::connect(&bad, &opts(), 2).await.err().unwrap();
    assert_eq!(err.status_code(), Some(401), "{err}");
    assert!(!err.to_string().contains("wrong"), "password leaked: {err}");
}

#[tokio::test]
async fn probe_reports_streams_and_first_key_frame() {
    let server = MockRtspServer::start(MockServerConfig::new(Fixture::h265()))
        .await
        .unwrap();
    let report = probe(&target(&server), &opts(), true).await.unwrap();
    assert_eq!(report.streams.len(), 1);
    assert_eq!(report.streams[0].encoding, "h265");
    assert_eq!(report.selected, Some(0));
    let video = report.video.unwrap();
    assert_eq!(video.codec, VideoCodec::H265);
    assert_eq!((video.width, video.height), (320, 240));
}

fn supervisor_config(server_url: &str, stall: Duration) -> SupervisorConfig {
    SupervisorConfig {
        camera: CameraId::new("test").unwrap(),
        role: StreamRole::Main,
        target: RtspTarget::parse(server_url, None, None).unwrap(),
        session: SessionOptions {
            stall_timeout: stall,
            ..opts()
        },
        backoff: fast_backoff(),
    }
}

#[tokio::test]
async fn supervisor_reconnects_after_disconnection() {
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.disconnect_after_frames = Some(12);
    let server = MockRtspServer::start(cfg).await.unwrap();

    let mut fanout = FrameFanout::new();
    let (mut rx, _) = fanout.subscribe("test", 1024);
    let cancel = CancellationToken::new();
    let handle = spawn_supervisor(
        supervisor_config(&server.url(), Duration::from_secs(2)),
        fanout,
        event_bus(64),
        cancel.clone(),
    );

    let mut sessions_with_frames = BTreeSet::new();
    let mut ended = BTreeSet::new();
    let mut frames_per_session = std::collections::BTreeMap::<u64, u64>::new();
    timeout(Duration::from_secs(15), async {
        while sessions_with_frames.len() < 3 || ended.len() < 2 {
            match rx.recv().await.unwrap() {
                StreamItem::Frame(f) => {
                    sessions_with_frames.insert(f.session);
                    let n = frames_per_session.entry(f.session).or_default();
                    // Sequence numbers restart at 0 on every session.
                    assert_eq!(f.seq, *n);
                    *n += 1;
                }
                StreamItem::SessionEnd { session } => {
                    ended.insert(session);
                }
            }
        }
    })
    .await
    .expect("expected three sessions");

    assert!(ServerStats::get(&server.stats().plays) >= 3);
    let m = handle.metrics.snapshot();
    assert!(m.sessions >= 3 && m.failures >= 2, "{m:?}");
    cancel.cancel();
    timeout(Duration::from_secs(5), handle.task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*handle.state.borrow(), StreamState::Stopped);
}

#[tokio::test]
async fn supervisor_detects_stalled_stream() {
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.stall_after_frames = Some(5);
    let server = MockRtspServer::start(cfg).await.unwrap();

    let mut fanout = FrameFanout::new();
    let (_rx, _) = fanout.subscribe("test", 1024);
    let cancel = CancellationToken::new();
    let mut handle = spawn_supervisor(
        supervisor_config(&server.url(), Duration::from_millis(300)),
        fanout,
        event_bus(64),
        cancel.clone(),
    );
    let state = timeout(Duration::from_secs(10), async {
        loop {
            handle.state.changed().await.unwrap();
            let s = handle.state.borrow().clone();
            if matches!(s, StreamState::Retrying { .. }) {
                return s;
            }
        }
    })
    .await
    .unwrap();
    let StreamState::Retrying { error, .. } = state else {
        unreachable!()
    };
    assert!(error.contains("no video frame"), "{error}");
    // The supervisor keeps trying: a second session is opened.
    timeout(Duration::from_secs(10), async {
        while ServerStats::get(&server.stats().plays) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    handle.task.await.unwrap();
}

#[tokio::test]
async fn supervisor_retries_refused_connections_and_stops_on_cancel() {
    // Reserve a port and close it so that connections are refused.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("rtsp://127.0.0.1:{port}/none");
    let cancel = CancellationToken::new();
    let mut handle = spawn_supervisor(
        supervisor_config(&url, Duration::from_secs(1)),
        FrameFanout::new(),
        event_bus(64),
        cancel.clone(),
    );
    let max_attempt = timeout(Duration::from_secs(10), async {
        loop {
            handle.state.changed().await.unwrap();
            if let StreamState::Retrying { attempt, error, .. } = &*handle.state.borrow()
                && *attempt >= 3
            {
                assert!(!error.is_empty());
                return *attempt;
            }
        }
    })
    .await
    .unwrap();
    assert!(max_attempt >= 3);
    cancel.cancel();
    timeout(Duration::from_secs(2), handle.task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*handle.state.borrow(), StreamState::Stopped);
}

/// Reproduces a real camera whose PLAY response announces an RTP sequence
/// number unrelated to the first packet (behind it, then ahead of it).
#[tokio::test]
async fn tolerates_wrong_rtp_info_sequence_number() {
    for offset in [3382u16, 65535 - 13974] {
        let mut cfg = MockServerConfig::new(Fixture::h265());
        cfg.rtp_info_seq_offset = offset;
        let server = MockRtspServer::start(cfg).await.unwrap();
        let mut s = RtspSession::connect(&target(&server), &opts(), 1)
            .await
            .unwrap();
        for _ in 0..12 {
            let f = timeout(Duration::from_secs(5), s.next_frame())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                f.loss, 0,
                "announced seq must not be counted as loss (offset {offset})"
            );
        }
    }
}

/// Reproduces a camera (Reolink E1) sending fragmented NALs whose first
/// packet is missing without a gap in the sequence numbers: Retina refuses
/// those packets (« FU-A has start bit unset while no frag in progress »),
/// the session skips them and keeps going.
#[tokio::test]
async fn skips_fragments_without_their_start() {
    for fixture in [Fixture::h264(), Fixture::h265()] {
        let mut cfg = MockServerConfig::new(fixture);
        cfg.drop_fragment_start_every = Some(3);
        let server = MockRtspServer::start(cfg).await.unwrap();
        let mut s = RtspSession::connect(&target(&server), &opts(), 1)
            .await
            .unwrap();
        let (mut damaged, mut keys) = (0, 0);
        for expected_seq in 0..40 {
            let f = timeout(Duration::from_secs(5), s.next_frame())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(f.seq, expected_seq);
            damaged += usize::from(f.loss > 0);
            keys += usize::from(f.is_key);
        }
        let stats = server.stats();
        assert!(ServerStats::get(&stats.dropped_fragment_starts) > 5);
        assert!(damaged > 0, "skipped packets are counted as lost");
        assert!(keys > 0);
        assert_eq!(ServerStats::get(&stats.connections), 1, "no reconnection");
    }
}

/// The same without RTP marker bits: Retina ends a frame on the next
/// timestamp, so a refused packet can leave a frame pending, on which it
/// panics afterwards. The session must fail (and be started again by the
/// supervisor), not the task.
#[tokio::test]
async fn retina_failures_end_the_session_only() {
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.drop_fragment_start_every = Some(3);
    cfg.marker = false;
    let server = MockRtspServer::start(cfg).await.unwrap();
    let mut s = RtspSession::connect(&target(&server), &opts(), 1)
        .await
        .unwrap();
    let mut frames = 0;
    let outcome = loop {
        match timeout(Duration::from_secs(5), s.next_frame())
            .await
            .unwrap()
        {
            Ok(_) if frames < 60 => frames += 1,
            Ok(_) => break None,
            Err(e) => break Some(e),
        }
    };
    if let Some(e) = outcome {
        assert!(
            matches!(e, rustnvr_stream::StreamError::ClientFailure(_)),
            "{e}"
        );
    }
}
