//! What RTSP republication costs: a synthetic camera (25 pictures per
//! second, a key frame every 2 s) at a given bit rate is served to 0, 1, 4
//! and 16 clients, and the processor time and memory of this process (the
//! server alone: clients are other processes that read and discard) are
//! printed for each.
//!
//! `cargo run --release -p rustnvr-restream --example cost [-- <Mbit/s>]`
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use bytes::Bytes;
use futures::FutureExt as _;
use rustnvr_core::{VideoCodec, VideoFrame, VideoParams};
use rustnvr_restream::{Lookup, Options, RestreamServer, Source};
use rustnvr_stream::LiveTap;
use std::io::{Read as _, Write as _};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const FPS: u64 = 25;
const GOP: u64 = 50;
const MEASURE: Duration = Duration::from_secs(10);

fn params() -> Arc<VideoParams> {
    // An avc1 sample entry with one SPS and one PPS.
    let avcc = [
        1, 0x64, 0x00, 0x28, 0xff, 0xe1, 0, 4, 0x67, 0x64, 0x00, 0x28, 1, 0, 1, 0x68,
    ];
    let mut entry = vec![0u8; 86];
    entry.extend_from_slice(&((8 + avcc.len()) as u32).to_be_bytes());
    entry.extend_from_slice(b"avcC");
    entry.extend_from_slice(&avcc);
    Arc::new(VideoParams {
        codec: VideoCodec::H264,
        rfc6381_codec: "avc1.640028".into(),
        width: 2560,
        height: 1440,
        clock_rate: 90_000,
        sample_entry: Bytes::from(entry),
    })
}

/// One picture of `size` bytes.
fn picture(size: usize, key: bool) -> Bytes {
    let mut data = (size as u32).to_be_bytes().to_vec();
    data.push(if key { 0x65 } else { 0x41 });
    data.extend((1..size).map(|i| (i % 251) as u8 | 1));
    Bytes::from(data)
}

/// Publishes the synthetic camera forever.
async fn camera(tap: LiveTap, bits_per_second: u64) {
    // Key frames are ten times the size of the others.
    let per_gop = bits_per_second / 8 * GOP / FPS;
    let small = (per_gop / (GOP + 9)) as usize;
    let (key, other) = (picture(small * 10, true), picture(small, false));
    let params = params();
    let mut tick = tokio::time::interval(Duration::from_millis(1000 / FPS));
    let mut seq: u64 = 0;
    loop {
        tick.tick().await;
        let is_key = seq.is_multiple_of(GOP);
        tap.publish(&VideoFrame {
            session: 1,
            seq,
            params: params.clone(),
            pts: (seq * 90_000 / FPS) as i64,
            is_key,
            wall_time: SystemTime::now(),
            loss: 0,
            data: if is_key { key.clone() } else { other.clone() },
            audio: None,
        });
        seq += 1;
    }
}

/// A client: plays the stream and discards what it receives.
fn client(addr: &str) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    let url = format!("rtsp://{addr}/cam");
    let mut buf = vec![0u8; 256 * 1024];
    for (i, request) in [
        format!("DESCRIBE {url} RTSP/1.0\r\nCSeq: 1\r\n\r\n"),
        format!("SETUP {url}/trackID=0 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n"),
        format!("PLAY {url} RTSP/1.0\r\nCSeq: 3\r\n\r\n"),
    ]
    .iter()
    .enumerate()
    {
        s.write_all(request.as_bytes()).unwrap();
        if i < 2 {
            let n = s.read(&mut buf).unwrap();
            assert!(buf[..n].starts_with(b"RTSP/1.0 200"), "request {i} refused");
        }
    }
    while s.read(&mut buf).is_ok_and(|n| n > 0) {}
}

/// Processor time used by this process (user + system), and its resident
/// memory in MiB.
fn usage() -> (Duration, f64) {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    // Fields after the command name (which may contain spaces).
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    let ticks: u64 = fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap();
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let rss_kb: f64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap();
    // The kernel reports times in 1/100 s.
    (Duration::from_millis(ticks * 10), rss_kb / 1024.0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("client") {
        client(&args[2]);
        return;
    }
    let mbits: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(4);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let tap = LiveTap::new();
        tokio::spawn(camera(tap.clone(), mbits * 1_000_000));
        let source = tap.clone();
        let lookup: Lookup = Arc::new(move |camera: String, sub: bool| {
            let tap = (camera == "cam" && !sub).then(|| source.clone());
            async move {
                Source {
                    tap,
                    credentials: None,
                }
            }
            .boxed()
        });
        let server = RestreamServer::start(
            "127.0.0.1:0".parse().unwrap(),
            lookup,
            Options {
                max_clients: 64,
                max_clients_per_address: 64,
            },
        )
        .await
        .unwrap();
        let addr = server.local_addr().to_string();
        println!(
            "camera: {mbits} Mbit/s, {FPS} pictures per second, key frame every {} s",
            GOP / FPS
        );
        println!(
            "{:>8} {:>18} {:>12}",
            "clients", "CPU (% of a core)", "memory (MiB)"
        );
        let mut clients = Vec::new();
        for count in [0usize, 1, 4, 16] {
            while clients.len() < count {
                clients.push(
                    std::process::Command::new(std::env::current_exe().unwrap())
                        .args(["client", &addr])
                        .spawn()
                        .unwrap(),
                );
            }
            // Let the clients start and the memory settle.
            tokio::time::sleep(Duration::from_secs(3)).await;
            assert_eq!(tap.viewers(), count, "clients connected");
            let (before, _) = usage();
            tokio::time::sleep(MEASURE).await;
            let (after, rss) = usage();
            let cpu = (after - before).as_secs_f64() / MEASURE.as_secs_f64() * 100.0;
            println!("{count:>8} {cpu:>18.1} {rss:>12.1}");
        }
        for mut c in clients {
            let _ = c.kill();
            let _ = c.wait();
        }
        server.shutdown().await;
    });
}
