//! Minimal fragmented MP4 (ISO/IEC 14496-12) writer: one video track and an
//! optional audio track (AAC), both in each fragment.
//!
//! Layout of a segment file:
//!
//! ```text
//! ftyp
//! moov (mvhd, trak(tkhd, mdia(mdhd, hdlr, minf(vmhd, dinf, stbl(stsd, empty tables)))), mvex(trex))
//! moof (mfhd, traf(tfhd, tfdt, trun))  mdat   ← one pair per fragment
//! moof ... mdat ...
//! ```
//!
//! The track timescale is the RTP clock rate, so sample durations are exact
//! timestamp differences. The `stsd` entry (`avc1`/`hvc1`) comes from the RTSP
//! layer ([`VideoParams::sample_entry`]).

use rustnvr_core::{AudioParams, VideoParams};

/// Track identifier of the video track.
pub const TRACK_ID: u32 = 1;
/// Track identifier of the audio track, when present.
pub const AUDIO_TRACK_ID: u32 = 2;

/// `sample_flags` of a sync sample (depends on no other sample).
pub const SYNC_SAMPLE_FLAGS: u32 = 0x0200_0000;
/// `sample_flags` of a non-sync sample (depends on others, `sample_is_non_sync_sample`).
pub const NON_SYNC_SAMPLE_FLAGS: u32 = 0x0101_0000;

/// `trun` flags: data-offset, sample-duration, sample-size and sample-flags present.
const TRUN_FLAGS: u32 = 0x0000_0701;
/// `tfhd` flag: default-base-is-moof.
const TFHD_DEFAULT_BASE_IS_MOOF: u32 = 0x0002_0000;

const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

/// Description of one sample of a fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleMeta {
    /// Duration in timescale units.
    pub duration: u32,
    /// Size in bytes.
    pub size: u32,
    /// Sync sample.
    pub is_key: bool,
}

fn write_box(out: &mut Vec<u8>, fourcc: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(fourcc);
    body(out);
    let size = u32::try_from(out.len() - start).unwrap_or(u32::MAX);
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn write_full_box(
    out: &mut Vec<u8>,
    fourcc: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    write_box(out, fourcc, |out| {
        out.extend_from_slice(&((u32::from(version) << 24) | (flags & 0x00ff_ffff)).to_be_bytes());
        body(out);
    });
}

fn u16be(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn u32be(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn u64be(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Builds the initialization segment (`ftyp` + `moov`) of a video-only file.
pub fn init_segment(params: &VideoParams) -> Vec<u8> {
    init_segment_av(params, None)
}

/// Media of a `trak`.
enum Track<'a> {
    Video(&'a VideoParams),
    Audio(&'a AudioParams),
}

fn write_trak(o: &mut Vec<u8>, track_id: u32, track: &Track<'_>) {
    let (width, height, volume, timescale, entry, handler, name): (
        u32,
        u32,
        u16,
        u32,
        &[u8],
        &[u8; 4],
        &[u8],
    ) = match track {
        Track::Video(p) => (
            p.width,
            p.height,
            0,
            p.clock_rate,
            &p.sample_entry,
            b"vide",
            b"RustNVR video\0",
        ),
        Track::Audio(p) => (
            0,
            0,
            0x0100,
            p.clock_rate,
            &p.sample_entry,
            b"soun",
            b"RustNVR audio\0",
        ),
    };
    write_box(o, b"trak", |o| {
        write_full_box(o, b"tkhd", 0, 0x3, |o| {
            u32be(o, 0); // creation_time
            u32be(o, 0); // modification_time
            u32be(o, track_id);
            u32be(o, 0); // reserved
            u32be(o, 0); // duration
            o.extend_from_slice(&[0; 8]); // reserved
            u16be(o, 0); // layer
            u16be(o, 0); // alternate_group
            u16be(o, volume);
            u16be(o, 0); // reserved
            UNITY_MATRIX.iter().for_each(|&v| u32be(o, v));
            u32be(o, width << 16);
            u32be(o, height << 16);
        });
        write_box(o, b"mdia", |o| {
            write_full_box(o, b"mdhd", 0, 0, |o| {
                u32be(o, 0);
                u32be(o, 0);
                u32be(o, timescale);
                u32be(o, 0);
                u16be(o, 0x55c4); // language "und"
                u16be(o, 0);
            });
            write_full_box(o, b"hdlr", 0, 0, |o| {
                u32be(o, 0); // pre_defined
                o.extend_from_slice(handler);
                o.extend_from_slice(&[0; 12]);
                o.extend_from_slice(name);
            });
            write_box(o, b"minf", |o| {
                match track {
                    Track::Video(_) => {
                        write_full_box(o, b"vmhd", 0, 1, |o| o.extend_from_slice(&[0; 8]));
                    }
                    // balance, reserved
                    Track::Audio(_) => {
                        write_full_box(o, b"smhd", 0, 0, |o| o.extend_from_slice(&[0; 4]));
                    }
                }
                write_box(o, b"dinf", |o| {
                    write_full_box(o, b"dref", 0, 0, |o| {
                        u32be(o, 1);
                        write_full_box(o, b"url ", 0, 1, |_| {});
                    });
                });
                write_box(o, b"stbl", |o| {
                    write_full_box(o, b"stsd", 0, 0, |o| {
                        u32be(o, 1);
                        o.extend_from_slice(entry);
                    });
                    write_full_box(o, b"stts", 0, 0, |o| u32be(o, 0));
                    write_full_box(o, b"stsc", 0, 0, |o| u32be(o, 0));
                    write_full_box(o, b"stsz", 0, 0, |o| {
                        u32be(o, 0);
                        u32be(o, 0);
                    });
                    write_full_box(o, b"stco", 0, 0, |o| u32be(o, 0));
                });
            });
        });
    });
}

/// Builds the initialization segment (`ftyp` + `moov`): the video track
/// ([`TRACK_ID`]) and, with `audio`, an audio track ([`AUDIO_TRACK_ID`]).
pub fn init_segment_av(params: &VideoParams, audio: Option<&AudioParams>) -> Vec<u8> {
    let mut out = Vec::with_capacity(1024 + params.sample_entry.len());
    write_box(&mut out, b"ftyp", |o| {
        o.extend_from_slice(b"isom");
        u32be(o, 0x200);
        for brand in [b"isom", b"iso5", b"iso6", b"mp41"] {
            o.extend_from_slice(brand);
        }
    });
    write_box(&mut out, b"moov", |o| {
        write_full_box(o, b"mvhd", 0, 0, |o| {
            u32be(o, 0); // creation_time
            u32be(o, 0); // modification_time
            u32be(o, 1000); // timescale
            u32be(o, 0); // duration: unknown (fragmented)
            u32be(o, 0x0001_0000); // rate 1.0
            u16be(o, 0x0100); // volume 1.0
            o.extend_from_slice(&[0; 10]); // reserved
            UNITY_MATRIX.iter().for_each(|&v| u32be(o, v));
            o.extend_from_slice(&[0; 24]); // pre_defined
            // next_track_ID
            u32be(
                o,
                if audio.is_some() {
                    AUDIO_TRACK_ID
                } else {
                    TRACK_ID
                } + 1,
            );
        });
        write_trak(o, TRACK_ID, &Track::Video(params));
        if let Some(a) = audio {
            write_trak(o, AUDIO_TRACK_ID, &Track::Audio(a));
        }
        write_box(o, b"mvex", |o| {
            for id in std::iter::once(TRACK_ID).chain(audio.map(|_| AUDIO_TRACK_ID)) {
                write_full_box(o, b"trex", 0, 0, |o| {
                    u32be(o, id);
                    u32be(o, 1); // default_sample_description_index
                    u32be(o, 0); // default_sample_duration
                    u32be(o, 0); // default_sample_size
                    u32be(o, 0); // default_sample_flags
                });
            }
        });
    });
    out
}

/// Builds `moof` and the 8-byte `mdat` header of a video-only fragment.
/// The caller writes the sample data, in order, right after the returned bytes.
///
/// `base_decode_time` is the decode time of the first sample, in timescale
/// units, relative to the start of the segment.
///
/// # Panics
///
/// Panics if `samples` is empty or the total size exceeds 4 GiB, both of
/// which are programming errors in the caller.
pub fn fragment_header(sequence: u32, base_decode_time: u64, samples: &[SampleMeta]) -> Vec<u8> {
    fragment_header_av(sequence, base_decode_time, samples, None)
}

/// Writes one `traf`; pushes the position of its `data_offset` field.
fn write_traf(
    o: &mut Vec<u8>,
    track_id: u32,
    base_decode_time: u64,
    samples: &[SampleMeta],
    offset_positions: &mut Vec<usize>,
) {
    write_box(o, b"traf", |o| {
        write_full_box(o, b"tfhd", 0, TFHD_DEFAULT_BASE_IS_MOOF, |o| {
            u32be(o, track_id)
        });
        write_full_box(o, b"tfdt", 1, 0, |o| u64be(o, base_decode_time));
        write_full_box(o, b"trun", 0, TRUN_FLAGS, |o| {
            u32be(o, u32::try_from(samples.len()).unwrap_or(u32::MAX));
            offset_positions.push(o.len());
            u32be(o, 0); // data_offset, patched by the caller
            for s in samples {
                u32be(o, s.duration);
                u32be(o, s.size);
                u32be(
                    o,
                    if s.is_key {
                        SYNC_SAMPLE_FLAGS
                    } else {
                        NON_SYNC_SAMPLE_FLAGS
                    },
                );
            }
        });
    });
}

/// Like [`fragment_header`], with an optional audio run `(base_decode_time,
/// samples)` in the same fragment (in the audio timescale). The caller writes
/// the video samples then the audio samples right after the returned bytes.
///
/// # Panics
///
/// Panics if `samples` is empty or the total size exceeds 4 GiB.
pub fn fragment_header_av(
    sequence: u32,
    base_decode_time: u64,
    samples: &[SampleMeta],
    audio: Option<(u64, &[SampleMeta])>,
) -> Vec<u8> {
    assert!(!samples.is_empty(), "empty fragment");
    let audio = audio.filter(|(_, a)| !a.is_empty());
    let video_len: u64 = samples.iter().map(|s| u64::from(s.size)).sum();
    let audio_len: u64 = audio.map_or(0, |(_, a)| a.iter().map(|s| u64::from(s.size)).sum());
    let mdat_size = u32::try_from(video_len + audio_len + 8).expect("fragment larger than 4 GiB");

    let mut out = Vec::with_capacity(160 + 12 * samples.len());
    let mut offset_positions = Vec::with_capacity(2);
    write_box(&mut out, b"moof", |o| {
        write_full_box(o, b"mfhd", 0, 0, |o| u32be(o, sequence));
        write_traf(
            o,
            TRACK_ID,
            base_decode_time,
            samples,
            &mut offset_positions,
        );
        if let Some((base, a)) = audio {
            write_traf(o, AUDIO_TRACK_ID, base, a, &mut offset_positions);
        }
    });
    // Data starts after moof and the mdat header; offsets are relative to
    // moof. Video samples come first, then audio samples.
    let video_offset = out.len() as u64 + 8;
    for (i, pos) in offset_positions.into_iter().enumerate() {
        let offset = if i == 0 {
            video_offset
        } else {
            video_offset + video_len
        };
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        out[pos..pos + 4].copy_from_slice(&offset.to_be_bytes());
    }
    u32be(&mut out, mdat_size);
    out.extend_from_slice(b"mdat");
    out
}
