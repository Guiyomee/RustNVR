//! Annex B elementary stream parsing for fixtures.

use bytes::Bytes;

/// Codec of a fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// H.264.
    H264,
    /// H.265.
    H265,
}

impl Codec {
    /// NAL unit type of `nal`.
    pub fn nal_type(self, nal: &[u8]) -> u8 {
        match self {
            Codec::H264 => nal[0] & 0x1f,
            Codec::H265 => (nal[0] >> 1) & 0x3f,
        }
    }

    fn is_aud(self, t: u8) -> bool {
        match self {
            Codec::H264 => t == 9,
            Codec::H265 => t == 35,
        }
    }

    fn is_key_slice(self, t: u8) -> bool {
        match self {
            Codec::H264 => t == 5,
            Codec::H265 => (16..=21).contains(&t),
        }
    }
}

/// One access unit (picture) as a list of NAL units without start codes.
#[derive(Clone, Debug)]
pub struct AccessUnit {
    /// NAL units, in decoding order.
    pub nals: Vec<Bytes>,
    /// Whether the access unit is a random access point.
    pub is_key: bool,
}

/// An elementary stream split into access units.
#[derive(Clone, Debug)]
pub struct Fixture {
    /// Codec.
    pub codec: Codec,
    /// Frame rate of the stream.
    pub fps: u32,
    /// Access units, starting with a key frame.
    pub access_units: Vec<AccessUnit>,
    /// Parameter sets (VPS for H.265, SPS, PPS) found first in the stream.
    pub vps: Option<Bytes>,
    /// Sequence parameter set.
    pub sps: Bytes,
    /// Picture parameter set.
    pub pps: Bytes,
}

const H264_FIXTURE: &[u8] = include_bytes!("../fixtures/h264_320x240_10fps.h264");
const H265_FIXTURE: &[u8] = include_bytes!("../fixtures/h265_320x240_10fps.h265");

impl Fixture {
    /// 320x240, 10 fps, 4 s, key frame every second, H.264 Constrained Baseline.
    pub fn h264() -> Self {
        Self::parse(Codec::H264, 10, H264_FIXTURE)
    }

    /// 320x240, 10 fps, 4 s, key frame every second, H.265 Main.
    pub fn h265() -> Self {
        Self::parse(Codec::H265, 10, H265_FIXTURE)
    }

    /// Splits an Annex B stream that contains access unit delimiters.
    ///
    /// # Panics
    ///
    /// Panics if the stream has no SPS/PPS (or VPS for H.265); fixtures are
    /// trusted test inputs.
    pub fn parse(codec: Codec, fps: u32, data: &[u8]) -> Self {
        let mut access_units = Vec::new();
        let mut current: Vec<Bytes> = Vec::new();
        let (mut vps, mut sps, mut pps) = (None, None, None);
        for nal in split_nals(data) {
            let t = codec.nal_type(nal);
            if codec.is_aud(t) {
                if !current.is_empty() {
                    access_units.push(Self::make_au(codec, std::mem::take(&mut current)));
                }
                continue;
            }
            let nal = Bytes::copy_from_slice(nal);
            match (codec, t) {
                (Codec::H264, 7) | (Codec::H265, 33) => {
                    sps.get_or_insert_with(|| nal.clone());
                }
                (Codec::H264, 8) | (Codec::H265, 34) => {
                    pps.get_or_insert_with(|| nal.clone());
                }
                (Codec::H265, 32) => {
                    vps.get_or_insert_with(|| nal.clone());
                }
                _ => {}
            }
            current.push(nal);
        }
        if !current.is_empty() {
            access_units.push(Self::make_au(codec, current));
        }
        assert!(
            access_units.first().is_some_and(|au| au.is_key),
            "fixture must start with a key frame"
        );
        assert!(
            codec == Codec::H264 || vps.is_some(),
            "H.265 fixture without VPS"
        );
        Self {
            codec,
            fps,
            access_units,
            vps,
            sps: sps.expect("fixture without SPS"),
            pps: pps.expect("fixture without PPS"),
        }
    }

    fn make_au(codec: Codec, nals: Vec<Bytes>) -> AccessUnit {
        let is_key = nals.iter().any(|n| codec.is_key_slice(codec.nal_type(n)));
        AccessUnit { nals, is_key }
    }

    /// RTP timestamp increment between access units at 90 kHz.
    pub fn ticks_per_frame(&self) -> u32 {
        90_000 / self.fps
    }
}

/// Splits an Annex B byte stream on `00 00 01` / `00 00 00 01` start codes.
pub fn split_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(data.len(), |&next| next - 3);
        let mut nal = &data[start..end];
        // Trailing zeros belong to the next (4-byte) start code or are
        // trailing_zero_8bits; neither is part of the NAL unit.
        while let [rest @ .., 0] = nal {
            nal = rest;
        }
        if !nal.is_empty() {
            nals.push(nal);
        }
    }
    nals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_handles_both_start_code_lengths() {
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4,
        ];
        let nals = split_nals(&data);
        assert_eq!(
            nals,
            vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4][..]]
        );
    }

    #[test]
    fn fixtures_parse() {
        for f in [Fixture::h264(), Fixture::h265()] {
            assert_eq!(f.access_units.len(), 40, "{:?}", f.codec);
            let keys = f.access_units.iter().filter(|a| a.is_key).count();
            assert_eq!(keys, 4, "{:?}", f.codec);
        }
    }
}
