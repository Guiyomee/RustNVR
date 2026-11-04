//! Recording listing from the files on disk.
//!
//! From the SQLite index when the database is available ([`list_indexed`]);
//! otherwise (and as reference) segments are found by walking
//! `<root>/<camera>/<YYYY-MM-DD>/`, their start comes from the file name and
//! their duration from the fMP4 fragments (cached per file size).

use crate::dto::RecordingDto;
use crate::state::ApiState;
use rustnvr_recorder::scan::scan;
use std::path::Path;

/// Longest range accepted by a listing request.
pub(crate) const MAX_RANGE_MS: i64 = 7 * 24 * 3600 * 1000;
/// Segments are listed from the day before `from`, since one may start
/// before midnight and end after it.
const DAY_MS: i64 = 24 * 3600 * 1000;

/// Parses `YYYYMMDDTHHMMSS.mmmZ[-suffix].mp4` into Unix milliseconds.
pub(crate) fn parse_segment_name(name: &str) -> Option<i64> {
    rustnvr_recorder::store::parse_segment_start_ms(name)
}

/// Whether `day` looks like `YYYY-MM-DD`.
pub(crate) fn valid_day(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn day_of(ms: i64) -> Option<String> {
    let ts = jiff::Timestamp::from_millisecond(ms).ok()?;
    Some(ts.strftime("%Y-%m-%d").to_string())
}

/// Duration in milliseconds of a finished segment, from its fragments.
pub(crate) fn segment_duration_ms(state: &ApiState, path: &Path, len: u64) -> Option<u64> {
    let cached = state
        .durations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(path)
        .copied();
    if let Some((cached_len, ms)) = cached
        && cached_len == len
    {
        return Some(ms);
    }
    let report = scan(&mut std::fs::File::open(path).ok()?).ok()?;
    let timescale = u64::from(report.timescale?.max(1));
    let ms = report.duration() * 1000 / timescale;
    state
        .durations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_owned(), (len, ms));
    Some(ms)
}

/// Like [`list`], from the SQLite index (`None` without database or on
/// error). Rows whose file no longer exists are skipped (the next
/// reconciliation removes them).
pub(crate) async fn list_indexed(
    state: &std::sync::Arc<ApiState>,
    camera: &str,
    from_ms: i64,
    to_ms: i64,
) -> Option<Vec<RecordingDto>> {
    let db = state.database.get()?;
    let mut rows = db.segments(camera, from_ms, to_ms).await.ok()?;
    // Local disk first, then the other destinations in configuration order.
    let order: Vec<(String, std::path::PathBuf, bool)> = state
        .destinations()
        .iter()
        .map(|d| (d.id.clone(), d.root.clone(), d.destination().is_available()))
        .collect();
    let rank = |id: &str| {
        order
            .iter()
            .position(|(d, _, _)| d == id)
            .unwrap_or(usize::MAX)
    };
    rows.sort_by_key(|r| (r.start_ms, rank(&r.destination)));
    tokio::task::spawn_blocking(move || {
        let mut seen = std::collections::HashSet::new();
        rows.into_iter()
            .filter_map(|r| {
                let (_, root, available) = order.iter().find(|(d, _, _)| *d == r.destination)?;
                if !available || seen.contains(&r.start_ms) {
                    return None;
                }
                if !root.join(&r.camera).join(&r.day).join(&r.file).is_file() {
                    return None;
                }
                seen.insert(r.start_ms);
                let duration_ms = u64::try_from(r.end_ms - r.start_ms).unwrap_or(0);
                Some(RecordingDto {
                    url: format!(
                        "/api/v1/recordings/{}/{}/{}/{}",
                        r.destination, r.camera, r.day, r.file
                    ),
                    camera: r.camera,
                    start_ms: r.start_ms,
                    end_ms: r.end_ms,
                    duration_ms,
                    bytes: r.bytes,
                })
            })
            .collect::<Vec<_>>()
    })
    .await
    .ok()
}

/// Finished segments of `camera` overlapping `[from_ms, to_ms)` on every
/// available destination, sorted by start. A segment recorded on several
/// destinations is listed once, from the first one (the local disk first).
/// Blocking: call from a blocking thread.
pub(crate) fn list(state: &ApiState, camera: &str, from_ms: i64, to_ms: i64) -> Vec<RecordingDto> {
    let mut out = Vec::new();
    let mut day_ms = from_ms - DAY_MS;
    let mut days = Vec::new();
    while day_ms < to_ms + DAY_MS {
        if let Some(d) = day_of(day_ms)
            && days.last() != Some(&d)
        {
            days.push(d);
        }
        day_ms += DAY_MS;
    }
    let mut seen = std::collections::HashSet::new();
    let destinations = state.destinations();
    for dest in destinations
        .iter()
        .filter(|d| d.destination().is_available())
    {
        for day in &days {
            let dir = dest.root.join(camera).join(day);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(start_ms) = parse_segment_name(name) else {
                    continue;
                };
                let Ok(meta) = entry.metadata() else { continue };
                if start_ms >= to_ms || !meta.is_file() || seen.contains(&start_ms) {
                    continue;
                }
                let Some(duration_ms) = segment_duration_ms(state, &entry.path(), meta.len())
                else {
                    continue;
                };
                let end_ms = start_ms + i64::try_from(duration_ms).unwrap_or(i64::MAX);
                if end_ms <= from_ms {
                    continue;
                }
                seen.insert(start_ms);
                out.push(RecordingDto {
                    camera: camera.to_owned(),
                    start_ms,
                    end_ms,
                    duration_ms,
                    bytes: meta.len(),
                    url: format!("/api/v1/recordings/{}/{camera}/{day}/{name}", dest.id),
                });
            }
        }
    }
    out.sort_by_key(|r| r.start_ms);
    out
}

/// Length of the initialization part of a segment file (`ftyp` + `moov`,
/// everything before the first `moof`), read from the box headers. Blocking.
pub(crate) fn init_length(path: &Path) -> Option<u64> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let size32 = u64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]));
        if &h[4..8] == b"moof" {
            return Some(pos);
        }
        let size = match size32 {
            1 => {
                f.read_exact(&mut h[8..16]).ok()?;
                u64::from_be_bytes(h[8..16].try_into().ok()?)
            }
            0 => return None,
            n => n,
        };
        if size < 8 {
            return None;
        }
        pos += size;
    }
    None
}

/// HLS media playlist (VOD) of the given segments, oldest first: one media
/// segment per file, addressed by byte ranges of the file (no copy). Files
/// restart their timestamps, hence a discontinuity before each.
pub(crate) fn hls_playlist(segments: &[(RecordingDto, u64)]) -> String {
    use std::fmt::Write as _;
    let target = segments
        .iter()
        .map(|(r, _)| r.duration_ms.div_ceil(1000))
        .max()
        .unwrap_or(1)
        .max(1);
    let mut m = format!(
        "#EXTM3U
#EXT-X-VERSION:7
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-TARGETDURATION:{target}
#EXT-X-MEDIA-SEQUENCE:0
"
    );
    for (i, (r, init)) in segments.iter().enumerate() {
        if i > 0 {
            m.push_str(
                "#EXT-X-DISCONTINUITY
",
            );
        }
        let date = jiff::Timestamp::from_millisecond(r.start_ms)
            .map(|t| t.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
            .unwrap_or_default();
        let _ = write!(
            m,
            "#EXT-X-PROGRAM-DATE-TIME:{date}
#EXT-X-MAP:URI=\"{url}\",BYTERANGE=\"{init}@0\"
#EXTINF:{dur:.3},
#EXT-X-BYTERANGE:{len}@{init}
{url}
",
            url = r.url,
            dur = r.duration_ms as f64 / 1000.0,
            len = r.bytes.saturating_sub(*init),
        );
    }
    m.push_str(
        "#EXT-X-ENDLIST
",
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_hls_playlists_from_segment_files() {
        use rustnvr_recorder::fmp4::{self, SampleMeta};
        let params = rustnvr_core::VideoParams {
            codec: rustnvr_core::VideoCodec::H264,
            rfc6381_codec: "avc1.42C01E".into(),
            width: 320,
            height: 240,
            clock_rate: 90_000,
            sample_entry: bytes::Bytes::from_static(&[0, 0, 0, 8, b'a', b'v', b'c', b'1']),
        };
        let init = fmp4::init_segment(&params);
        let mut file = init.clone();
        let meta = [SampleMeta {
            duration: 90_000,
            size: 4,
            is_key: true,
        }];
        file.extend_from_slice(&fmp4::fragment_header(1, 0, &meta));
        file.extend_from_slice(&[0, 0, 0, 0]);
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("s.mp4");
        std::fs::write(&path, &file).expect("write");
        assert_eq!(init_length(&path), Some(init.len() as u64));
        assert_eq!(init_length(Path::new("/nonexistent")), None);

        let rec = |start: i64, url: &str| RecordingDto {
            camera: "a".into(),
            start_ms: start,
            end_ms: start + 60_000,
            duration_ms: 60_000,
            bytes: 1_000,
            url: url.into(),
        };
        let m = hls_playlist(&[
            (rec(1_793_613_600_000, "/r/1.mp4"), 100),
            (rec(1_793_613_660_000, "/r/2.mp4"), 120),
        ]);
        assert!(m.starts_with(
            "#EXTM3U
#EXT-X-VERSION:7
#EXT-X-PLAYLIST-TYPE:VOD"
        ));
        assert!(m.contains(
            "#EXT-X-MAP:URI=\"/r/1.mp4\",BYTERANGE=\"100@0\"
#EXTINF:60.000,
#EXT-X-BYTERANGE:900@100
/r/1.mp4"
        ));
        assert!(m.contains("#EXT-X-PROGRAM-DATE-TIME:2026-11-02T10:00:00.000Z"));
        assert_eq!(m.matches("#EXT-X-DISCONTINUITY").count(), 1);
        assert!(m.ends_with(
            "#EXT-X-ENDLIST
"
        ));
    }

    #[test]
    fn parses_names() {
        assert_eq!(
            parse_segment_name("20261101T182233.120Z.mp4"),
            Some(1_793_557_353_120)
        );
        assert_eq!(
            parse_segment_name("20261101T182233.120Z-1.mp4"),
            Some(1_793_557_353_120)
        );
        assert_eq!(
            parse_segment_name("20261101T182233.120Z-recovered2.mp4"),
            Some(1_793_557_353_120)
        );
        for bad in [
            "20261101T182233.120Z.mp4.part",
            "20261301T182233.120Z.mp4",
            "20261101T182233.120Z-.mp4",
            "20261101T182233.120Z-x.mp4",
            "../../etc/passwd.mp4",
            "x.mp4",
        ] {
            assert_eq!(parse_segment_name(bad), None, "{bad}");
        }
        assert!(valid_day("2026-11-01"));
        assert!(!valid_day("2026-11-1"));
        assert!(!valid_day("../../../x"));
    }
}
