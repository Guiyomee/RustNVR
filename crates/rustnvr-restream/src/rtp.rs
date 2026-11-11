//! RTP packets of the republished streams, written as RTSP interleaved
//! frames (`$`, channel, length, packet): H.264 (RFC 6184), H.265
//! (RFC 7798), AAC (RFC 3640, AAC-hbr) and Opus (RFC 7587).

use crate::describe::{AudioKind, VIDEO_CLOCK};
use rustnvr_core::{VideoCodec, VideoFrame};
use std::time::{SystemTime, UNIX_EPOCH};

/// Largest RTP payload produced (the packet fits a 1500-byte Ethernet frame).
pub const MAX_PAYLOAD: usize = 1400;

/// Seconds between 1900 (NTP) and 1970 (Unix).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// Sequence numbers and identity of one RTP stream, written on one
/// interleaved channel.
#[derive(Debug)]
pub struct Track {
    /// Interleaved channel of the RTP packets (RTCP goes on the next one).
    pub channel: u8,
    payload_type: u8,
    ssrc: u32,
    seq: u16,
    packets: u32,
    octets: u32,
}

impl Track {
    /// A track sending `payload_type` on `channel`.
    pub fn new(channel: u8, payload_type: u8, ssrc: u32, first_seq: u16) -> Self {
        Self {
            channel,
            payload_type,
            ssrc,
            seq: first_seq,
            packets: 0,
            octets: 0,
        }
    }

    /// Sequence number of the next packet.
    pub fn next_seq(&self) -> u16 {
        self.seq
    }

    /// Synchronization source identifier.
    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    /// Appends one packet whose payload is the concatenation of `parts`.
    fn packet(&mut self, out: &mut Vec<u8>, marker: bool, timestamp: u32, parts: &[&[u8]]) {
        let payload: usize = parts.iter().map(|p| p.len()).sum();
        let Ok(len) = u16::try_from(12 + payload) else {
            return;
        };
        out.push(b'$');
        out.push(self.channel);
        out.extend_from_slice(&len.to_be_bytes());
        out.push(0x80);
        out.push((u8::from(marker) << 7) | self.payload_type);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&timestamp.to_be_bytes());
        out.extend_from_slice(&self.ssrc.to_be_bytes());
        for p in parts {
            out.extend_from_slice(p);
        }
        self.seq = self.seq.wrapping_add(1);
        self.packets = self.packets.wrapping_add(1);
        self.octets = self.octets.wrapping_add(payload as u32);
    }

    /// Appends an RTCP sender report saying that `timestamp` was sampled at
    /// `wall`: clients line up sound and picture with it.
    pub fn sender_report(&self, out: &mut Vec<u8>, wall: SystemTime, timestamp: u32) {
        let since = wall.duration_since(UNIX_EPOCH).unwrap_or_default();
        let ntp_secs = (since.as_secs() + NTP_UNIX_OFFSET) as u32;
        let ntp_frac = ((u64::from(since.subsec_nanos()) << 32) / 1_000_000_000) as u32;
        out.push(b'$');
        out.push(self.channel.wrapping_add(1));
        out.extend_from_slice(&28u16.to_be_bytes());
        // Version 2, no report block; sender report; length in words minus one.
        out.extend_from_slice(&[0x80, 200, 0, 6]);
        out.extend_from_slice(&self.ssrc.to_be_bytes());
        out.extend_from_slice(&ntp_secs.to_be_bytes());
        out.extend_from_slice(&ntp_frac.to_be_bytes());
        out.extend_from_slice(&timestamp.to_be_bytes());
        out.extend_from_slice(&self.packets.to_be_bytes());
        out.extend_from_slice(&self.octets.to_be_bytes());
    }
}

/// The NAL units of a length-prefixed (4 bytes) access unit.
fn nals(mut data: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        while data.len() > 4 {
            let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
            let nal = data.get(4..4 + len)?;
            data = &data[4 + len..];
            if !nal.is_empty() {
                return Some(nal);
            }
        }
        None
    })
}

/// Appends the packets of one NAL unit: alone when it fits, in fragments
/// (FU-A / FU) otherwise.
fn nal_packets(
    track: &mut Track,
    out: &mut Vec<u8>,
    codec: VideoCodec,
    nal: &[u8],
    last: bool,
    timestamp: u32,
) {
    if nal.len() <= MAX_PAYLOAD {
        track.packet(out, last, timestamp, &[nal]);
        return;
    }
    // Fragment header without the start and end bits, and the NAL without
    // its own header (rebuilt by the receiver).
    let (header, body): (Vec<u8>, &[u8]) = match codec {
        VideoCodec::H264 => (vec![(nal[0] & 0xe0) | 28, nal[0] & 0x1f], &nal[1..]),
        VideoCodec::H265 => (
            vec![(nal[0] & 0x81) | (49 << 1), nal[1], (nal[0] >> 1) & 0x3f],
            &nal[2..],
        ),
    };
    let size = MAX_PAYLOAD - header.len();
    let count = body.len().div_ceil(size);
    for (i, chunk) in body.chunks(size).enumerate() {
        let mut h = header.clone();
        let end = i + 1 == count;
        if let Some(bits) = h.last_mut() {
            *bits |= (u8::from(i == 0) << 7) | (u8::from(end) << 6);
        }
        track.packet(out, last && end, timestamp, &[&h, chunk]);
    }
}

/// Appends the packets of a picture. `parameter_sets` are sent first (before
/// key frames: a client joining there needs them).
pub fn video_packets(
    track: &mut Track,
    out: &mut Vec<u8>,
    frame: &VideoFrame,
    parameter_sets: &[Vec<u8>],
    timestamp: u32,
) {
    let codec = frame.params.codec;
    let mut units: Vec<&[u8]> = parameter_sets.iter().map(Vec::as_slice).collect();
    units.extend(nals(&frame.data));
    let count = units.len();
    for (i, nal) in units.into_iter().enumerate() {
        nal_packets(track, out, codec, nal, i + 1 == count, timestamp);
    }
}

/// Appends the packets of one audio frame.
pub fn audio_packets(
    track: &mut Track,
    out: &mut Vec<u8>,
    kind: AudioKind,
    data: &[u8],
    timestamp: u32,
) {
    match kind {
        AudioKind::Opus => track.packet(out, true, timestamp, &[data]),
        AudioKind::Aac => {
            // One AU header (16 bits: 13 of size, 3 of index) after the
            // 16-bit length, in bits, of the AU headers. A frame too large
            // for one packet is split; each part repeats the header.
            let Ok(size) = u16::try_from(data.len()) else {
                return;
            };
            if size >= 1 << 13 {
                return;
            }
            let header = [0, 16, (size >> 5) as u8, (size << 3) as u8];
            let count = data.len().div_ceil(MAX_PAYLOAD - 4).max(1);
            for (i, chunk) in data.chunks(MAX_PAYLOAD - 4).enumerate() {
                track.packet(out, i + 1 == count, timestamp, &[&header, chunk]);
            }
        }
    }
}

/// RTP timestamps of a client: continuous through the reconnections of the
/// camera (whose timestamps start again at each session).
#[derive(Debug)]
pub struct Timeline {
    first: u32,
    session: Option<u64>,
    /// Added to the camera's timestamps, in [`VIDEO_CLOCK`] units.
    offset: i64,
    /// Last video timestamp given, before truncation.
    last: i64,
    /// Last distance between two pictures.
    step: i64,
}

impl Timeline {
    /// A timeline whose first picture is at `first`.
    pub fn new(first: u32) -> Self {
        Self {
            first,
            session: None,
            offset: 0,
            last: 0,
            step: i64::from(VIDEO_CLOCK / 25),
        }
    }

    /// RTP timestamp of the first picture.
    pub fn first(&self) -> u32 {
        self.first
    }

    fn scaled(pts: i64, from: u32, to: u32) -> i64 {
        (i128::from(pts) * i128::from(to) / i128::from(from.max(1))) as i64
    }

    /// RTP timestamp of `frame`.
    pub fn video(&mut self, frame: &VideoFrame) -> u32 {
        let pts = Self::scaled(frame.pts, frame.params.clock_rate, VIDEO_CLOCK);
        match self.session {
            None => self.offset = i64::from(self.first) - pts,
            // The camera reconnected: go on one picture later.
            Some(s) if s != frame.session => self.offset = self.last + self.step - pts,
            Some(_) => {
                let step = pts + self.offset - self.last;
                if (1..i64::from(VIDEO_CLOCK)).contains(&step) {
                    self.step = step;
                }
            }
        }
        self.session = Some(frame.session);
        self.last = pts + self.offset;
        self.last as u32
    }

    /// RTP timestamp of an audio frame at `pts` (in `rate` units, on the
    /// timeline of the pictures of the session last given to
    /// [`Self::video`]).
    pub fn audio(&self, pts: i64, rate: u32) -> u32 {
        (pts + Self::scaled(self.offset, VIDEO_CLOCK, rate)) as u32
    }

    /// The instant of the last picture, on the clock of an audio track.
    pub fn video_as_audio(&self, rate: u32) -> u32 {
        Self::scaled(self.last, VIDEO_CLOCK, rate) as u32
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::describe::tests::{frame, h264_params};
    use bytes::Bytes;

    /// Splits interleaved frames: (channel, packet).
    fn split(mut data: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        while !data.is_empty() {
            assert_eq!(data[0], b'$');
            let len = usize::from(u16::from_be_bytes([data[2], data[3]]));
            out.push((data[1], data[4..4 + len].to_vec()));
            data = &data[4 + len..];
        }
        out
    }

    fn access_unit(nals: &[Vec<u8>]) -> Bytes {
        let mut d = Vec::new();
        for n in nals {
            d.extend_from_slice(&(n.len() as u32).to_be_bytes());
            d.extend_from_slice(n);
        }
        Bytes::from(d)
    }

    #[test]
    fn h264_picture_with_parameter_sets_and_fragments() {
        let mut big = vec![0x65];
        big.extend(std::iter::repeat_n(0xab, 3000));
        let mut f = frame(h264_params(), None);
        f.data = access_unit(&[vec![0x06, 1], big.clone()]);
        let mut track = Track::new(2, 96, 0xdead_beef, 65_534);
        let mut out = Vec::new();
        video_packets(&mut track, &mut out, &f, &[vec![0x67, 1], vec![0x68]], 9000);
        let packets = split(&out);
        // SPS, PPS, SEI, then the IDR slice in three fragments.
        assert_eq!(packets.len(), 6);
        assert!(packets.iter().all(|(c, _)| *c == 2));
        let seqs: Vec<u16> = packets
            .iter()
            .map(|(_, p)| u16::from_be_bytes([p[2], p[3]]))
            .collect();
        assert_eq!(seqs, vec![65_534, 65_535, 0, 1, 2, 3]);
        for (i, (_, p)) in packets.iter().enumerate() {
            assert_eq!(p[0], 0x80);
            assert_eq!(p[1] & 0x7f, 96);
            assert_eq!(p[1] >> 7 == 1, i == 5, "marker on the last packet only");
            assert_eq!(u32::from_be_bytes(p[4..8].try_into().unwrap()), 9000);
            assert_eq!(
                u32::from_be_bytes(p[8..12].try_into().unwrap()),
                0xdead_beef
            );
        }
        assert_eq!(&packets[0].1[12..], &[0x67, 1]);
        assert_eq!(&packets[2].1[12..], &[0x06, 1]);
        // FU-A: indicator keeps the NRI, header carries the type.
        assert_eq!(packets[3].1[12], 0x60 | 28);
        assert_eq!(packets[3].1[13], 0x80 | 5);
        assert_eq!(packets[4].1[13], 5);
        assert_eq!(packets[5].1[13], 0x40 | 5);
        let body: Vec<u8> = packets[3..]
            .iter()
            .flat_map(|(_, p)| p[14..].to_vec())
            .collect();
        assert_eq!(body, big[1..]);
        assert!(packets.iter().all(|(_, p)| p.len() <= 12 + MAX_PAYLOAD));
    }

    #[test]
    fn h265_fragments() {
        let mut big = vec![19 << 1, 1];
        big.extend(std::iter::repeat_n(0xcd, 2000));
        let mut params = h264_params();
        params.codec = VideoCodec::H265;
        let mut f = frame(params, None);
        f.data = access_unit(&[big.clone()]);
        let mut track = Track::new(0, 96, 1, 0);
        let mut out = Vec::new();
        video_packets(&mut track, &mut out, &f, &[], 0);
        let packets = split(&out);
        assert_eq!(packets.len(), 2);
        assert_eq!((packets[0].1[12] >> 1) & 0x3f, 49);
        assert_eq!(packets[0].1[13], 1);
        assert_eq!(packets[0].1[14], 0x80 | 19);
        assert_eq!(packets[1].1[14], 0x40 | 19);
        assert_eq!(packets[1].1[1] >> 7, 1);
        let body: Vec<u8> = packets.iter().flat_map(|(_, p)| p[15..].to_vec()).collect();
        assert_eq!(body, big[2..]);
    }

    #[test]
    fn audio_frames() {
        let mut track = Track::new(2, 97, 7, 10);
        let mut out = Vec::new();
        audio_packets(&mut track, &mut out, AudioKind::Aac, &[1, 2, 3], 1024);
        audio_packets(&mut track, &mut out, AudioKind::Opus, &[9, 9], 960);
        let packets = split(&out);
        assert_eq!(packets.len(), 2);
        // AU headers: 16 bits, then size 3 on 13 bits and index 0.
        assert_eq!(&packets[0].1[12..], &[0, 16, 0, 3 << 3, 1, 2, 3]);
        assert_eq!(packets[0].1[1], 0x80 | 97);
        assert_eq!(&packets[1].1[12..], &[9, 9]);
    }

    #[test]
    fn sender_report_layout() {
        let mut track = Track::new(4, 96, 0x0102_0304, 0);
        let mut out = Vec::new();
        track.packet(&mut out, true, 0, &[&[0; 10]]);
        out.clear();
        let wall = UNIX_EPOCH + std::time::Duration::from_millis(1_500);
        track.sender_report(&mut out, wall, 4242);
        let packets = split(&out);
        assert_eq!(packets[0].0, 5, "RTCP on the next channel");
        let p = &packets[0].1;
        assert_eq!(p.len(), 28);
        assert_eq!(&p[..8], &[0x80, 200, 0, 6, 1, 2, 3, 4]);
        assert_eq!(
            u32::from_be_bytes(p[8..12].try_into().unwrap()),
            NTP_UNIX_OFFSET as u32 + 1
        );
        assert_eq!(u32::from_be_bytes(p[12..16].try_into().unwrap()), 1 << 31);
        assert_eq!(u32::from_be_bytes(p[16..20].try_into().unwrap()), 4242);
        assert_eq!(u32::from_be_bytes(p[20..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_be_bytes(p[24..28].try_into().unwrap()), 10);
    }

    #[test]
    fn timeline_survives_camera_reconnections() {
        let mut t = Timeline::new(1_000_000);
        let mut f = frame(h264_params(), None);
        f.pts = 500_000;
        assert_eq!(t.video(&f), 1_000_000);
        f.pts += 3600;
        assert_eq!(t.video(&f), 1_003_600);
        // Sound 0.1 s after that picture, at 16 kHz.
        let audio_pts = (f.pts + 9000) * 16_000 / 90_000;
        let expected = (1_003_600 + 9000) * 16_000 / 90_000;
        assert!((i64::from(t.audio(audio_pts, 16_000)) - expected).abs() <= 1);
        // New session: timestamps start again at the camera, not for the client.
        f.session = 2;
        f.pts = 0;
        assert_eq!(t.video(&f), 1_007_200);
        f.pts = 3000;
        assert_eq!(t.video(&f), 1_010_200);
    }
}
