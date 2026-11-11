//! What a republished stream carries, read from the ISO BMFF sample entries
//! of the frames, and its SDP description.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rustnvr_core::{AudioParams, VideoCodec, VideoFrame, VideoParams};
use std::sync::Arc;

/// RTP payload type of the video track.
pub const VIDEO_PAYLOAD_TYPE: u8 = 96;
/// RTP payload type of the audio track.
pub const AUDIO_PAYLOAD_TYPE: u8 = 97;
/// Clock of the video RTP timestamps.
pub const VIDEO_CLOCK: u32 = 90_000;

/// Sound formats republished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioKind {
    /// AAC (`mpeg4-generic`, RFC 3640).
    Aac,
    /// Opus (RFC 7587): the G.711 of cameras, already converted.
    Opus,
}

/// The audio track of a stream.
#[derive(Clone, Debug)]
pub struct AudioTrack {
    /// Format.
    pub kind: AudioKind,
    /// Parameters of the frames.
    pub params: Arc<AudioParams>,
    /// AAC `AudioSpecificConfig` (empty for Opus).
    pub config: Vec<u8>,
}

/// The tracks of a stream, as announced to a client.
#[derive(Clone, Debug)]
pub struct Description {
    /// Video parameters.
    pub video: Arc<VideoParams>,
    /// Parameter sets (H.264: SPS, PPS; H.265: VPS, SPS, PPS), without start
    /// codes or length prefixes.
    pub parameter_sets: Vec<Vec<u8>>,
    /// Audio track, when the camera's sound is received in a format clients
    /// take over RTSP.
    pub audio: Option<AudioTrack>,
}

impl Description {
    /// The description of the stream `frame` belongs to; `None` when its
    /// parameter sets cannot be read.
    pub fn of(frame: &VideoFrame) -> Option<Self> {
        let parameter_sets = parameter_sets(&frame.params)?;
        let audio = frame.audio.as_ref().and_then(|a| {
            let params = a.params.clone();
            if params.rfc6381_codec == "opus" {
                Some(AudioTrack {
                    kind: AudioKind::Opus,
                    params,
                    config: Vec::new(),
                })
            } else if params.rfc6381_codec.starts_with("mp4a") {
                let config = aac_config(&params.sample_entry)?;
                Some(AudioTrack {
                    kind: AudioKind::Aac,
                    params,
                    config,
                })
            } else {
                None
            }
        });
        Some(Self {
            video: frame.params.clone(),
            parameter_sets,
            audio,
        })
    }

    /// SDP of the stream. Tracks are `trackID=0` (video) and `trackID=1`
    /// (audio), relative to the address of the stream.
    pub fn sdp(&self) -> String {
        let pt = VIDEO_PAYLOAD_TYPE;
        let sets: Vec<String> = self.parameter_sets.iter().map(|s| B64.encode(s)).collect();
        let (rtpmap, fmtp) = match self.video.codec {
            VideoCodec::H264 => {
                let profile = self
                    .parameter_sets
                    .first()
                    .and_then(|sps| sps.get(1..4))
                    .map(|p| format!("profile-level-id={:02X}{:02X}{:02X};", p[0], p[1], p[2]))
                    .unwrap_or_default();
                (
                    "H264",
                    format!(
                        "packetization-mode=1;{profile}sprop-parameter-sets={}",
                        sets.join(",")
                    ),
                )
            }
            VideoCodec::H265 => {
                let mut parts = Vec::new();
                for (set, value) in self.parameter_sets.iter().zip(&sets) {
                    let name = match set.first().map(|b| (b >> 1) & 0x3f) {
                        Some(32) => "sprop-vps",
                        Some(33) => "sprop-sps",
                        Some(34) => "sprop-pps",
                        _ => continue,
                    };
                    parts.push(format!("{name}={value}"));
                }
                ("H265", parts.join(";"))
            }
        };
        let mut sdp = format!(
            "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=RustNVR\r\nc=IN IP4 0.0.0.0\r\nt=0 0\r\n\
             a=control:*\r\n\
             m=video 0 RTP/AVP {pt}\r\na=rtpmap:{pt} {rtpmap}/{VIDEO_CLOCK}\r\n\
             a=fmtp:{pt} {fmtp}\r\na=control:trackID=0\r\n"
        );
        if let Some(audio) = &self.audio {
            let pt = AUDIO_PAYLOAD_TYPE;
            let rate = audio.params.clock_rate;
            let channels = audio.params.channels;
            sdp.push_str(&format!("m=audio 0 RTP/AVP {pt}\r\n"));
            match audio.kind {
                // RFC 7587: always announced as 48 kHz stereo.
                AudioKind::Opus => sdp.push_str(&format!(
                    "a=rtpmap:{pt} opus/48000/2\r\na=fmtp:{pt} sprop-stereo={}\r\n",
                    u8::from(channels > 1)
                )),
                AudioKind::Aac => {
                    let config: String = audio.config.iter().map(|b| format!("{b:02X}")).collect();
                    sdp.push_str(&format!(
                        "a=rtpmap:{pt} mpeg4-generic/{rate}/{channels}\r\n\
                         a=fmtp:{pt} streamtype=5;profile-level-id=1;mode=AAC-hbr;\
                         sizelength=13;indexlength=3;indexdeltalength=3;config={config}\r\n"
                    ));
                }
            }
            sdp.push_str("a=control:trackID=1\r\n");
        }
        sdp
    }
}

/// Child boxes of a sample entry whose own fields take `skip` bytes after
/// its 8-byte header: the contents of the first box named `name`.
fn child_box<'a>(entry: &'a [u8], skip: usize, name: &[u8; 4]) -> Option<&'a [u8]> {
    let mut rest = entry.get(8 + skip..)?;
    loop {
        let size = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        if size < 8 {
            return None;
        }
        let body = rest.get(8..size)?;
        if rest.get(4..8)? == name {
            return Some(body);
        }
        rest = &rest[size..];
    }
}

/// `count` parameter sets, each preceded by its 16-bit length, at `*pos`.
fn read_sets(data: &[u8], pos: &mut usize, count: usize, out: &mut Vec<Vec<u8>>) -> Option<()> {
    for _ in 0..count {
        let len = usize::from(u16::from_be_bytes(
            data.get(*pos..*pos + 2)?.try_into().ok()?,
        ));
        out.push(data.get(*pos + 2..*pos + 2 + len)?.to_vec());
        *pos += 2 + len;
    }
    Some(())
}

/// Parameter sets of a video sample entry (`avc1` with `avcC`, or
/// `hvc1`/`hev1` with `hvcC`): SPS then PPS, or VPS, SPS then PPS.
pub fn parameter_sets(params: &VideoParams) -> Option<Vec<Vec<u8>>> {
    // A visual sample entry has 78 bytes of fields before its child boxes.
    const VISUAL_FIELDS: usize = 78;
    let mut out = Vec::new();
    match params.codec {
        VideoCodec::H264 => {
            let avcc = child_box(&params.sample_entry, VISUAL_FIELDS, b"avcC")?;
            let mut pos = 6;
            read_sets(avcc, &mut pos, usize::from(avcc.get(5)? & 0x1f), &mut out)?;
            let pps = usize::from(*avcc.get(pos)?);
            pos += 1;
            read_sets(avcc, &mut pos, pps, &mut out)?;
        }
        VideoCodec::H265 => {
            let hvcc = child_box(&params.sample_entry, VISUAL_FIELDS, b"hvcC")?;
            let arrays = *hvcc.get(22)?;
            let mut pos = 23;
            for _ in 0..arrays {
                let count = usize::from(u16::from_be_bytes(
                    hvcc.get(pos + 1..pos + 3)?.try_into().ok()?,
                ));
                pos += 3;
                read_sets(hvcc, &mut pos, count, &mut out)?;
            }
            // VPS (32), SPS (33), PPS (34) in that order; nothing else (SEI).
            out.retain(|s| {
                s.first()
                    .is_some_and(|b| (32..=34).contains(&((b >> 1) & 0x3f)))
            });
            out.sort_by_key(|s| (s[0] >> 1) & 0x3f);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Reads a descriptor header (tag, then a length of 1 to 4 bytes): the tag
/// and the descriptor's contents.
fn descriptor(data: &[u8]) -> Option<(u8, &[u8])> {
    let tag = *data.first()?;
    let mut len = 0usize;
    let mut pos = 1;
    loop {
        let b = *data.get(pos)?;
        pos += 1;
        len = (len << 7) | usize::from(b & 0x7f);
        if b & 0x80 == 0 || pos > 4 {
            break;
        }
    }
    Some((tag, data.get(pos..pos + len)?))
}

/// The `AudioSpecificConfig` of an `mp4a` sample entry (in its `esds` box).
pub fn aac_config(sample_entry: &[u8]) -> Option<Vec<u8>> {
    // An audio sample entry has 28 bytes of fields before its child boxes.
    let esds = child_box(sample_entry, 28, b"esds")?;
    // Version and flags, then the ES descriptor.
    let (tag, es) = descriptor(esds.get(4..)?)?;
    if tag != 0x03 {
        return None;
    }
    let flags = *es.get(2)?;
    let mut pos = 3;
    if flags & 0x80 != 0 {
        pos += 2;
    }
    if flags & 0x40 != 0 {
        pos += 1 + usize::from(*es.get(pos)?);
    }
    if flags & 0x20 != 0 {
        pos += 2;
    }
    let (tag, decoder) = descriptor(es.get(pos..)?)?;
    if tag != 0x04 {
        return None;
    }
    // Object type, stream type, buffer size, maximum and average bit rates.
    let (tag, specific) = descriptor(decoder.get(13..)?)?;
    (tag == 0x05 && !specific.is_empty()).then(|| specific.to_vec())
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use bytes::Bytes;
    use rustnvr_core::FrameAudio;
    use std::time::SystemTime;

    fn boxed(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(name);
        b.extend_from_slice(body);
        b
    }

    fn entry(name: &[u8; 4], fields: usize, child: &[u8]) -> Bytes {
        let mut body = vec![0u8; fields];
        body.extend_from_slice(child);
        Bytes::from(boxed(name, &body))
    }

    pub(crate) fn h264_params() -> VideoParams {
        let avcc = [
            1, 0x42, 0xc0, 0x1e, 0xff, 0xe1, 0, 4, 0x67, 0x42, 0xc0, 0x1e, 1, 0, 1, 0x68,
        ];
        VideoParams {
            codec: VideoCodec::H264,
            rfc6381_codec: "avc1.42C01E".into(),
            width: 320,
            height: 240,
            clock_rate: 90_000,
            sample_entry: entry(b"avc1", 78, &boxed(b"avcC", &avcc)),
        }
    }

    fn h265_params() -> VideoParams {
        let mut hvcc = vec![0u8; 22];
        // PPS, VPS and SPS arrays, out of order, plus an SEI array.
        hvcc.push(4);
        for (ty, nal) in [
            (34u8, vec![34 << 1, 1, 0xc0]),
            (32, vec![32 << 1, 1, 0x0c]),
            (39, vec![39 << 1, 1, 0xaa]),
            (33, vec![33 << 1, 1, 0x01]),
        ] {
            hvcc.push(0x80 | ty);
            hvcc.extend_from_slice(&1u16.to_be_bytes());
            hvcc.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            hvcc.extend_from_slice(&nal);
        }
        VideoParams {
            codec: VideoCodec::H265,
            rfc6381_codec: "hvc1.1.6.L60.B0".into(),
            width: 320,
            height: 240,
            clock_rate: 90_000,
            sample_entry: entry(b"hvc1", 78, &boxed(b"hvcC", &hvcc)),
        }
    }

    pub(crate) fn aac_params() -> AudioParams {
        // ES descriptor (ES_ID 0, no flags) > decoder configuration >
        // AudioSpecificConfig 0x14 0x08 (AAC-LC, 16 kHz, mono), with long
        // length forms as FFmpeg writes them.
        let specific = [0x05, 0x80, 0x80, 0x80, 2, 0x14, 0x08];
        let mut decoder = vec![0x04, 13 + specific.len() as u8, 0x40, 0x15];
        decoder.extend_from_slice(&[0; 11]);
        decoder.extend_from_slice(&specific);
        let mut es = vec![0x03, 3 + decoder.len() as u8 + 3, 0, 0, 0];
        es.extend_from_slice(&decoder);
        es.extend_from_slice(&[0x06, 1, 2]);
        let mut esds = vec![0u8; 4];
        esds.extend_from_slice(&es);
        AudioParams {
            rfc6381_codec: "mp4a.40.2".into(),
            clock_rate: 16_000,
            channels: 1,
            sample_entry: entry(b"mp4a", 28, &boxed(b"esds", &esds)),
        }
    }

    pub(crate) fn frame(params: VideoParams, audio: Option<AudioParams>) -> VideoFrame {
        VideoFrame {
            session: 1,
            seq: 0,
            params: Arc::new(params),
            pts: 0,
            is_key: true,
            wall_time: SystemTime::now(),
            loss: 0,
            data: Bytes::new(),
            audio: audio.map(|p| {
                Arc::new(FrameAudio {
                    params: Arc::new(p),
                    samples: Vec::new(),
                })
            }),
        }
    }

    #[test]
    fn reads_h264_parameter_sets_and_describes_them() {
        let d = Description::of(&frame(h264_params(), None)).unwrap();
        assert_eq!(
            d.parameter_sets,
            vec![vec![0x67, 0x42, 0xc0, 0x1e], vec![0x68]]
        );
        let sdp = d.sdp();
        assert!(sdp.contains("a=rtpmap:96 H264/90000\r\n"), "{sdp}");
        assert!(
            sdp.contains(
                "packetization-mode=1;profile-level-id=42C01E;sprop-parameter-sets=Z0LAHg==,aA=="
            ),
            "{sdp}"
        );
        assert!(!sdp.contains("m=audio"));
    }

    #[test]
    fn reads_h265_parameter_sets_in_order() {
        let d = Description::of(&frame(h265_params(), None)).unwrap();
        let types: Vec<u8> = d
            .parameter_sets
            .iter()
            .map(|s| (s[0] >> 1) & 0x3f)
            .collect();
        assert_eq!(types, vec![32, 33, 34]);
        let sdp = d.sdp();
        assert!(sdp.contains("a=rtpmap:96 H265/90000\r\n"), "{sdp}");
        let (vps, sps, pps) = (
            sdp.find("sprop-vps=").unwrap(),
            sdp.find("sprop-sps=").unwrap(),
            sdp.find("sprop-pps=").unwrap(),
        );
        assert!(vps < sps && sps < pps);
    }

    #[test]
    fn describes_aac_and_opus_sound() {
        let d = Description::of(&frame(h264_params(), Some(aac_params()))).unwrap();
        let audio = d.audio.as_ref().unwrap();
        assert_eq!(
            (audio.kind, audio.config.as_slice()),
            (AudioKind::Aac, &[0x14, 0x08][..])
        );
        let sdp = d.sdp();
        assert!(
            sdp.contains("a=rtpmap:97 mpeg4-generic/16000/1\r\n"),
            "{sdp}"
        );
        assert!(
            sdp.contains(
                "mode=AAC-hbr;sizelength=13;indexlength=3;indexdeltalength=3;config=1408\r\n"
            ),
            "{sdp}"
        );
        assert!(sdp.ends_with("a=control:trackID=1\r\n"));

        let opus = AudioParams {
            rfc6381_codec: "opus".into(),
            clock_rate: 48_000,
            channels: 1,
            sample_entry: Bytes::new(),
        };
        let sdp = Description::of(&frame(h264_params(), Some(opus)))
            .unwrap()
            .sdp();
        assert!(
            sdp.contains("a=rtpmap:97 opus/48000/2\r\na=fmtp:97 sprop-stereo=0\r\n"),
            "{sdp}"
        );
    }

    #[test]
    fn unreadable_entries_give_nothing() {
        let mut p = h264_params();
        p.sample_entry = Bytes::from_static(&[0; 20]);
        assert!(Description::of(&frame(p, None)).is_none());
        // Unknown sound: the video alone.
        let mut a = aac_params();
        a.sample_entry = Bytes::new();
        assert!(
            Description::of(&frame(h264_params(), Some(a)))
                .unwrap()
                .audio
                .is_none()
        );
    }
}
