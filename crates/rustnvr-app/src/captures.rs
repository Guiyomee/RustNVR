//! Pictures analysed by the object recognition, kept to replay the AI on
//! them later: `<root>/.captures/<camera>/<YYYY-MM-DD>/<time>-<source>.jpg`
//! (UTC day, as the recordings), each with a `.json` of what was found.
//!
//! The folder starts with a dot: the scans of the recordings (retention,
//! index) skip it, as `.rustnvr`. Every name coming from the API is checked
//! before it is joined to a path.

use rustnvr_core::{CameraId, DetectedObject};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Folder of the kept pictures, at the root of a destination.
pub const DIR: &str = ".captures";
/// Pictures replayed at most at once.
pub const MAX_REPLAY: usize = 200;

/// What is known of a kept picture (its `.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    /// When it was analysed, in milliseconds since the Unix epoch.
    pub at_ms: i64,
    /// Event whose picture it is, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<i64>,
    /// Picture number within the event (1 = the first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<u32>,
    /// Crossing line it was checking, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Recognition in use (`onnx`, `deepstack_http`) and its model.
    pub provider: String,
    /// Time taken by the analysis.
    pub ms: u64,
    /// Objects returned by the recognition (after its own score and label
    /// filters).
    pub found: Vec<DetectedObject>,
    /// Labels that counted for the camera (objects of the camera, zones,
    /// line), the ones the event or the crossing kept.
    pub kept: Vec<String>,
}

/// Pictures of one camera on one day.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Day {
    /// Camera identifier.
    pub camera: String,
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    /// Pictures.
    pub count: usize,
    /// Size on disk, `.json` included.
    pub bytes: u64,
}

fn valid_day(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn valid_file(file: &str) -> bool {
    file.ends_with(".jpg")
        && !file.starts_with('.')
        && file.len() < 120
        && file
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

/// `<root>/.captures/<camera>/<day>`, if both names are valid.
fn day_dir(root: &Path, camera: &str, day: &str) -> io::Result<PathBuf> {
    let camera = CameraId::new(camera).map_err(|_| invalid("caméra inconnue"))?;
    if !valid_day(day) {
        return Err(invalid("jour invalide (AAAA-MM-JJ attendu)"));
    }
    Ok(root.join(DIR).join(camera.as_str()).join(day))
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_owned())
}

/// A source name usable in a file name: letters, digits and `_`.
fn file_part(s: &str) -> String {
    let part: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if part.is_empty() {
        "_".to_owned()
    } else {
        part
    }
}

/// Keeps `jpeg` and what was found in it. Blocking.
pub fn save(root: &Path, camera: &CameraId, jpeg: &[u8], capture: &Capture) -> io::Result<PathBuf> {
    let at = jiff::Timestamp::from_millisecond(capture.at_ms).map_err(io::Error::other)?;
    let utc = at.to_zoned(jiff::tz::TimeZone::UTC);
    let dir = day_dir(root, camera.as_str(), &utc.date().to_string())?;
    std::fs::create_dir_all(&dir)?;
    let source = match (&capture.event, &capture.line) {
        (Some(e), _) => format!("e{e}-p{}", capture.picture.unwrap_or(1)),
        (None, Some(line)) => format!("line-{}", file_part(line)),
        (None, None) => "test".to_owned(),
    };
    let stem = format!("{}-{source}", utc.strftime("%Y%m%dT%H%M%S%.3fZ"));
    let jpg = dir.join(format!("{stem}.jpg"));
    std::fs::write(&jpg, jpeg)?;
    let json = serde_json::to_vec_pretty(capture).map_err(io::Error::other)?;
    std::fs::write(dir.join(format!("{stem}.json")), json)?;
    Ok(jpg)
}

fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Every camera and day with kept pictures, the most recent day first.
/// Blocking.
pub fn days(root: &Path) -> Vec<Day> {
    let mut out = Vec::new();
    for (camera, cam_dir) in subdirs(&root.join(DIR)) {
        if CameraId::new(&camera).is_err() {
            continue;
        }
        for (day, dir) in subdirs(&cam_dir) {
            if !valid_day(&day) {
                continue;
            }
            let (mut count, mut bytes) = (0, 0);
            for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let Ok(meta) = f.metadata() else { continue };
                bytes += meta.len();
                if f.file_name().to_string_lossy().ends_with(".jpg") {
                    count += 1;
                }
            }
            out.push(Day {
                camera: camera.clone(),
                day,
                count,
                bytes,
            });
        }
    }
    out.sort_by(|a, b| b.day.cmp(&a.day).then_with(|| a.camera.cmp(&b.camera)));
    out
}

/// Pictures of `camera` on `day`, in time order, with what was found in
/// them (pictures without a readable `.json` are skipped). Blocking.
pub fn list(root: &Path, camera: &str, day: &str) -> io::Result<Vec<(String, Capture)>> {
    let dir = day_dir(root, camera, day)?;
    let mut out = Vec::new();
    for f in std::fs::read_dir(&dir)?.flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        if !valid_file(&name) {
            continue;
        }
        // Not `with_extension`: the time in the name has a dot.
        let json = dir.join(format!("{}.json", name.trim_end_matches(".jpg")));
        let Ok(text) = std::fs::read(&json) else {
            continue;
        };
        if let Ok(capture) = serde_json::from_slice::<Capture>(&text) {
            out.push((name, capture));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The JPEG `file` of `camera` on `day`. Blocking.
pub fn read(root: &Path, camera: &str, day: &str, file: &str) -> io::Result<Vec<u8>> {
    if !valid_file(file) {
        return Err(invalid("nom de fichier invalide"));
    }
    std::fs::read(day_dir(root, camera, day)?.join(file))
}

/// Deletes the pictures of `camera` on `day`; how many. Blocking.
pub fn delete_day(root: &Path, camera: &str, day: &str) -> io::Result<usize> {
    let dir = day_dir(root, camera, day)?;
    let count = list(root, camera, day).map_or(0, |l| l.len());
    std::fs::remove_dir_all(&dir)?;
    Ok(count)
}

/// Deletes the days older than `days` days before `now_ms`; how many days.
/// Blocking.
pub fn prune(root: &Path, days: u32, now_ms: i64) -> usize {
    let Ok(now) = jiff::Timestamp::from_millisecond(now_ms) else {
        return 0;
    };
    let today = now.to_zoned(jiff::tz::TimeZone::UTC).date();
    let Ok(oldest) = today.checked_sub(jiff::Span::new().days(i64::from(days))) else {
        return 0;
    };
    let oldest = oldest.to_string();
    let mut removed = 0;
    for (_, cam_dir) in subdirs(&root.join(DIR)) {
        for (day, dir) in subdirs(&cam_dir) {
            if valid_day(&day) && day < oldest && std::fs::remove_dir_all(&dir).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(at_ms: i64, event: Option<i64>, line: Option<&str>) -> Capture {
        Capture {
            at_ms,
            event,
            picture: event.map(|_| 2),
            line: line.map(str::to_owned),
            provider: "onnx yolox_s".into(),
            ms: 41,
            found: vec![DetectedObject {
                label: "person".into(),
                score: 0.81,
                bbox: Some([0.1, 0.2, 0.3, 0.9]),
                zone: None,
            }],
            kept: vec!["person".into()],
        }
    }

    #[test]
    fn keeps_lists_reads_and_prunes_pictures() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        let garage = CameraId::new("garage").expect("id");
        // 2026-11-05 13:07:21.110 UTC and the day before.
        let t = 1_793_884_041_110;
        let p = save(root, &garage, b"jpeg-1", &capture(t, Some(108), None)).expect("save");
        assert!(
            p.ends_with(".captures/garage/2026-11-05/20261105T130721.110Z-e108-p2.jpg"),
            "{}",
            p.display()
        );
        save(
            root,
            &garage,
            b"jpeg-2",
            &capture(t + 1000, None, Some("Porte du garage")),
        )
        .expect("save");
        save(
            root,
            &garage,
            b"old",
            &capture(t - 86_400_000, Some(1), None),
        )
        .expect("save");
        let summary = days(root);
        assert_eq!(
            summary
                .iter()
                .map(|d| (d.day.as_str(), d.count))
                .collect::<Vec<_>>(),
            [("2026-11-05", 2), ("2026-11-04", 1)]
        );
        let pictures = list(root, "garage", "2026-11-05").expect("list");
        assert_eq!(pictures.len(), 2);
        assert_eq!(pictures[0].1, capture(t, Some(108), None));
        assert!(pictures[1].0.ends_with("-line-porte_du_garage.jpg"));
        assert_eq!(
            read(root, "garage", "2026-11-05", &pictures[0].0).expect("read"),
            b"jpeg-1"
        );
        // Names from the API never leave the folder.
        assert!(read(root, "garage", "2026-11-05", "../../x.jpg").is_err());
        assert!(read(root, "../etc", "2026-11-05", "a.jpg").is_err());
        assert!(list(root, "garage", "2026-11-05/..").is_err());
        // Older than 0 days before 2026-11-05: the 4th goes.
        assert_eq!(prune(root, 0, t), 1);
        assert_eq!(days(root).len(), 1);
        assert_eq!(delete_day(root, "garage", "2026-11-05").expect("delete"), 2);
        assert!(days(root).is_empty());
    }
}
