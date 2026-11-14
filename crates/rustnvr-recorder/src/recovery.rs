//! Repair of segments interrupted by a crash, power loss or write error.

use crate::scan::scan;
use crate::store::{PART_SUFFIX, sync_dir};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// What happened to one `.mp4.part` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Truncated after its last complete fragment and renamed.
    Recovered {
        /// Final path.
        path: PathBuf,
        /// Number of complete fragments kept.
        fragments: usize,
        /// Bytes removed from the end of the file.
        truncated_bytes: u64,
    },
    /// Deleted because it contained no complete fragment.
    Deleted,
}

impl RecoveryOutcome {
    /// Path of the kept file, if any.
    pub fn kept_path(&self) -> Option<PathBuf> {
        match self {
            RecoveryOutcome::Recovered { path, .. } => Some(path.clone()),
            RecoveryOutcome::Deleted => None,
        }
    }
}

/// Truncates `part` to its last complete fragment and renames it to drop the
/// `.part` suffix, or deletes it if nothing usable remains.
pub fn recover_part_file(part: &Path) -> io::Result<RecoveryOutcome> {
    let report = {
        let mut f = fs::File::open(part)?;
        scan(&mut f)?
    };
    if report.fragments.is_empty() {
        fs::remove_file(part)?;
        return Ok(RecoveryOutcome::Deleted);
    }
    let f = OpenOptions::new().write(true).open(part)?;
    if report.valid_len < report.file_len {
        f.set_len(report.valid_len)?;
    }
    f.sync_all()?;
    drop(f);
    let name = part
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(PART_SUFFIX))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a .part file"))?;
    let mut final_path = part.with_file_name(name);
    // Do not clobber an existing complete segment.
    let mut n = 1;
    while final_path.exists() {
        let stem = name.strip_suffix(".mp4").unwrap_or(name);
        final_path = part.with_file_name(format!("{stem}-recovered{n}.mp4"));
        n += 1;
    }
    fs::rename(part, &final_path)?;
    sync_dir(&final_path);
    Ok(RecoveryOutcome::Recovered {
        path: final_path,
        fragments: report.fragments.len(),
        truncated_bytes: report.file_len - report.valid_len,
    })
}

/// Summary of [`recover_tree`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Files recovered.
    pub recovered: Vec<PathBuf>,
    /// Files deleted (no complete fragment).
    pub deleted: Vec<PathBuf>,
    /// Files that could not be processed, with the error.
    pub failed: Vec<(PathBuf, String)>,
}

/// Recovers every `.mp4.part` file below `root`. A missing `root` is not an error.
pub fn recover_tree(root: &Path) -> RecoveryReport {
    recover_tree_where(root, |_| true)
}

/// Recovers the `.mp4.part` files below `root` left untouched for `idle` at
/// least: those of segments interrupted while RustNVR runs (a network share
/// that went away in the middle of a segment), never one still being
/// written, which changes every few seconds.
pub fn recover_stale_tree(root: &Path, idle: std::time::Duration) -> RecoveryReport {
    let now = std::time::SystemTime::now();
    recover_tree_where(root, |path| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= idle)
    })
}

fn recover_tree_where(root: &Path, wanted: impl Fn(&Path) -> bool) -> RecoveryReport {
    let mut report = RecoveryReport::default();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                report.failed.push((dir, e.to_string()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => dirs.push(path),
                Ok(t)
                    if t.is_file()
                        && path
                            .to_string_lossy()
                            .ends_with(&format!(".mp4{PART_SUFFIX}"))
                        && wanted(&path) =>
                {
                    match recover_part_file(&path) {
                        Ok(RecoveryOutcome::Recovered { path: p, .. }) => report.recovered.push(p),
                        Ok(RecoveryOutcome::Deleted) => report.deleted.push(path),
                        Err(e) => report.failed.push((path, e.to_string())),
                    }
                }
                _ => {}
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// Only the segments left untouched for a while are recovered: one still
    /// being written is never touched.
    #[test]
    fn recovers_only_stale_part_files() {
        let dir = tempfile::tempdir().expect("tmp");
        let day = dir.path().join("jardin").join("2026-11-13");
        fs::create_dir_all(&day).expect("dir");
        let old = day.join("20261113T100000.000Z.mp4.part");
        let fresh = day.join("20261113T101000.000Z.mp4.part");
        fs::write(&old, b"").expect("old");
        fs::write(&fresh, b"").expect("fresh");
        fs::File::options()
            .write(true)
            .open(&old)
            .and_then(|f| f.set_modified(SystemTime::now() - Duration::from_secs(600)))
            .expect("age");
        let report = recover_stale_tree(dir.path(), Duration::from_secs(300));
        // Nothing complete in it: deleted.
        assert_eq!(report.deleted, vec![old.clone()]);
        assert!(!old.exists() && fresh.exists());
        assert!(report.failed.is_empty());
    }
}
