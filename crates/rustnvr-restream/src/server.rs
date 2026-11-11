//! RTSP/1.0 server republishing the cameras.
//!
//! `rtsp://host:port/<camera>` is the main stream of a camera,
//! `rtsp://host:port/<camera>/sub` its sub stream. RTP is sent interleaved in
//! the RTSP connection only (`RTP/AVP/TCP`). A stream may require credentials
//! (Digest or Basic); addresses that keep sending wrong ones are refused.
//!
//! A client costs one task and one copy of the stream into RTP packets. A
//! client that reads too slowly starts again at the next key frame, and is
//! dropped when it stops reading: it never slows ingestion or recording.

use crate::auth::{self, Credentials, FailureGuard};
use crate::describe::{AUDIO_PAYLOAD_TYPE, Description, VIDEO_CLOCK, VIDEO_PAYLOAD_TYPE};
use crate::rtp::{Timeline, Track, audio_packets, video_packets};
use bytes::{Buf, BytesMut};
use futures::future::BoxFuture;
use rustnvr_core::VideoFrame;
use rustnvr_stream::LiveTap;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// What the application knows about a stream asked for.
#[derive(Clone, Default)]
pub struct Source {
    /// Its live frames; `None` when the camera is unknown, disabled or has
    /// no such stream.
    pub tap: Option<LiveTap>,
    /// What a client must present to read it (`None`: nothing). Also given
    /// for unknown cameras, so that only authenticated clients learn which
    /// cameras exist.
    pub credentials: Option<Credentials>,
}

/// Finds a stream of a camera (`true`: its sub stream).
pub type Lookup = Arc<dyn Fn(String, bool) -> BoxFuture<'static, Source> + Send + Sync>;

/// Limits of a server.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Most clients served at a time.
    pub max_clients: usize,
    /// Most clients served at a time from one address.
    pub max_clients_per_address: usize,
}

/// What the connections of a server share.
struct Shared {
    lookup: Lookup,
    options: Options,
    failures: FailureGuard,
    /// Connections open, by client address.
    per_address: Mutex<HashMap<IpAddr, usize>>,
}

/// One connection counted for its address, until dropped.
struct AddressSlot(Arc<Shared>, IpAddr);

impl AddressSlot {
    fn take(shared: &Arc<Shared>, address: IpAddr) -> Option<Self> {
        let mut map = shared.per_address.lock().unwrap_or_else(|e| e.into_inner());
        let count = map.entry(address).or_insert(0);
        if *count >= shared.options.max_clients_per_address.max(1) {
            return None;
        }
        *count += 1;
        Some(Self(shared.clone(), address))
    }
}

impl Drop for AddressSlot {
    fn drop(&mut self) {
        let mut map = self.0.per_address.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = map.get_mut(&self.1) {
            *count -= 1;
            if *count == 0 {
                map.remove(&self.1);
            }
        }
    }
}

/// Largest request accepted.
const MAX_REQUEST: usize = 64 * 1024;
/// A client that has not started playing after this long is dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// A client that does not take what is sent within this delay is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long DESCRIBE waits for a first picture of a camera that is connecting.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// Interval between RTCP sender reports.
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

const UNAUTHORIZED: (u16, &str) = (401, "Unauthorized");

const SERVER: &str = concat!("RustNVR/", env!("CARGO_PKG_VERSION"));
const METHODS: &str = "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER";

/// A running republication server.
pub struct RestreamServer {
    addr: SocketAddr,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl RestreamServer {
    /// Listens on `addr` and serves the streams found by `lookup`.
    pub async fn start(addr: SocketAddr, lookup: Lookup, options: Options) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(accept_loop(
            listener,
            Arc::new(Shared {
                lookup,
                options,
                failures: FailureGuard::default(),
                per_address: Mutex::new(HashMap::new()),
            }),
            cancel.clone(),
        ));
        Ok(Self {
            addr,
            cancel,
            task: Some(task),
        })
    }

    /// Address listened on.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops listening and disconnects the clients.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for RestreamServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>, cancel: CancellationToken) {
    let slots = Arc::new(Semaphore::new(shared.options.max_clients.max(1)));
    let counter = Arc::new(AtomicU32::new(0));
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return,
            a = listener.accept() => a,
        };
        let (stream, peer) = match accepted {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(error = %e, "RTSP republication: accept failed");
                // Out of file descriptors, typically: do not spin.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            tracing::warn!(%peer, "RTSP republication: too many clients, connection refused (restream.max_clients)");
            continue;
        };
        // Too many wrong passwords from this address: not even a 401.
        if shared.failures.blocked(peer.ip()) {
            tracing::debug!(%peer, "RTSP republication: address refused after wrong passwords");
            continue;
        }
        let Some(address_slot) = AddressSlot::take(&shared, peer.ip()) else {
            tracing::warn!(%peer, "RTSP republication: too many clients from this address, connection refused (restream.max_clients_per_address)");
            continue;
        };
        let _ = stream.set_nodelay(true);
        let id = counter.fetch_add(1, Ordering::Relaxed);
        let (shared, cancel) = (shared.clone(), cancel.clone());
        tokio::spawn(async move {
            let mut conn = Connection::new(shared, peer, id);
            let result = tokio::select! {
                _ = cancel.cancelled() => Ok(()),
                r = conn.run(stream) => r,
            };
            if let Some(stream) = &conn.playing_name {
                tracing::info!(%peer, stream = %stream, "RTSP client left");
            }
            if let Err(e) = result {
                tracing::debug!(%peer, error = %e, "RTSP client connection ended with an error");
            }
            drop((slot, address_slot));
        });
    }
}

struct Request {
    method: String,
    uri: String,
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

/// Takes one complete request off `buf`; interleaved data (the RTCP of the
/// client) is skipped.
fn parse_request(buf: &mut BytesMut) -> io::Result<Option<Request>> {
    let invalid = |m: &'static str| io::Error::new(io::ErrorKind::InvalidData, m);
    loop {
        // Blank lines between requests are allowed.
        while buf.first().is_some_and(|b| *b == b'\r' || *b == b'\n') {
            buf.advance(1);
        }
        if buf.first() != Some(&b'$') {
            break;
        }
        if buf.len() < 4 {
            return Ok(None);
        }
        let len = usize::from(u16::from_be_bytes([buf[2], buf[3]]));
        if buf.len() < 4 + len {
            return Ok(None);
        }
        buf.advance(4 + len);
    }
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        if buf.len() > MAX_REQUEST {
            return Err(invalid("request too large"));
        }
        return Ok(None);
    };
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| invalid("request is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let mut words = lines.next().unwrap_or_default().split(' ');
    let method = words.next().unwrap_or_default().to_owned();
    let uri = words.next().unwrap_or_default().to_owned();
    if !words.next().is_some_and(|v| v.starts_with("RTSP/")) {
        return Err(invalid("not an RTSP request"));
    }
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let body = find("Content-Length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if body > MAX_REQUEST {
        return Err(invalid("request too large"));
    }
    if buf.len() < end + 4 + body {
        return Ok(None);
    }
    let cseq = find("CSeq").unwrap_or_default();
    buf.advance(end + 4 + body);
    Ok(Some(Request {
        method,
        uri,
        cseq,
        headers,
    }))
}

fn response(code: u16, reason: &str, cseq: &str, extra: &[(&str, String)], body: &str) -> Vec<u8> {
    let mut r = format!("RTSP/1.0 {code} {reason}\r\nCSeq: {cseq}\r\nServer: {SERVER}\r\n");
    for (k, v) in extra {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() {
        r.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    r.push_str("\r\n");
    r.push_str(body);
    r.into_bytes()
}

/// What a request address designates.
#[derive(Debug, PartialEq, Eq)]
struct StreamPath {
    camera: String,
    sub: bool,
    /// `trackID=n` at the end (SETUP).
    track: Option<u8>,
}

impl StreamPath {
    fn name(&self) -> String {
        if self.sub {
            format!("{}/sub", self.camera)
        } else {
            self.camera.clone()
        }
    }
}

/// Reads `rtsp://host[:port]/<camera>[/sub][/trackID=n]`.
fn parse_path(uri: &str) -> Option<StreamPath> {
    let rest = uri.strip_prefix("rtsp://")?;
    let path = rest.split_once('/').map_or("", |(_, p)| p);
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let track = match parts.last().and_then(|p| p.strip_prefix("trackID=")) {
        Some(n) => {
            parts.pop();
            Some(n.parse().ok()?)
        }
        None => None,
    };
    let (camera, sub) = match parts.as_slice() {
        [camera] => (*camera, false),
        [camera, "sub"] => (*camera, true),
        _ => return None,
    };
    Some(StreamPath {
        camera: camera.to_owned(),
        sub,
        track,
    })
}

/// The stream a client asked for.
struct Selected {
    name: String,
    tap: LiveTap,
    description: Description,
}

/// What is being sent to a playing client.
struct Playing {
    frames: broadcast::Receiver<VideoFrame>,
    timeline: Timeline,
    /// Waiting for a key frame (start, or after falling behind).
    started: bool,
    /// The first picture was sent (its sound, older, is left out).
    sent_any: bool,
    next_report: Instant,
}

enum Outcome {
    Continue,
    Close,
}

/// What a connection waits for.
enum Event {
    /// Bytes read from the client (0: it closed the connection).
    Read(usize),
    Frame(Result<VideoFrame, RecvError>),
    /// Nothing played for too long.
    Idle,
}

struct Connection {
    shared: Arc<Shared>,
    peer: SocketAddr,
    session: String,
    /// Nonce of the Digest challenges of this connection.
    nonce: String,
    /// The address sent too many wrong passwords: close after answering.
    refused: bool,
    /// Seed of the sequence numbers and timestamps.
    seed: u32,
    selected: Option<Selected>,
    video: Option<Track>,
    audio: Option<Track>,
    playing: Option<Playing>,
    /// Name of the stream being played, for the log.
    playing_name: Option<String>,
}

impl Connection {
    fn new(shared: Arc<Shared>, peer: SocketAddr, id: u32) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        // Not a secret: only distinguishes sessions.
        let seed = nanos
            .wrapping_mul(2_654_435_761)
            .wrapping_add(id.wrapping_mul(40_503));
        Self {
            shared,
            peer,
            session: format!("{seed:08x}{id:04x}"),
            nonce: auth::new_nonce(),
            refused: false,
            seed,
            selected: None,
            video: None,
            audio: None,
            playing: None,
            playing_name: None,
        }
    }

    async fn run(&mut self, mut stream: TcpStream) -> io::Result<()> {
        let mut buf = BytesMut::with_capacity(4096);
        let mut out = Vec::new();
        let idle = tokio::time::sleep(IDLE_TIMEOUT);
        tokio::pin!(idle);
        loop {
            while let Some(req) = parse_request(&mut buf)? {
                out.clear();
                let outcome = self.handle(&req, &mut out).await;
                write(&mut stream, &out).await?;
                if matches!(outcome, Outcome::Close) || self.refused {
                    return Ok(());
                }
            }
            let playing = self.playing.is_some();
            let frame = async {
                match &mut self.playing {
                    Some(p) => p.frames.recv().await,
                    None => std::future::pending().await,
                }
            };
            let event = tokio::select! {
                n = stream.read_buf(&mut buf) => Event::Read(n?),
                frame = frame => Event::Frame(frame),
                _ = &mut idle, if !playing => Event::Idle,
            };
            match event {
                Event::Read(0) | Event::Idle | Event::Frame(Err(RecvError::Closed)) => {
                    return Ok(());
                }
                Event::Read(_) => {}
                Event::Frame(Ok(f)) => {
                    out.clear();
                    if !self.packets(&f, &mut out) {
                        // Another codec or picture size: the client asks again.
                        return Ok(());
                    }
                    write(&mut stream, &out).await?;
                }
                Event::Frame(Err(RecvError::Lagged(missed))) => {
                    tracing::debug!(peer = %self.peer, missed, "RTSP client too slow, resuming at the next key frame");
                    if let Some(p) = &mut self.playing {
                        p.started = false;
                    }
                }
            }
        }
    }

    /// Finds the stream of `path`, checks that `req` may read it and waits
    /// for its description, unless it is the one already selected (and
    /// authorized) on this connection.
    async fn select(
        &mut self,
        req: &Request,
        path: &StreamPath,
    ) -> Result<(), (u16, &'static str)> {
        let name = path.name();
        if self.selected.as_ref().is_some_and(|s| s.name == name) {
            return Ok(());
        }
        let source = (self.shared.lookup)(path.camera.clone(), path.sub).await;
        if let Some(credentials) = &source.credentials {
            let address = self.peer.ip();
            match req.header("Authorization") {
                // The first request of a client carries none: not a failure.
                None => return Err(UNAUTHORIZED),
                Some(h) if auth::authorized(h, &req.method, credentials, &self.nonce) => {
                    self.shared.failures.succeeded(address);
                }
                Some(_) => {
                    self.refused = self.shared.failures.failed(address);
                    tracing::warn!(client = %address, stream = %name, refused = self.refused, "RTSP republication: wrong user name or password");
                    return Err(UNAUTHORIZED);
                }
            }
        }
        let tap = source.tap.ok_or((404, "Not Found"))?;
        let (backlog, mut frames) = tap.subscribe();
        let first = match backlog.into_iter().next() {
            Some(f) => Some(f),
            None => tokio::time::timeout(FIRST_FRAME_TIMEOUT, async {
                loop {
                    match frames.recv().await {
                        Ok(f) => return Some(f),
                        Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => return None,
                    }
                }
            })
            .await
            .ok()
            .flatten(),
        };
        // The camera is not connected, or its parameters cannot be read.
        let description = first
            .and_then(|f| Description::of(&f))
            .ok_or((503, "Service Unavailable"))?;
        self.selected = Some(Selected {
            name,
            tap,
            description,
        });
        self.video = None;
        self.audio = None;
        Ok(())
    }

    async fn handle(&mut self, req: &Request, out: &mut Vec<u8>) -> Outcome {
        let cseq = req.cseq.as_str();
        let session = ("Session", format!("{};timeout=60", self.session));
        let challenges = auth::challenges(&self.nonce);
        let fail = |out: &mut Vec<u8>, (code, reason): (u16, &str)| {
            let extra: Vec<(&str, String)> = if code == UNAUTHORIZED.0 {
                challenges
                    .iter()
                    .map(|c| ("WWW-Authenticate", c.clone()))
                    .collect()
            } else {
                Vec::new()
            };
            out.extend(response(code, reason, cseq, &extra, ""));
            Outcome::Continue
        };
        match req.method.as_str() {
            "OPTIONS" => out.extend(response(
                200,
                "OK",
                cseq,
                &[("Public", METHODS.to_owned())],
                "",
            )),
            "DESCRIBE" => {
                let Some(path) = parse_path(&req.uri) else {
                    return fail(out, (404, "Not Found"));
                };
                if let Err(e) = self.select(req, &path).await {
                    return fail(out, e);
                }
                let Some(selected) = &self.selected else {
                    return fail(out, (500, "Internal Server Error"));
                };
                let base = req.uri.split(['?', '#']).next().unwrap_or_default();
                let extra = [
                    ("Content-Base", format!("{}/", base.trim_end_matches('/'))),
                    ("Content-Type", "application/sdp".to_owned()),
                ];
                out.extend(response(
                    200,
                    "OK",
                    cseq,
                    &extra,
                    &selected.description.sdp(),
                ));
            }
            "SETUP" => {
                let Some(path) = parse_path(&req.uri) else {
                    return fail(out, (404, "Not Found"));
                };
                if self.playing.is_some() {
                    return fail(out, (455, "Method Not Valid in This State"));
                }
                let transport = req.header("Transport").unwrap_or_default();
                // The first transport offered that is interleaved.
                let Some(spec) = transport
                    .split(',')
                    .find(|t| t.contains("RTP/AVP/TCP") || t.contains("interleaved="))
                else {
                    return fail(out, (461, "Unsupported Transport"));
                };
                if let Err(e) = self.select(req, &path).await {
                    return fail(out, e);
                }
                let has_audio = self
                    .selected
                    .as_ref()
                    .is_some_and(|s| s.description.audio.is_some());
                let track = path.track.unwrap_or(0);
                if track > 1 || (track == 1 && !has_audio) {
                    return fail(out, (404, "Not Found"));
                }
                let channel = spec
                    .split(';')
                    .find_map(|p| p.trim().strip_prefix("interleaved="))
                    .and_then(|p| p.split('-').next())
                    .and_then(|c| c.parse::<u8>().ok())
                    .unwrap_or(track * 2);
                // Derived from the seed: distinct per track and per client.
                let ssrc = self.seed ^ (u32::from(track) + 1).wrapping_mul(0x9e37_79b9);
                let first_seq = (self.seed >> (8 * u32::from(track))) as u16;
                if track == 0 {
                    self.video = Some(Track::new(channel, VIDEO_PAYLOAD_TYPE, ssrc, first_seq));
                } else {
                    self.audio = Some(Track::new(channel, AUDIO_PAYLOAD_TYPE, ssrc, first_seq));
                }
                let extra = [
                    (
                        "Transport",
                        format!(
                            "RTP/AVP/TCP;unicast;interleaved={channel}-{};ssrc={ssrc:08X}",
                            channel.wrapping_add(1)
                        ),
                    ),
                    session,
                ];
                out.extend(response(200, "OK", cseq, &extra, ""));
            }
            "PLAY" => {
                let (Some(selected), Some(video)) = (&self.selected, &self.video) else {
                    return fail(out, (455, "Method Not Valid in This State"));
                };
                if self.playing.is_some() {
                    out.extend(response(200, "OK", cseq, &[session], ""));
                    return Outcome::Continue;
                }
                let timeline = Timeline::new(self.seed.rotate_left(13));
                let base = req.uri.split(['?', '#']).next().unwrap_or_default();
                let base = base.trim_end_matches('/');
                let mut info = format!(
                    "url={base}/trackID=0;seq={};rtptime={}",
                    video.next_seq(),
                    timeline.first()
                );
                if let (Some(audio), Some(track)) = (&self.audio, &selected.description.audio) {
                    // The sound starts with the second picture sent.
                    info.push_str(&format!(
                        ",url={base}/trackID=1;seq={};rtptime={}",
                        audio.next_seq(),
                        (i64::from(timeline.first()) * i64::from(track.params.clock_rate)
                            / i64::from(VIDEO_CLOCK)) as u32
                    ));
                }
                let extra = [
                    session,
                    ("Range", "npt=0.000-".to_owned()),
                    ("RTP-Info", info),
                ];
                out.extend(response(200, "OK", cseq, &extra, ""));
                let (backlog, frames) = selected.tap.subscribe();
                tracing::info!(peer = %self.peer, stream = %selected.name, "RTSP client playing");
                self.playing_name = Some(selected.name.clone());
                self.playing = Some(Playing {
                    frames,
                    timeline,
                    started: false,
                    sent_any: false,
                    next_report: Instant::now(),
                });
                // What the camera sent since its last key frame: a picture at once.
                for f in &backlog {
                    if !self.packets(f, out) {
                        return Outcome::Close;
                    }
                }
            }
            "GET_PARAMETER" | "SET_PARAMETER" => {
                out.extend(response(200, "OK", cseq, &[session], ""));
            }
            "TEARDOWN" => {
                out.extend(response(200, "OK", cseq, &[session], ""));
                return Outcome::Close;
            }
            "PAUSE" | "RECORD" | "ANNOUNCE" => {
                return fail(out, (405, "Method Not Allowed"));
            }
            _ => return fail(out, (501, "Not Implemented")),
        }
        Outcome::Continue
    }

    /// Appends the packets of `frame` (and of its sound); false when the
    /// frame no longer matches what was described to the client.
    fn packets(&mut self, frame: &VideoFrame, out: &mut Vec<u8>) -> bool {
        let (Some(selected), Some(video), Some(playing)) =
            (&self.selected, &mut self.video, &mut self.playing)
        else {
            return true;
        };
        let description = &selected.description;
        if !Arc::ptr_eq(&frame.params, &description.video) && *frame.params != *description.video {
            return false;
        }
        if !playing.started {
            if !frame.is_key {
                return true;
            }
            playing.started = true;
        }
        let timestamp = playing.timeline.video(frame);
        let report = playing.next_report <= Instant::now();
        if report {
            playing.next_report = Instant::now() + REPORT_INTERVAL;
            video.sender_report(out, frame.wall_time, timestamp);
        }
        let sets: &[Vec<u8>] = if frame.is_key {
            &description.parameter_sets
        } else {
            &[]
        };
        video_packets(video, out, frame, sets, timestamp);
        if let (Some(track), Some(audio), Some(sound)) =
            (&mut self.audio, &description.audio, &frame.audio)
        {
            let rate = audio.params.clock_rate;
            if report {
                track.sender_report(out, frame.wall_time, playing.timeline.video_as_audio(rate));
            }
            // The sound that came with the first picture is older than it.
            if playing.sent_any && sound.params.rfc6381_codec == audio.params.rfc6381_codec {
                for sample in &sound.samples {
                    let ts = playing.timeline.audio(sample.pts, rate);
                    audio_packets(track, out, audio.kind, &sample.data, ts);
                }
            }
        }
        playing.sent_any = true;
        true
    }
}

/// Writes `data`, giving up on a client that stopped reading.
async fn write(stream: &mut TcpStream, data: &[u8]) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(data))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "client stopped reading"))?
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn reads_stream_addresses() {
        let p = |camera: &str, sub, track| {
            Some(StreamPath {
                camera: camera.to_owned(),
                sub,
                track,
            })
        };
        assert_eq!(
            parse_path("rtsp://nvr:8554/jardin"),
            p("jardin", false, None)
        );
        assert_eq!(
            parse_path("rtsp://nvr:8554/jardin/"),
            p("jardin", false, None)
        );
        assert_eq!(
            parse_path("rtsp://nvr/jardin/sub?x=1"),
            p("jardin", true, None)
        );
        assert_eq!(
            parse_path("rtsp://[::1]:8554/jardin/sub/trackID=1"),
            p("jardin", true, Some(1))
        );
        assert_eq!(
            parse_path("rtsp://nvr/jardin/trackID=0"),
            p("jardin", false, Some(0))
        );
        assert_eq!(parse_path("rtsp://nvr:8554/"), None);
        assert_eq!(parse_path("rtsp://nvr/a/b/c"), None);
        assert_eq!(parse_path("rtsp://nvr/a/trackID=x"), None);
        assert_eq!(parse_path("*"), None);
    }

    #[test]
    fn reads_requests_and_skips_interleaved_data() {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(
            b"$\x01\x00\x02ab\r\nOPTIONS * RTSP/1.0\r\ncseq: 7\r\nUser-Agent: x\r\n\r\nDESC",
        );
        let r = parse_request(&mut buf).unwrap().unwrap();
        assert_eq!(
            (r.method.as_str(), r.uri.as_str(), r.cseq.as_str()),
            ("OPTIONS", "*", "7")
        );
        assert_eq!(r.header("user-agent"), Some("x"));
        // Incomplete: wait for more.
        assert!(parse_request(&mut buf).unwrap().is_none());
        assert_eq!(&buf[..], b"DESC");

        let mut http = BytesMut::from(&b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"[..]);
        assert!(parse_request(&mut http).is_err());
        let mut huge = BytesMut::from(vec![b'a'; MAX_REQUEST + 1].as_slice());
        assert!(parse_request(&mut huge).is_err());
    }

    #[test]
    fn responses_have_a_length_only_with_a_body() {
        let r = String::from_utf8(response(200, "OK", "3", &[("Public", "X".into())], "")).unwrap();
        assert!(r.starts_with("RTSP/1.0 200 OK\r\nCSeq: 3\r\nServer: RustNVR/"));
        assert!(r.ends_with("Public: X\r\n\r\n"));
        let r = String::from_utf8(response(200, "OK", "3", &[], "v=0\r\n")).unwrap();
        assert!(r.ends_with("Content-Length: 5\r\n\r\nv=0\r\n"));
    }
}
