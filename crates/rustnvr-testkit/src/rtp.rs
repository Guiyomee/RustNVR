//! RTP packetization of H.264 (RFC 6184) and H.265 (RFC 7798) access units.

use crate::annexb::{AccessUnit, Codec};

/// Maximum RTP payload size produced.
pub const MAX_PAYLOAD: usize = 1200;

/// Builds the RTP payloads of one access unit: single NAL unit packets when
/// the NAL fits, fragmentation units (FU-A / FU) otherwise.
pub fn payloads(codec: Codec, au: &AccessUnit) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for nal in &au.nals {
        if nal.len() <= MAX_PAYLOAD {
            out.push(nal.to_vec());
            continue;
        }
        match codec {
            Codec::H264 => {
                let indicator = (nal[0] & 0xe0) | 28;
                let nal_type = nal[0] & 0x1f;
                fragment(&nal[1..], MAX_PAYLOAD - 2, |chunk, start, end| {
                    let mut p = Vec::with_capacity(chunk.len() + 2);
                    let header = (u8::from(start) << 7) | (u8::from(end) << 6) | nal_type;
                    p.extend_from_slice(&[indicator, header]);
                    p.extend_from_slice(chunk);
                    out.push(p);
                });
            }
            Codec::H265 => {
                let nal_type = (nal[0] >> 1) & 0x3f;
                let ph0 = (nal[0] & 0x81) | (49 << 1);
                let ph1 = nal[1];
                fragment(&nal[2..], MAX_PAYLOAD - 3, |chunk, start, end| {
                    let mut p = Vec::with_capacity(chunk.len() + 3);
                    let header = (u8::from(start) << 7) | (u8::from(end) << 6) | nal_type;
                    p.extend_from_slice(&[ph0, ph1, header]);
                    p.extend_from_slice(chunk);
                    out.push(p);
                });
            }
        }
    }
    out
}

fn fragment(data: &[u8], max: usize, mut emit: impl FnMut(&[u8], bool, bool)) {
    let n = data.len().div_ceil(max);
    for (i, chunk) in data.chunks(max).enumerate() {
        emit(chunk, i == 0, i + 1 == n);
    }
}

/// Serializes an RTP packet (version 2, no padding, extension or CSRC).
pub fn packet(
    payload_type: u8,
    marker: bool,
    seq: u16,
    timestamp: u32,
    ssrc: u32,
    payload: &[u8],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + payload.len());
    p.push(0x80);
    p.push((u8::from(marker) << 7) | (payload_type & 0x7f));
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&timestamp.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn fragments_large_h264_nal() {
        let mut nal = vec![0x65];
        nal.extend(std::iter::repeat_n(0xab, 3000));
        let au = AccessUnit {
            nals: vec![Bytes::from(nal)],
            is_key: true,
        };
        let p = payloads(Codec::H264, &au);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0][0] & 0x1f, 28);
        assert_eq!(p[0][1], 0x80 | 5);
        assert_eq!(p[2][1], 0x40 | 5);
        let total: usize = p.iter().map(|x| x.len() - 2).sum();
        assert_eq!(total, 3000);
    }

    #[test]
    fn fragments_large_h265_nal() {
        let mut nal = vec![19 << 1, 1];
        nal.extend(std::iter::repeat_n(0xcd, 2500));
        let au = AccessUnit {
            nals: vec![Bytes::from(nal)],
            is_key: true,
        };
        let p = payloads(Codec::H265, &au);
        assert_eq!(p.len(), 3);
        assert_eq!((p[0][0] >> 1) & 0x3f, 49);
        assert_eq!(p[0][2], 0x80 | 19);
        assert_eq!(p[2][2], 0x40 | 19);
    }
}
