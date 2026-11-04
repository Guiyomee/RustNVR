//! Live view over WebRTC (prototype): one H.264 stream per viewer, sent with
//! str0m (sans-IO WebRTC) from a [`LiveTap`], without re-encoding.
//!
//! Signaling follows WHEP: the browser POSTs an SDP offer (receive-only
//! video), the server answers with its SDP and only host ICE candidates (no
//! STUN/TURN: the viewer must reach this machine directly, typically on the
//! local network). Each viewer gets its own UDP socket on an ephemeral port.
//!
//! H.264 only (browsers do not take H.265 over WebRTC): for H.265 cameras the
//! caller passes the tap of the H.264 conversion.

use bytes::Bytes;
use rustnvr_core::{VideoCodec, VideoFrame};
use rustnvr_stream::LiveTap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use str0m::change::SdpOffer;
use str0m::format::Codec;
use str0m::media::{MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

/// A session that does not connect within this delay is dropped.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Concatenated parameter sets (SPS, PPS) of an `avc1` sample entry, in
/// Annex B form, to send before each key frame.
pub fn h264_parameter_sets(sample_entry: &[u8]) -> Option<Vec<u8>> {
    // avc1 entry: 8-byte box header, 78-byte visual sample entry, then child
    // boxes including avcC.
    let children = sample_entry.get(86..)?;
    let mut pos = 0;
    let avcc = loop {
        let size = u32::from_be_bytes(children.get(pos..pos + 4)?.try_into().ok()?) as usize;
        if children.get(pos + 4..pos + 8)? == b"avcC" {
            break children.get(pos + 8..pos + size)?;
        }
        if size < 8 {
            return None;
        }
        pos += size;
    };
    let mut out = Vec::new();
    let mut i = 5;
    let sps_count = avcc.get(i)? & 0x1f;
    i += 1;
    for _ in 0..sps_count {
        let len = u16::from_be_bytes(avcc.get(i..i + 2)?.try_into().ok()?) as usize;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(avcc.get(i + 2..i + 2 + len)?);
        i += 2 + len;
    }
    let pps_count = *avcc.get(i)?;
    i += 1;
    for _ in 0..pps_count {
        let len = u16::from_be_bytes(avcc.get(i..i + 2)?.try_into().ok()?) as usize;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(avcc.get(i + 2..i + 2 + len)?);
        i += 2 + len;
    }
    Some(out)
}

/// A length-prefixed (4 bytes) access unit converted to Annex B.
pub fn to_annex_b(mut data: &[u8], prefix: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + data.len() + 16);
    out.extend_from_slice(prefix);
    while data.len() > 4 {
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let Some(nal) = data.get(4..4 + len) else {
            break;
        };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        data = &data[4 + len..];
    }
    out
}

/// Starts a viewer session for `offer` (SDP); returns the SDP answer. The
/// session ends when the viewer leaves, the stream ends or `cancel` fires.
pub async fn start_session(
    offer: &str,
    source: LiveTap,
    host_ips: &[IpAddr],
    cancel: CancellationToken,
) -> Result<String, String> {
    let offer =
        SdpOffer::from_sdp_string(offer).map_err(|e| format!("offre SDP invalide : {e}"))?;
    let mut rtc = RtcConfig::new()
        .clear_codecs()
        .enable_h264(true)
        .build(Instant::now());
    // One socket per address: str0m needs the exact local address of each
    // received packet, which a socket bound to 0.0.0.0 does not give.
    let mut sockets = Vec::new();
    let mut candidates = Vec::new();
    for ip in host_ips {
        let socket = UdpSocket::bind(SocketAddr::new(*ip, 0))
            .await
            .map_err(|e| format!("socket UDP {ip} : {e}"))?;
        let addr = socket.local_addr().map_err(|e| e.to_string())?;
        sockets.push(Arc::new(socket));
        let c = Candidate::host(addr, "udp").map_err(|e| format!("candidat ICE {addr} : {e}"))?;
        rtc.add_local_candidate(c);
        candidates.push(addr);
    }
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| format!("offre refusée : {e}"))?;
    tracing::info!(candidates = ?candidates, "WebRTC viewer session starting");
    tokio::spawn(async move {
        if let Err(e) = run(rtc, sockets, source, cancel).await {
            tracing::debug!(error = %e, "WebRTC session ended with an error");
        }
        tracing::info!("WebRTC viewer session ended");
    });
    Ok(answer.to_sdp_string())
}

struct Sender {
    mid: Option<Mid>,
    pt: Option<Pt>,
    connected: bool,
    /// Sending started (from a key frame).
    started: bool,
}

async fn run(
    mut rtc: Rtc,
    sockets: Vec<Arc<UdpSocket>>,
    source: LiveTap,
    cancel: CancellationToken,
) -> Result<(), String> {
    // Each socket forwards its datagrams with its own address.
    let (packets_tx, mut packets) =
        tokio::sync::mpsc::channel::<(Vec<u8>, SocketAddr, SocketAddr)>(256);
    let mut readers = tokio::task::JoinSet::new();
    for socket in &sockets {
        let (socket, tx) = (socket.clone(), packets_tx.clone());
        readers.spawn(async move {
            let local = socket.local_addr()?;
            let mut buf = vec![0u8; 2000];
            loop {
                let (n, from) = socket.recv_from(&mut buf).await?;
                if tx.send((buf[..n].to_vec(), from, local)).await.is_err() {
                    return Ok::<(), std::io::Error>(());
                }
            }
        });
    }
    drop(packets_tx);
    let (_, mut frames) = source.subscribe();
    let mut s = Sender {
        mid: None,
        pt: None,
        connected: false,
        started: false,
    };
    let started_at = Instant::now();

    loop {
        // Drain what str0m wants to do now.
        let deadline = loop {
            match rtc.poll_output().map_err(|e| e.to_string())? {
                Output::Timeout(t) => break t,
                Output::Transmit(t) => {
                    if let Some(s) = sockets
                        .iter()
                        .find(|s| s.local_addr().is_ok_and(|a| a == t.source))
                    {
                        let _ = s.send_to(&t.contents, t.destination).await;
                    }
                }
                Output::Event(e) => match e {
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        return Ok(());
                    }
                    Event::Connected => s.connected = true,
                    Event::MediaAdded(m) if m.kind == MediaKind::Video => {
                        s.mid = Some(m.mid);
                        s.pt = rtc.writer(m.mid).and_then(|w| {
                            w.payload_params()
                                .find(|p| p.spec().codec == Codec::H264)
                                .map(|p| p.pt())
                        });
                    }
                    _ => {}
                },
            }
        };
        if !rtc.is_alive() {
            return Ok(());
        }
        if !s.connected && started_at.elapsed() > CONNECT_TIMEOUT {
            return Err("viewer never connected".to_owned());
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        tokio::select! {
            _ = cancel.cancelled() => {
                rtc.disconnect();
                return Ok(());
            }
            _ = tokio::time::sleep(wait) => {
                rtc.handle_input(Input::Timeout(Instant::now())).map_err(|e| e.to_string())?;
            }
            received = packets.recv() => {
                let Some((data, source, destination)) = received else {
                    return Err("UDP sockets closed".to_owned());
                };
                let Ok(contents) = data.as_slice().try_into() else { continue };
                let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination, contents });
                if rtc.accepts(&input) {
                    rtc.handle_input(input).map_err(|e| e.to_string())?;
                }
            }
            frame = frames.recv() => match frame {
                Ok(f) => send_frame(&mut rtc, &mut s, &f)?,
                // Too slow: start again at the next key frame.
                Err(RecvError::Lagged(_)) => s.started = false,
                Err(RecvError::Closed) => return Ok(()),
            },
        }
    }
}

fn send_frame(rtc: &mut Rtc, s: &mut Sender, f: &VideoFrame) -> Result<(), String> {
    let (Some(mid), Some(pt)) = (s.mid, s.pt) else {
        return Ok(());
    };
    if !s.connected || f.params.codec != VideoCodec::H264 {
        return Ok(());
    }
    if !s.started {
        if !f.is_key {
            return Ok(());
        }
        s.started = true;
    }
    let prefix = if f.is_key {
        h264_parameter_sets(&f.params.sample_entry).unwrap_or_default()
    } else {
        Vec::new()
    };
    let data: Bytes = to_annex_b(&f.data, &prefix).into();
    let clock = u64::from(f.params.clock_rate.max(1));
    let rtp = MediaTime::from_90khz(u64::try_from(f.pts).unwrap_or(0) * 90_000 / clock);
    let Some(writer) = rtc.writer(mid) else {
        return Ok(());
    };
    writer
        .write(pt, Instant::now(), rtp, data.to_vec())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_access_units_to_annex_b() {
        let au = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x06];
        assert_eq!(
            to_annex_b(&au, &[9]),
            vec![9, 0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x06]
        );
        // avc1 entry with an avcC carrying one SPS [0x67, 1] and one PPS [0x68].
        let mut entry = vec![0u8; 86];
        let avcc = [
            1, 0x42, 0xc0, 0x1e, 0xff, 0xe1, 0, 2, 0x67, 1, 1, 0, 1, 0x68,
        ];
        entry.extend_from_slice(&((8 + avcc.len()) as u32).to_be_bytes());
        entry.extend_from_slice(b"avcC");
        entry.extend_from_slice(&avcc);
        assert_eq!(
            h264_parameter_sets(&entry),
            Some(vec![0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68])
        );
        assert_eq!(h264_parameter_sets(&[0; 10]), None);
    }
}
