//! Minimal RTSP/1.0 server streaming a fixture over TCP interleaved RTP.

use crate::annexb::{Codec, Fixture};
use crate::rtp;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use bytes::{Buf, BytesMut};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const PAYLOAD_TYPE: u8 = 96;
const SSRC: u32 = 0x1234_5678;
const SESSION_ID: &str = "4f3a2b1c";

/// Behaviour of a [`MockRtspServer`]. Behaviours apply to every connection.
#[derive(Clone, Debug)]
pub struct MockServerConfig {
    /// Stream to serve, looped forever.
    pub fixture: Arc<Fixture>,
    /// Wall-clock delay between frames. RTP timestamps always advance at the
    /// fixture frame rate, so a short interval simulates faster-than-real-time.
    pub frame_interval: Duration,
    /// When set, requests other than OPTIONS require this Basic user/password.
    pub credentials: Option<(String, String)>,
    /// Close the TCP connection after sending this many frames.
    pub disconnect_after_frames: Option<usize>,
    /// Stop sending frames (but keep the connection open) after this many frames.
    pub stall_after_frames: Option<usize>,
    /// URL path of the stream.
    pub path: String,
    /// Added to the sequence number announced in the PLAY `RTP-Info` header,
    /// to imitate cameras whose announced value does not match the first
    /// RTP packet.
    pub rtp_info_seq_offset: u16,
}

impl MockServerConfig {
    /// Serves `fixture` with a 10 ms frame interval and no special behaviour.
    pub fn new(fixture: Fixture) -> Self {
        Self {
            fixture: Arc::new(fixture),
            frame_interval: Duration::from_millis(10),
            credentials: None,
            disconnect_after_frames: None,
            stall_after_frames: None,
            path: "/stream".to_owned(),
            rtp_info_seq_offset: 0,
        }
    }
}

/// Counters observed by tests.
#[derive(Debug, Default)]
pub struct ServerStats {
    /// Accepted TCP connections.
    pub connections: AtomicU64,
    /// PLAY requests answered successfully.
    pub plays: AtomicU64,
    /// Frames (access units) sent over all connections.
    pub frames_sent: AtomicU64,
    /// Requests rejected with 401.
    pub auth_failures: AtomicU64,
}

impl ServerStats {
    /// Reads a counter.
    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::SeqCst)
    }
}

/// A running mock server, stopped when dropped.
pub struct MockRtspServer {
    addr: SocketAddr,
    path: String,
    stats: Arc<ServerStats>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl MockRtspServer {
    /// Binds to an ephemeral port on 127.0.0.1 and starts serving.
    pub async fn start(config: MockServerConfig) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let stats = Arc::new(ServerStats::default());
        let cancel = CancellationToken::new();
        let config = Arc::new(config);
        let task = tokio::spawn(accept_loop(
            listener,
            config.clone(),
            stats.clone(),
            cancel.clone(),
        ));
        Ok(Self {
            addr,
            path: config.path.clone(),
            stats,
            cancel,
            task: Some(task),
        })
    }

    /// `rtsp://127.0.0.1:<port>/<path>`.
    pub fn url(&self) -> String {
        format!("rtsp://{}{}", self.addr, self.path)
    }

    /// Bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Counters.
    pub fn stats(&self) -> &ServerStats {
        &self.stats
    }

    /// Stops accepting connections, closes existing ones and waits for the server task.
    pub async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockRtspServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn accept_loop(
    listener: TcpListener,
    config: Arc<MockServerConfig>,
    stats: Arc<ServerStats>,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return,
            a = listener.accept() => a,
        };
        let Ok((stream, peer)) = accepted else {
            continue;
        };
        stats.connections.fetch_add(1, Ordering::SeqCst);
        let (config, stats, cancel) = (config.clone(), stats.clone(), cancel.clone());
        tokio::spawn(async move {
            let base = format!(
                "rtsp://{}{}",
                stream.local_addr().unwrap_or(peer),
                config.path
            );
            tokio::select! {
                _ = cancel.cancelled() => {}
                r = Connection::new(&config, &stats, base).run(stream) => {
                    if let Err(e) = r {
                        tracing::debug!(error = %e, "mock connection ended with error");
                    }
                }
            }
        });
    }
}

struct Request {
    method: String,
    cseq: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

enum Incoming {
    Request(Request),
    InterleavedData,
}

fn parse_incoming(buf: &mut BytesMut) -> io::Result<Option<Incoming>> {
    if buf.is_empty() {
        return Ok(None);
    }
    if buf[0] == b'$' {
        if buf.len() < 4 {
            return Ok(None);
        }
        let len = usize::from(u16::from_be_bytes([buf[2], buf[3]]));
        if buf.len() < 4 + len {
            return Ok(None);
        }
        buf.advance(4 + len);
        return Ok(Some(Incoming::InterleavedData));
    }
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        if buf.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request too large",
            ));
        }
        return Ok(None);
    };
    let head = std::str::from_utf8(&buf[..end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 request"))?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let method = request_line
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Content-Length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    if buf.len() < end + 4 + content_length {
        return Ok(None);
    }
    let cseq = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("CSeq"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    buf.advance(end + 4 + content_length);
    Ok(Some(Incoming::Request(Request {
        method,
        cseq,
        headers,
    })))
}

fn response(code: u16, reason: &str, cseq: &str, extra: &[(&str, String)], body: &str) -> Vec<u8> {
    let mut r = format!("RTSP/1.0 {code} {reason}\r\nCSeq: {cseq}\r\nServer: rustnvr-testkit\r\n");
    for (k, v) in extra {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    r.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    r.into_bytes()
}

struct Connection<'a> {
    config: &'a MockServerConfig,
    stats: &'a ServerStats,
    base: String,
    playing: bool,
    channel: u8,
    seq: u16,
    timestamp: u32,
    next_frame: usize,
    sent: usize,
}

enum Outcome {
    Continue,
    Close,
}

impl<'a> Connection<'a> {
    fn new(config: &'a MockServerConfig, stats: &'a ServerStats, base: String) -> Self {
        Self {
            config,
            stats,
            base,
            playing: false,
            channel: 0,
            seq: 1000,
            timestamp: 0,
            next_frame: 0,
            sent: 0,
        }
    }

    async fn run(mut self, mut stream: TcpStream) -> io::Result<()> {
        let mut buf = BytesMut::with_capacity(4096);
        let mut ticker = tokio::time::interval(self.config.frame_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            while let Some(incoming) = parse_incoming(&mut buf)? {
                if let Incoming::Request(req) = incoming {
                    let (resp, outcome) = self.handle(&req);
                    stream.write_all(&resp).await?;
                    if matches!(outcome, Outcome::Close) {
                        return Ok(());
                    }
                }
            }
            let stalled = self
                .config
                .stall_after_frames
                .is_some_and(|n| self.sent >= n);
            tokio::select! {
                n = stream.read_buf(&mut buf) => {
                    if n? == 0 {
                        return Ok(());
                    }
                }
                _ = ticker.tick(), if self.playing && !stalled => {
                    let packets = self.next_frame_packets();
                    stream.write_all(&packets).await?;
                    self.sent += 1;
                    self.stats.frames_sent.fetch_add(1, Ordering::SeqCst);
                    if self.config.disconnect_after_frames.is_some_and(|n| self.sent >= n) {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn next_frame_packets(&mut self) -> Vec<u8> {
        let fixture = &self.config.fixture;
        let au = &fixture.access_units[self.next_frame % fixture.access_units.len()];
        self.next_frame += 1;
        let payloads = rtp::payloads(fixture.codec, au);
        let mut out = Vec::new();
        let n = payloads.len();
        for (i, payload) in payloads.iter().enumerate() {
            let pkt = rtp::packet(
                PAYLOAD_TYPE,
                i + 1 == n,
                self.seq,
                self.timestamp,
                SSRC,
                payload,
            );
            self.seq = self.seq.wrapping_add(1);
            out.push(b'$');
            out.push(self.channel);
            out.extend_from_slice(&u16::try_from(pkt.len()).unwrap_or(u16::MAX).to_be_bytes());
            out.extend_from_slice(&pkt);
        }
        self.timestamp = self.timestamp.wrapping_add(fixture.ticks_per_frame());
        out
    }

    fn authorized(&self, req: &Request) -> bool {
        let Some((user, pass)) = &self.config.credentials else {
            return true;
        };
        let expected = format!("Basic {}", B64.encode(format!("{user}:{pass}")));
        req.header("Authorization") == Some(expected.as_str())
    }

    fn handle(&mut self, req: &Request) -> (Vec<u8>, Outcome) {
        let cseq = req.cseq.as_str();
        if req.method != "OPTIONS" && !self.authorized(req) {
            self.stats.auth_failures.fetch_add(1, Ordering::SeqCst);
            let extra = [(
                "WWW-Authenticate",
                "Basic realm=\"rustnvr-testkit\"".to_owned(),
            )];
            return (
                response(401, "Unauthorized", cseq, &extra, ""),
                Outcome::Continue,
            );
        }
        let session = [("Session", format!("{SESSION_ID};timeout=60"))];
        let resp = match req.method.as_str() {
            "OPTIONS" => {
                let public = "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER".to_owned();
                response(200, "OK", cseq, &[("Public", public)], "")
            }
            "DESCRIBE" => {
                let extra = [
                    ("Content-Base", format!("{}/", self.base)),
                    ("Content-Type", "application/sdp".to_owned()),
                ];
                response(200, "OK", cseq, &extra, &self.sdp())
            }
            "SETUP" => {
                let transport = req.header("Transport").unwrap_or_default();
                if !transport.contains("TCP") {
                    return (
                        response(461, "Unsupported Transport", cseq, &[], ""),
                        Outcome::Continue,
                    );
                }
                self.channel = transport
                    .split(';')
                    .find_map(|p| p.strip_prefix("interleaved="))
                    .and_then(|p| p.split('-').next())
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                let extra = [
                    (
                        "Transport",
                        format!(
                            "RTP/AVP/TCP;unicast;interleaved={}-{};ssrc={SSRC:08X}",
                            self.channel,
                            self.channel + 1
                        ),
                    ),
                    session[0].clone(),
                ];
                response(200, "OK", cseq, &extra, "")
            }
            "PLAY" => {
                self.playing = true;
                self.stats.plays.fetch_add(1, Ordering::SeqCst);
                let extra = [
                    session[0].clone(),
                    ("Range", "npt=0.000-".to_owned()),
                    (
                        "RTP-Info",
                        format!(
                            "url={}/trackID=0;seq={};rtptime={}",
                            self.base,
                            self.seq.wrapping_add(self.config.rtp_info_seq_offset),
                            self.timestamp
                        ),
                    ),
                ];
                response(200, "OK", cseq, &extra, "")
            }
            "GET_PARAMETER" | "SET_PARAMETER" => response(200, "OK", cseq, &session, ""),
            "TEARDOWN" => return (response(200, "OK", cseq, &session, ""), Outcome::Close),
            _ => response(501, "Not Implemented", cseq, &[], ""),
        };
        (resp, Outcome::Continue)
    }

    fn sdp(&self) -> String {
        let f = &self.config.fixture;
        let (rtpmap, fmtp) = match f.codec {
            Codec::H264 => (
                "H264/90000".to_owned(),
                format!(
                    "packetization-mode=1;profile-level-id={:02X}{:02X}{:02X};sprop-parameter-sets={},{}",
                    f.sps[1],
                    f.sps[2],
                    f.sps[3],
                    B64.encode(&f.sps),
                    B64.encode(&f.pps)
                ),
            ),
            Codec::H265 => (
                "H265/90000".to_owned(),
                format!(
                    "sprop-vps={};sprop-sps={};sprop-pps={}",
                    B64.encode(f.vps.as_deref().unwrap_or_default()),
                    B64.encode(&f.sps),
                    B64.encode(&f.pps)
                ),
            ),
        };
        format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=rustnvr-testkit\r\nt=0 0\r\n\
             m=video 0 RTP/AVP {PAYLOAD_TYPE}\r\nc=IN IP4 0.0.0.0\r\n\
             a=rtpmap:{PAYLOAD_TYPE} {rtpmap}\r\na=fmtp:{PAYLOAD_TYPE} {fmtp}\r\n\
             a=framerate:{}\r\na=control:trackID=0\r\n",
            f.fps
        )
    }
}
