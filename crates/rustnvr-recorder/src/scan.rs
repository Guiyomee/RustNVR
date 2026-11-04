//! Fragment-level parser for the files written by [`crate::fmp4`].
//!
//! Used by crash recovery to find the end of the last complete fragment, and
//! by tests to check what was written. It validates structure (box sizes,
//! `moof` followed by an `mdat` of the announced size) but not codec data.

use std::io::{self, Read, Seek, SeekFrom};

/// One complete `moof` + `mdat` pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentSummary {
    /// Offset of the `moof` box.
    pub offset: u64,
    /// Length of `moof` + `mdat`.
    pub len: u64,
    /// `mfhd` sequence number.
    pub sequence: u32,
    /// `tfdt` base media decode time.
    pub base_decode_time: u64,
    /// Sum of sample durations.
    pub duration: u64,
    /// Number of samples.
    pub samples: u32,
    /// Number of sync samples.
    pub key_samples: u32,
    /// Whether the first sample is a sync sample.
    pub starts_with_key: bool,
    /// Number of audio samples (other tracks than the video track).
    pub audio_samples: u32,
}

/// Result of [`scan`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// End offset of `ftyp` + `moov`, or 0 if the initialization segment is incomplete.
    pub init_len: u64,
    /// Track timescale from `mdhd`.
    pub timescale: Option<u32>,
    /// Sample entry four-character code (`avc1`, `hvc1`, …).
    pub sample_entry: Option<[u8; 4]>,
    /// The file has an audio track.
    pub has_audio: bool,
    /// Complete fragments, in file order.
    pub fragments: Vec<FragmentSummary>,
    /// Length of the valid prefix of the file (end of the last complete fragment).
    pub valid_len: u64,
    /// Actual file length.
    pub file_len: u64,
}

impl ScanReport {
    /// Total media duration in timescale units.
    pub fn duration(&self) -> u64 {
        self.fragments.iter().map(|f| f.duration).sum()
    }

    /// Total number of samples.
    pub fn samples(&self) -> u64 {
        self.fragments.iter().map(|f| u64::from(f.samples)).sum()
    }
}

struct BoxHeader {
    fourcc: [u8; 4],
    header_len: u64,
    size: u64,
}

fn read_header<R: Read>(r: &mut R, remaining: u64) -> io::Result<Option<BoxHeader>> {
    if remaining < 8 {
        return Ok(None);
    }
    let mut h = [0u8; 8];
    r.read_exact(&mut h)?;
    let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
    let fourcc = [h[4], h[5], h[6], h[7]];
    let (size, header_len) = match size32 {
        0 => return Ok(None), // "extends to end of file": never written by us
        1 => {
            if remaining < 16 {
                return Ok(None);
            }
            let mut l = [0u8; 8];
            r.read_exact(&mut l)?;
            (u64::from_be_bytes(l), 16)
        }
        n => (u64::from(n), 8),
    };
    if size < header_len {
        return Ok(None);
    }
    Ok(Some(BoxHeader {
        fourcc,
        header_len,
        size,
    }))
}

/// Iterates over the child boxes in `data`, yielding `(fourcc, body)`.
fn children(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        if data.len() < 8 {
            return None;
        }
        let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if size < 8 || size > data.len() {
            return None;
        }
        let fourcc = [data[4], data[5], data[6], data[7]];
        let body = &data[8..size];
        data = &data[size..];
        Some((fourcc, body))
    })
}

/// Body of the box reached by following `path` (e.g. `[b"trak", b"mdia"]`)
/// through nested boxes of `data`, if present.
pub fn find_box<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    let (first, rest) = path.split_first()?;
    let body = children(data).find(|(f, _)| f == *first)?.1;
    if rest.is_empty() {
        Some(body)
    } else {
        find_box(body, rest)
    }
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_be_bytes)
}

struct MoofInfo {
    sequence: u32,
    base_decode_time: u64,
    duration: u64,
    samples: u32,
    key_samples: u32,
    starts_with_key: bool,
    data_len: u64,
    audio_samples: u32,
}

/// Video statistics come from the video track (`track_ID` 1, or the first
/// `traf`); the data length covers every track (audio is in the same `mdat`).
fn parse_moof(body: &[u8]) -> Option<MoofInfo> {
    let mfhd = find_box(body, &[b"mfhd"])?;
    let sequence = be32(mfhd, 4)?;
    let mut video: Option<MoofInfo> = None;
    let (mut data_len, mut all_samples) = (0, 0);
    for (fourcc, traf) in children(body) {
        if &fourcc != b"traf" {
            continue;
        }
        let info = parse_traf(traf, sequence)?;
        data_len += info.data_len;
        all_samples += info.samples;
        let track_id = find_box(traf, &[b"tfhd"]).and_then(|t| be32(t, 4));
        if video.is_none() || track_id == Some(crate::fmp4::TRACK_ID) {
            video = Some(info);
        }
    }
    let mut info = video?;
    info.data_len = data_len;
    info.audio_samples = all_samples - info.samples;
    Some(info)
}

fn parse_traf(traf: &[u8], sequence: u32) -> Option<MoofInfo> {
    let tfdt = find_box(traf, &[b"tfdt"])?;
    let base_decode_time = if tfdt[0] == 1 {
        be64(tfdt, 4)?
    } else {
        u64::from(be32(tfdt, 4)?)
    };
    let trun = find_box(traf, &[b"trun"])?;
    let flags = be32(trun, 0)? & 0x00ff_ffff;
    let samples = be32(trun, 4)?;
    let mut pos = 8;
    if flags & 0x1 != 0 {
        pos += 4; // data_offset
    }
    let first_sample_flags = if flags & 0x4 != 0 {
        let f = be32(trun, pos)?;
        pos += 4;
        Some(f)
    } else {
        None
    };
    let (mut duration, mut data_len, mut key_samples, mut starts_with_key) =
        (0u64, 0u64, 0u32, false);
    for i in 0..samples {
        let mut sample_flags = 0;
        if flags & 0x100 != 0 {
            duration += u64::from(be32(trun, pos)?);
            pos += 4;
        }
        if flags & 0x200 != 0 {
            data_len += u64::from(be32(trun, pos)?);
            pos += 4;
        }
        if flags & 0x400 != 0 {
            sample_flags = be32(trun, pos)?;
            pos += 4;
        } else if i == 0
            && let Some(f) = first_sample_flags
        {
            sample_flags = f;
        }
        if flags & 0x800 != 0 {
            pos += 4;
        }
        let is_key = sample_flags & 0x0001_0000 == 0;
        key_samples += u32::from(is_key);
        if i == 0 {
            starts_with_key = is_key;
        }
    }
    Some(MoofInfo {
        sequence,
        base_decode_time,
        duration,
        samples,
        key_samples,
        starts_with_key,
        data_len,
        audio_samples: 0,
    })
}

/// Scans a segment file. Stops at the first incomplete or inconsistent box;
/// everything before [`ScanReport::valid_len`] is a well-formed file.
pub fn scan<R: Read + Seek>(r: &mut R) -> io::Result<ScanReport> {
    let file_len = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(0))?;
    let mut report = ScanReport {
        file_len,
        ..ScanReport::default()
    };
    let mut pos = 0u64;
    let (mut seen_ftyp, mut pending_moof) = (false, None::<(u64, MoofInfo)>);

    while let Some(h) = read_header(r, file_len - pos)? {
        if pos + h.size > file_len {
            break; // truncated box
        }
        let body_len = h.size - h.header_len;
        match &h.fourcc {
            b"ftyp" if pos == 0 => {
                seen_ftyp = true;
                r.seek(SeekFrom::Current(body_len as i64))?;
            }
            b"moov" if seen_ftyp && report.init_len == 0 => {
                let mut body = vec![0; usize::try_from(body_len).unwrap_or(0)];
                r.read_exact(&mut body)?;
                report.timescale = find_box(&body, &[b"trak", b"mdia", b"mdhd"])
                    .and_then(|m| if m[0] == 1 { be32(m, 20) } else { be32(m, 12) });
                report.sample_entry =
                    find_box(&body, &[b"trak", b"mdia", b"minf", b"stbl", b"stsd"])
                        .and_then(|s| s.get(12..16))
                        .and_then(|f| f.try_into().ok());
                report.has_audio = children(&body).any(|(f, trak)| {
                    &f == b"trak"
                        && find_box(trak, &[b"mdia", b"hdlr"]).and_then(|h| h.get(8..12))
                            == Some(&b"soun"[..])
                });
                report.init_len = pos + h.size;
                report.valid_len = report.init_len;
            }
            b"moof" if report.init_len > 0 && pending_moof.is_none() && body_len <= 16 << 20 => {
                let mut body = vec![0; usize::try_from(body_len).unwrap_or(0)];
                r.read_exact(&mut body)?;
                let Some(info) = parse_moof(&body) else {
                    break;
                };
                pending_moof = Some((pos, info));
            }
            b"mdat" if pending_moof.is_some() => {
                let Some((offset, info)) = pending_moof.take() else {
                    break;
                };
                if body_len != info.data_len {
                    break; // mdat does not match its moof
                }
                report.fragments.push(FragmentSummary {
                    offset,
                    len: pos + h.size - offset,
                    sequence: info.sequence,
                    base_decode_time: info.base_decode_time,
                    duration: info.duration,
                    samples: info.samples,
                    key_samples: info.key_samples,
                    starts_with_key: info.starts_with_key,
                    audio_samples: info.audio_samples,
                });
                report.valid_len = pos + h.size;
                r.seek(SeekFrom::Current(body_len as i64))?;
            }
            _ => break, // unexpected box: stop at the last known-good point
        }
        pos += h.size;
    }
    Ok(report)
}
