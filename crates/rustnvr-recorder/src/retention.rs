//! Retention: deletes the oldest finished segments of a destination.
//!
//! Only finished segments (`<camera>/<YYYY-MM-DD>/<start>.mp4`) are
//! considered; segments being written (`.mp4.part`) are never touched. Age is
//! taken from the start time in the file name, so it does not depend on file
//! modification times (which copies or NAS clocks may change).

use crate::store::parse_segment_start_ms;
use rustnvr_core::config::RetentionConfig;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Limits applied to one destination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// Delete segments that started longer ago than this.
    pub max_age: Option<Duration>,
    /// Delete the oldest segments while the total exceeds this many bytes.
    pub max_bytes: Option<u64>,
}

impl From<RetentionConfig> for RetentionPolicy {
    fn from(c: RetentionConfig) -> Self {
        Self {
            max_age: c
                .retention_days
                .map(|d| Duration::from_secs(u64::from(d) * 86_400)),
            max_bytes: c.max_size_gb.map(|g| u64::from(g) * 1_000_000_000),
        }
    }
}

impl RetentionPolicy {
    /// Whether any limit is set.
    pub fn is_active(&self) -> bool {
        self.max_age.is_some() || self.max_bytes.is_some()
    }
}

/// What a retention pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// Segments deleted.
    pub deleted: u64,
    /// Bytes freed.
    pub freed_bytes: u64,
    /// Size of the segments kept.
    pub kept_bytes: u64,
    /// Files that could not be deleted.
    pub errors: Vec<String>,
}

/// Periods of recordings the user protected: never deleted, whatever the
/// retention or the free-space reserve.
#[derive(Clone, Debug, Default)]
pub struct Protected {
    /// `(camera, start_ms, end_ms)`.
    pub ranges: Vec<(String, i64, i64)>,
    /// Longest segment (ms): a segment started up to this long before a
    /// period may cover its start.
    pub segment_ms: i64,
    /// Keep every segment (the protected periods could not be read).
    pub everything: bool,
}

impl Protected {
    /// Whether the segment of `camera` started at `start_ms` holds protected
    /// pictures.
    pub fn covers(&self, camera: &str, start_ms: i64) -> bool {
        if self.everything {
            return true;
        }
        self.ranges.iter().any(|(c, from, to)| {
            c == camera && start_ms < *to && start_ms.saturating_add(self.segment_ms) > *from
        })
    }
}

struct Segment {
    start_ms: i64,
    bytes: u64,
    path: PathBuf,
}

impl Segment {
    /// `<root>/<camera>/<day>/<file>`: the camera directory name.
    fn camera(&self) -> &str {
        self.path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    }
}

/// Camera directories of a destination root.
fn camera_dirs(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect()
        },
    )
}

fn segments(camera_dirs: &[PathBuf]) -> Vec<Segment> {
    let mut out = Vec::new();
    for cam in camera_dirs {
        let Ok(days) = std::fs::read_dir(cam) else {
            continue;
        };
        for day in days
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        {
            let Ok(files) = std::fs::read_dir(day.path()) else {
                continue;
            };
            for f in files.flatten() {
                let name = f.file_name();
                let Some(start_ms) = name.to_str().and_then(parse_segment_start_ms) else {
                    continue;
                };
                let Ok(meta) = f.metadata() else { continue };
                if meta.is_file() {
                    out.push(Segment {
                        start_ms,
                        bytes: meta.len(),
                        path: f.path(),
                    });
                }
            }
        }
    }
    out
}

/// Deletes the oldest finished segments under `root` (every camera) until
/// at least `bytes` were freed or nothing is left, except the `protected`
/// ones. Blocking (disk I/O).
pub fn free_space(root: &Path, bytes: u64, protected: &Protected) -> RetentionReport {
    let mut report = RetentionReport::default();
    let mut all = segments(&camera_dirs(root));
    all.sort_by_key(|s| s.start_ms);
    let mut touched_dirs = Vec::new();
    let total: u64 = all.iter().map(|s| s.bytes).sum();
    for seg in &all {
        if report.freed_bytes >= bytes {
            break;
        }
        if protected.covers(seg.camera(), seg.start_ms) {
            continue;
        }
        match std::fs::remove_file(&seg.path) {
            Ok(()) => {
                report.deleted += 1;
                report.freed_bytes += seg.bytes;
                if let Some(dir) = seg.path.parent() {
                    touched_dirs.push(dir.to_owned());
                }
            }
            Err(e) => report.errors.push(format!("{}: {e}", seg.path.display())),
        }
    }
    touched_dirs.dedup();
    for dir in touched_dirs {
        let _ = std::fs::remove_dir(&dir);
    }
    report.kept_bytes = total - report.freed_bytes;
    report
}

/// Applies `policy` to the segments under `root` (every camera), except the
/// `protected` ones. Blocking (disk I/O).
pub fn enforce(
    root: &Path,
    policy: &RetentionPolicy,
    now: SystemTime,
    protected: &Protected,
) -> RetentionReport {
    enforce_dirs(&camera_dirs(root), policy, now, protected)
}

/// Applies `policy` to the segments of one camera directory
/// (`<root>/<camera>`), for per-camera retention, except the `protected`
/// ones. Blocking.
pub fn enforce_camera(
    dir: &Path,
    policy: &RetentionPolicy,
    now: SystemTime,
    protected: &Protected,
) -> RetentionReport {
    enforce_dirs(&[dir.to_owned()], policy, now, protected)
}

fn enforce_dirs(
    camera_dirs: &[PathBuf],
    policy: &RetentionPolicy,
    now: SystemTime,
    protected: &Protected,
) -> RetentionReport {
    let mut report = RetentionReport::default();
    let mut all = segments(camera_dirs);
    all.sort_by_key(|s| s.start_ms);
    let now_ms = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let cutoff = policy
        .max_age
        .map(|a| now_ms.saturating_sub(i64::try_from(a.as_millis()).unwrap_or(i64::MAX)));
    let mut total: u64 = all.iter().map(|s| s.bytes).sum();
    let mut touched_dirs = Vec::new();
    for seg in &all {
        let too_old = cutoff.is_some_and(|c| seg.start_ms < c);
        let over_quota = policy.max_bytes.is_some_and(|m| total > m);
        if !too_old && !over_quota {
            // Sorted by age: nothing newer can be too old, and the quota is met.
            break;
        }
        if protected.covers(seg.camera(), seg.start_ms) {
            continue;
        }
        match std::fs::remove_file(&seg.path) {
            Ok(()) => {
                report.deleted += 1;
                report.freed_bytes += seg.bytes;
                total -= seg.bytes;
                if let Some(dir) = seg.path.parent() {
                    touched_dirs.push(dir.to_owned());
                }
            }
            Err(e) => report.errors.push(format!("{}: {e}", seg.path.display())),
        }
    }
    // Remove day directories left empty (fails harmlessly if not empty).
    touched_dirs.dedup();
    for dir in touched_dirs {
        let _ = std::fs::remove_dir(&dir);
    }
    report.kept_bytes = total;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, cam: &str, day: &str, name: &str, len: usize) -> PathBuf {
        let dir = root.join(cam).join(day);
        std::fs::create_dir_all(&dir).expect("dir");
        let p = dir.join(name);
        std::fs::write(&p, vec![0u8; len]).expect("write");
        p
    }

    #[test]
    fn deletes_by_age_then_by_size_oldest_first() {
        let root = tempfile::tempdir().expect("tmp");
        let r = root.path();
        let old = touch(r, "a", "2026-10-01", "20261001T120000.000Z.mp4", 100);
        let mid = touch(r, "b", "2026-10-30", "20261030T120000.000Z.mp4", 100);
        let new1 = touch(r, "a", "2026-11-01", "20261101T100000.000Z.mp4", 100);
        let new2 = touch(r, "b", "2026-11-01", "20261101T110000.000Z.mp4", 100);
        let part = touch(r, "a", "2026-10-01", "20261001T130000.000Z.mp4.part", 100);
        let other = touch(r, "a", "2026-10-01", "notes.txt", 5);
        // 2026-11-01T12:00:00Z
        let now = UNIX_EPOCH + Duration::from_secs(1_793_534_400);

        let none = enforce(r, &RetentionPolicy::default(), now, &Protected::default());
        assert_eq!((none.deleted, none.kept_bytes), (0, 400));

        let by_age = RetentionPolicy {
            max_age: Some(Duration::from_secs(10 * 86_400)),
            max_bytes: None,
        };
        let rep = enforce(r, &by_age, now, &Protected::default());
        assert_eq!(
            (rep.deleted, rep.freed_bytes, rep.kept_bytes),
            (1, 100, 300)
        );
        assert!(!old.exists() && mid.exists());
        assert!(
            part.exists() && other.exists(),
            "only finished segments are deleted"
        );

        let by_size = RetentionPolicy {
            max_age: None,
            max_bytes: Some(150),
        };
        let rep = enforce(r, &by_size, now, &Protected::default());
        assert_eq!(rep.deleted, 2);
        assert!(!mid.exists() && !new1.exists() && new2.exists());
        assert!(
            !r.join("b").join("2026-10-30").exists(),
            "empty day directory removed"
        );
    }

    #[test]
    fn per_camera_retention_leaves_other_cameras_alone() {
        let root = tempfile::tempdir().expect("tmp");
        let r = root.path();
        let a = touch(r, "a", "2026-10-01", "20261001T120000.000Z.mp4", 100);
        let b = touch(r, "b", "2026-10-01", "20261001T120000.000Z.mp4", 100);
        let now = UNIX_EPOCH + Duration::from_secs(1_793_534_400);
        let week = RetentionPolicy {
            max_age: Some(Duration::from_secs(7 * 86_400)),
            max_bytes: None,
        };
        let rep = enforce_camera(&r.join("a"), &week, now, &Protected::default());
        assert_eq!(rep.deleted, 1);
        assert!(!a.exists() && b.exists());
    }

    #[test]
    fn frees_space_oldest_first_across_cameras() {
        let root = tempfile::tempdir().expect("tmp");
        let r = root.path();
        let old = touch(r, "b", "2026-10-01", "20261001T120000.000Z.mp4", 100);
        let mid = touch(r, "a", "2026-10-02", "20261002T120000.000Z.mp4", 100);
        let new = touch(r, "b", "2026-10-03", "20261003T120000.000Z.mp4", 100);
        let rep = free_space(r, 150, &Protected::default());
        assert_eq!(
            (rep.deleted, rep.freed_bytes, rep.kept_bytes),
            (2, 200, 100)
        );
        assert!(!old.exists() && !mid.exists() && new.exists());
        assert_eq!(free_space(r, 0, &Protected::default()).deleted, 0);
    }

    #[test]
    fn protected_segments_are_never_deleted() {
        let root = tempfile::tempdir().expect("tmp");
        let r = root.path();
        let kept = touch(r, "a", "2026-10-01", "20261001T120000.000Z.mp4", 100);
        let other_camera = touch(r, "b", "2026-10-01", "20261001T120000.000Z.mp4", 100);
        let next = touch(r, "a", "2026-10-01", "20261001T120100.000Z.mp4", 100);
        let start = parse_segment_start_ms("20261001T120000.000Z.mp4").expect("start");
        // 30 s protected in the middle of the first segment (60 s long).
        let protected = Protected {
            ranges: vec![("a".into(), start + 20_000, start + 50_000)],
            segment_ms: 60_000,
            everything: false,
        };
        assert!(protected.covers("a", start));
        assert!(!protected.covers("a", start + 60_000));
        assert!(!protected.covers("b", start));
        let now = UNIX_EPOCH + Duration::from_secs(1_793_534_400);
        let week = RetentionPolicy {
            max_age: Some(Duration::from_secs(7 * 86_400)),
            max_bytes: None,
        };
        let rep = enforce(r, &week, now, &protected);
        assert_eq!(rep.deleted, 2);
        assert!(kept.exists() && !other_camera.exists() && !next.exists());
        // The free-space reserve does not take it either.
        let rep = free_space(r, 1_000, &protected);
        assert_eq!(rep.deleted, 0);
        assert!(kept.exists());
    }

    #[test]
    fn policy_from_config() {
        let p = RetentionPolicy::from(RetentionConfig {
            retention_days: Some(7),
            max_size_gb: Some(2),
            min_free_gb: None,
        });
        assert_eq!(p.max_age, Some(Duration::from_secs(7 * 86_400)));
        assert_eq!(p.max_bytes, Some(2_000_000_000));
        assert!(!RetentionPolicy::default().is_active());
    }
}
