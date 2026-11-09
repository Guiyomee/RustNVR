//! G.711 sound converted to Opus.
//!
//! Many cameras send G.711 (PCMU / PCMA, RFC 3551), which neither MP4 files
//! nor browsers play. It is cheap to decode (one table lookup per sample) and
//! to encode again as Opus, which MP4 files carry (`Opus` sample entry, see
//! "Encapsulation of Opus in ISO Base Media File Format") and every current
//! browser plays. Only the sound is converted; the video is never decoded.

use bytes::Bytes;
use rustnvr_core::{AudioParams, AudioSample};
use std::sync::Arc;

/// Clock of Opus timestamps in MP4 files (whatever the input rate).
pub const OPUS_RATE: u32 = 48_000;

/// Opus frame length: 20 ms.
const FRAMES_PER_SECOND: u32 = 50;

/// Opus packets are much smaller (about 50 bytes at 16 kbit/s).
const MAX_PACKET: usize = 1500;

/// The two G.711 companding laws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Law {
    /// µ-law (PCMU, North America, Japan, most cameras).
    Mu,
    /// A-law (PCMA, Europe).
    A,
}

impl Law {
    /// The law of an RTP encoding name (`pcmu`, `pcma`), if it is G.711.
    pub fn from_encoding(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "pcmu" => Some(Self::Mu),
            "pcma" => Some(Self::A),
            _ => None,
        }
    }

    /// Name for the logs and the interface.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mu => "G.711 µ-law",
            Self::A => "G.711 A-law",
        }
    }

    /// One 16-bit linear sample from one G.711 byte (ITU-T G.711).
    pub fn decode(self, byte: u8) -> i16 {
        match self {
            Self::Mu => {
                let u = !byte;
                let exponent = (u >> 4) & 0x07;
                let mantissa = i32::from(u & 0x0f);
                let magnitude = (((mantissa << 3) + 0x84) << exponent) - 0x84;
                // At most 32124: always fits.
                (if u & 0x80 != 0 { -magnitude } else { magnitude }) as i16
            }
            Self::A => {
                let a = byte ^ 0x55;
                let exponent = (a >> 4) & 0x07;
                let mantissa = i32::from(a & 0x0f);
                let magnitude = if exponent == 0 {
                    (mantissa << 4) + 8
                } else {
                    ((mantissa << 4) + 0x108) << (exponent - 1)
                };
                // At most 32256: always fits.
                (if a & 0x80 != 0 { magnitude } else { -magnitude }) as i16
            }
        }
    }
}

/// Converts the G.711 sound of one RTSP session to Opus frames of 20 ms.
pub struct G711ToOpus {
    law: Law,
    rate: u32,
    /// Samples per Opus frame at the input rate.
    frame: usize,
    encoder: opus::Encoder,
    /// Decoded samples not encoded yet (less than one frame).
    pcm: Vec<i16>,
    /// Input timestamp expected for the next packet.
    next_in: Option<i64>,
    /// Timestamp of the next Opus frame, in [`OPUS_RATE`] units.
    out_pts: i64,
    packet: Vec<u8>,
    failures: u64,
    params: Arc<AudioParams>,
}

impl G711ToOpus {
    /// A converter for G.711 at `rate` Hz, `channels` channels. Opus takes
    /// 8, 12, 16, 24 and 48 kHz; cameras send mono G.711 at 8 kHz.
    pub fn new(law: Law, rate: u32, channels: u16) -> Result<Self, String> {
        if !matches!(rate, 8_000 | 12_000 | 16_000 | 24_000 | 48_000) {
            return Err(format!(
                "{} at {rate} Hz: not a rate Opus takes",
                law.name()
            ));
        }
        if channels != 1 {
            return Err(format!(
                "{} with {channels} channels: only mono is converted",
                law.name()
            ));
        }
        let opus_err = |e: opus::Error| format!("Opus: {e}");
        let mut encoder = opus::Encoder::new(rate, opus::Channels::Mono, opus::Application::Voip)
            .map_err(opus_err)?;
        // Transparent for 8 kHz telephone-band sound, a quarter of G.711's
        // 64 kbit/s; complexity 5 keeps the processor load negligible.
        let bitrate = if rate <= 8_000 { 16_000 } else { 24_000 };
        encoder
            .set_bitrate(opus::Bitrate::Bits(bitrate))
            .map_err(opus_err)?;
        encoder.set_complexity(5).map_err(opus_err)?;
        // Samples the decoder drops at the start (RFC 7845), at 48 kHz.
        let lookahead = encoder.get_lookahead().map_err(opus_err)?;
        let pre_skip =
            u16::try_from(i64::from(lookahead) * i64::from(OPUS_RATE / rate)).unwrap_or(312);
        let params = AudioParams {
            rfc6381_codec: "opus".to_owned(),
            clock_rate: OPUS_RATE,
            channels: 1,
            sample_entry: opus_sample_entry(1, pre_skip),
        };
        Ok(Self {
            law,
            rate,
            frame: (rate / FRAMES_PER_SECOND) as usize,
            encoder,
            pcm: Vec::new(),
            next_in: None,
            out_pts: 0,
            packet: vec![0; MAX_PACKET],
            failures: 0,
            params: Arc::new(params),
        })
    }

    /// Parameters of the Opus sound produced.
    pub fn params(&self) -> &Arc<AudioParams> {
        &self.params
    }

    /// The G.711 law converted.
    pub fn law(&self) -> Law {
        self.law
    }

    /// Converts the G.711 bytes of one packet, whose first sample is at `pts`
    /// (input clock, on the video timeline); complete Opus frames are
    /// appended to `out`, timed in [`OPUS_RATE`] units.
    pub fn push(&mut self, pts: i64, data: &[u8], out: &mut Vec<AudioSample>) {
        let scale = i64::from(OPUS_RATE / self.rate);
        // First packet, or a jump of more than 100 ms (lost packets, camera
        // restarting its sound): what is pending is dropped and the Opus
        // timeline starts again from this packet.
        let continuous = self
            .next_in
            .is_some_and(|n| (pts - n).abs() <= i64::from(self.rate / 10));
        if !continuous {
            self.pcm.clear();
            self.out_pts = pts * scale;
        }
        self.next_in = Some(pts + data.len() as i64);
        let law = self.law;
        self.pcm.extend(data.iter().map(|&b| law.decode(b)));
        let duration = (self.frame as i64 * scale) as u32;
        let mut start = 0;
        while self.pcm.len() - start >= self.frame {
            match self
                .encoder
                .encode(&self.pcm[start..start + self.frame], &mut self.packet)
            {
                Ok(len) => out.push(AudioSample {
                    pts: self.out_pts,
                    duration,
                    data: Bytes::copy_from_slice(&self.packet[..len]),
                }),
                Err(e) => {
                    self.failures += 1;
                    if self.failures.is_power_of_two() {
                        tracing::warn!(error = %e, count = self.failures, "Opus encoding failed: 20 ms of sound skipped");
                    }
                }
            }
            self.out_pts += i64::from(duration);
            start += self.frame;
        }
        self.pcm.drain(..start);
    }
}

/// ISO BMFF `Opus` audio sample entry with its `dOps` box.
///
/// Its input sample rate is 48 kHz, not the G.711 rate: the field is only
/// informative (decoders always give 48 kHz), and Chromium's media source
/// refuses a stream whose `dOps` rate differs from the sample entry's
/// ("Opus AudioSampleEntry sample rate mismatches OpusSpecificBox"), which
/// broke live view and playback with sound. FFmpeg writes 48 kHz too.
fn opus_sample_entry(channels: u8, pre_skip: u16) -> Bytes {
    let mut dops = Vec::with_capacity(19);
    dops.extend_from_slice(&19u32.to_be_bytes());
    dops.extend_from_slice(b"dOps");
    dops.push(0); // version
    dops.push(channels);
    dops.extend_from_slice(&pre_skip.to_be_bytes());
    dops.extend_from_slice(&OPUS_RATE.to_be_bytes());
    dops.extend_from_slice(&0i16.to_be_bytes()); // output gain
    dops.push(0); // channel mapping family: mono / stereo
    let mut entry = Vec::with_capacity(8 + 28 + dops.len());
    entry.extend_from_slice(&((8 + 28 + dops.len()) as u32).to_be_bytes());
    entry.extend_from_slice(b"Opus");
    entry.extend_from_slice(&[0; 6]); // reserved
    entry.extend_from_slice(&1u16.to_be_bytes()); // data reference index
    entry.extend_from_slice(&[0; 8]); // reserved
    entry.extend_from_slice(&u16::from(channels).to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes()); // sample size
    entry.extend_from_slice(&[0; 4]); // pre-defined, reserved
    entry.extend_from_slice(&(OPUS_RATE << 16).to_be_bytes());
    entry.extend_from_slice(&dops);
    Bytes::from(entry)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use rustnvr_testkit::audio::{mulaw, tone};

    #[test]
    fn g711_bytes_decode_like_the_standard() {
        assert_eq!(Law::Mu.decode(0xff), 0);
        assert_eq!(Law::Mu.decode(0x7f), 0);
        assert_eq!(Law::Mu.decode(0x00), -32_124);
        assert_eq!(Law::Mu.decode(0x80), 32_124);
        assert_eq!(Law::A.decode(0xd5), 8);
        assert_eq!(Law::A.decode(0x55), -8);
        assert_eq!(Law::A.decode(0xaa), 32_256);
        assert_eq!(Law::A.decode(0x2a), -32_256);
        for s in [-30_000i16, -1000, -10, 0, 10, 1000, 30_000] {
            let back = i32::from(Law::Mu.decode(mulaw(s)));
            assert!(
                (back - i32::from(s)).abs() <= i32::from(s).abs() / 16 + 8,
                "{s} -> {back}"
            );
        }
        assert_eq!(Law::from_encoding("PCMU"), Some(Law::Mu));
        assert_eq!(Law::from_encoding("pcma"), Some(Law::A));
        assert_eq!(Law::from_encoding("mpeg4-generic"), None);
    }

    #[test]
    fn only_rates_and_channels_opus_takes() {
        assert!(G711ToOpus::new(Law::Mu, 8_000, 1).is_ok());
        assert!(G711ToOpus::new(Law::A, 16_000, 1).is_ok());
        assert!(G711ToOpus::new(Law::Mu, 11_025, 1).is_err());
        assert!(G711ToOpus::new(Law::Mu, 8_000, 2).is_err());
    }

    #[test]
    fn the_sample_entry_is_an_opus_box() {
        let c = G711ToOpus::new(Law::Mu, 8_000, 1).unwrap();
        let e = &c.params().sample_entry;
        assert_eq!(
            u32::from_be_bytes(e[0..4].try_into().unwrap()) as usize,
            e.len()
        );
        assert_eq!(&e[4..8], b"Opus");
        assert_eq!(&e[36 + 4..36 + 8], b"dOps");
        // The same rate in the sample entry and in dOps (Chromium checks it).
        assert_eq!(
            u32::from_be_bytes(e[32..36].try_into().unwrap()),
            48_000 << 16
        );
        assert_eq!(
            u32::from_be_bytes(e[36 + 12..36 + 16].try_into().unwrap()),
            48_000
        );
        assert_eq!(c.params().clock_rate, OPUS_RATE);
        assert_eq!(c.params().rfc6381_codec, "opus");
    }

    /// One second of a 440 Hz tone in G.711 packets of 20 ms, then the same
    /// after a gap: Opus frames every 20 ms (960 at 48 kHz), the timeline
    /// starting again after the gap, and the tone back after decoding.
    #[test]
    fn a_tone_comes_back_through_opus() {
        let mut c = G711ToOpus::new(Law::Mu, 8_000, 1).unwrap();
        let mut out = Vec::new();
        for packet in 0..50i64 {
            let bytes: Vec<u8> = (0..160)
                .map(|i| mulaw(tone((packet * 160 + i) as u64)))
                .collect();
            c.push(1_000 + packet * 160, &bytes, &mut out);
        }
        assert_eq!(out.len(), 50);
        for (i, s) in out.iter().enumerate() {
            assert_eq!(s.pts, 6_000 + i as i64 * 960);
            assert_eq!(s.duration, 960);
            assert!(s.data.len() < 120, "{} bytes", s.data.len());
        }
        // 5 s later: a new timeline.
        let bytes: Vec<u8> = (0..160u64).map(|i| mulaw(tone(i))).collect();
        c.push(1_000 + 45_000, &bytes, &mut out);
        assert_eq!(out.last().unwrap().pts, (1_000 + 45_000) * 6);

        let mut decoder = opus::Decoder::new(48_000, opus::Channels::Mono).unwrap();
        let mut pcm = Vec::new();
        let mut buf = vec![0i16; 5760];
        for s in &out[..50] {
            let n = decoder.decode(&s.data, &mut buf, false).unwrap();
            pcm.extend_from_slice(&buf[..n]);
        }
        assert_eq!(pcm.len(), 48_000);
        // After the start: as loud as the tone, and 440 periods per second
        // (zero crossings: 880 per second).
        let steady = &pcm[9_600..];
        let rms = (steady.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / steady.len() as f64)
            .sqrt();
        assert!((4_000.0..7_500.0).contains(&rms), "rms {rms}");
        let crossings = steady
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count();
        let per_second = crossings as f64 * 48_000.0 / steady.len() as f64;
        assert!(
            (860.0..900.0).contains(&per_second),
            "{per_second} crossings per second"
        );
    }

    /// Processor time of the conversion, measured with
    /// `cargo test --release -p rustnvr-stream conversion_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement"]
    fn conversion_cost() {
        let mut c = G711ToOpus::new(Law::Mu, 8_000, 1).unwrap();
        let packets: Vec<Vec<u8>> = (0..50i64)
            .map(|p| {
                (0..160)
                    .map(|i| mulaw(tone((p * 160 + i) as u64)))
                    .collect()
            })
            .collect();
        let mut out = Vec::new();
        let seconds = 600i64;
        let start = std::time::Instant::now();
        for s in 0..seconds {
            for (p, bytes) in packets.iter().enumerate() {
                c.push((s * 50 + p as i64) * 160, bytes, &mut out);
            }
            out.clear();
        }
        let per_second = start.elapsed() / seconds as u32;
        println!(
            "{per_second:?} of processor time per second of sound ({:.3} % of one core)",
            per_second.as_secs_f64() * 100.0
        );
    }
}
