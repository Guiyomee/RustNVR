//! Segment files on disk.
//!
//! A segment is written to `NAME.mp4.part`, then on completion flushed,
//! fsynced and atomically renamed to `NAME.mp4` (and the directory fsynced).
//! The traits allow tests to inject failing writers (disk full).

use crate::recovery::recover_part_file;
use rustnvr_core::CameraId;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Suffix of segments being written.
pub const PART_SUFFIX: &str = ".part";

/// A segment being written.
pub trait SegmentFile: Write + Send {
    /// Path of the file being written.
    fn path(&self) -> &Path;
    /// Flushes, syncs and moves the file to its final name, which is returned.
    fn finish(self: Box<Self>) -> io::Result<PathBuf>;
    /// Gives up after a write error. Implementations keep whatever complete
    /// fragments were written and return the path of the salvaged file.
    fn abandon(self: Box<Self>) -> Option<PathBuf>;
    /// Makes what was written durable (`fsync` of the data).
    fn sync(&mut self) -> io::Result<()> {
        self.flush()
    }
    /// Makes everything durable before [`Self::finish`]: a wrapper may move
    /// the segment elsewhere when this fails.
    fn settle(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// Start of the segment that is not in this file (the segment moved to
    /// another file when its destination failed); zero normally.
    fn skipped(&self) -> Duration {
        Duration::ZERO
    }
}

/// Creates segment files.
pub trait SegmentStore: Send {
    /// Creates a new, empty segment for `camera` starting at `start`.
    fn create(&mut self, camera: &CameraId, start: SystemTime) -> io::Result<Box<dyn SegmentFile>>;
}

/// Stores segments under `root/<camera>/<YYYY-MM-DD>/<YYYYMMDDTHHMMSS.mmmZ>.mp4` (UTC).
#[derive(Clone, Debug)]
pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    /// Uses `root` as the recordings directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Recordings directory.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Directory and base file name of a segment starting at `start`.
pub fn segment_location(root: &Path, camera: &CameraId, start: SystemTime) -> (PathBuf, String) {
    let ts = jiff::Timestamp::try_from(start).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    let day = ts.strftime("%Y-%m-%d").to_string();
    let name = ts.strftime("%Y%m%dT%H%M%S.%3fZ").to_string();
    (root.join(camera.as_str()).join(day), name)
}

/// Start time (Unix milliseconds) of a finished segment from its file name
/// `YYYYMMDDTHHMMSS.mmmZ[-suffix].mp4`; `None` for any other file.
pub fn parse_segment_start_ms(name: &str) -> Option<i64> {
    let stem = name.strip_suffix(".mp4")?;
    let (ts, suffix) = stem.split_at_checked(20)?;
    let suffix_ok = suffix.is_empty()
        || suffix
            .strip_prefix('-')
            .map(|s| s.strip_prefix("recovered").unwrap_or(s))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let b = ts.as_bytes();
    if !suffix_ok || b[8] != b'T' || b[15] != b'.' || b[19] != b'Z' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = ts.get(r)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let date = jiff::civil::Date::new(
        i16::try_from(num(0..4)?).ok()?,
        i8::try_from(num(4..6)?).ok()?,
        i8::try_from(num(6..8)?).ok()?,
    )
    .ok()?;
    let time = jiff::civil::Time::new(
        i8::try_from(num(9..11)?).ok()?,
        i8::try_from(num(11..13)?).ok()?,
        i8::try_from(num(13..15)?).ok()?,
        i32::try_from(num(16..19)? * 1_000_000).ok()?,
    )
    .ok()?;
    let ts = date
        .to_datetime(time)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()?;
    Some(ts.timestamp().as_millisecond())
}

impl SegmentStore for FsStore {
    fn create(&mut self, camera: &CameraId, start: SystemTime) -> io::Result<Box<dyn SegmentFile>> {
        let (dir, name) = segment_location(&self.root, camera, start);
        fs::create_dir_all(&dir)?;
        // Never overwrite: on a name collision (same millisecond, clock set
        // back) add a counter.
        for n in 0u32..100 {
            let base = if n == 0 {
                name.clone()
            } else {
                format!("{name}-{n}")
            };
            let final_path = dir.join(format!("{base}.mp4"));
            let part_path = dir.join(format!("{base}.mp4{PART_SUFFIX}"));
            if final_path.exists() {
                continue;
            }
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&part_path)
            {
                Ok(file) => {
                    return Ok(Box::new(FsSegmentFile {
                        writer: Some(BufWriter::with_capacity(256 * 1024, file)),
                        part_path,
                        final_path,
                    }));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "too many segments with the same start time",
        ))
    }
}

struct FsSegmentFile {
    writer: Option<BufWriter<File>>,
    part_path: PathBuf,
    final_path: PathBuf,
}

impl FsSegmentFile {
    fn writer(&mut self) -> io::Result<&mut BufWriter<File>> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::other("segment already closed"))
    }
}

impl Write for FsSegmentFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer()?.write(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.writer()?.write_all(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer()?.flush()
    }
}

/// Makes a rename durable by syncing the containing directory (no-op where unsupported).
pub(crate) fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = File::open(dir)
    {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

impl SegmentFile for FsSegmentFile {
    fn path(&self) -> &Path {
        &self.part_path
    }

    fn sync(&mut self) -> io::Result<()> {
        let writer = self.writer()?;
        writer.flush()?;
        writer.get_ref().sync_data()
    }

    fn finish(mut self: Box<Self>) -> io::Result<PathBuf> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| io::Error::other("segment already closed"))?;
        let file = writer
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&self.part_path, &self.final_path)?;
        sync_dir(&self.final_path);
        Ok(self.final_path.clone())
    }

    fn abandon(mut self: Box<Self>) -> Option<PathBuf> {
        // Dropping the BufWriter attempts a final flush; whatever happens, the
        // recovery pass truncates the file to its last complete fragment.
        drop(self.writer.take());
        match recover_part_file(&self.part_path) {
            Ok(outcome) => outcome.kept_path(),
            Err(e) => {
                tracing::warn!(path = %self.part_path.display(), error = %e, "could not salvage partial segment; recovery will retry at next start");
                None
            }
        }
    }
}

impl Drop for FsSegmentFile {
    fn drop(&mut self) {
        if self.writer.is_some() {
            // Neither finished nor abandoned (e.g. panic): leave the .part
            // file for startup recovery rather than doing I/O in drop.
            tracing::debug!(path = %self.part_path.display(), "segment dropped while open");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn location_uses_utc_names() {
        let cam = CameraId::new("garage").expect("valid");
        let t = UNIX_EPOCH + Duration::from_millis(1_793_557_353_120);
        let (dir, name) = segment_location(Path::new("/rec"), &cam, t);
        assert_eq!(dir, Path::new("/rec/garage/2026-11-01"));
        assert_eq!(name, "20261101T182233.120Z");
    }
}
