//! Recording destinations: the local disk and directories on mounted network
//! shares (NAS over SMB/CIFS or NFS).
//!
//! A network share is written directly, like a local disk. The danger is the
//! unmounted share: its mount point is then an ordinary (empty) directory of
//! the system disk, and writing there would silently fill the system disk.
//! A destination with `require_mount` is therefore only used while its
//! directory exists *and* lies on another filesystem than the system root.
//!
//! Hung network mounts can block writes indefinitely: mount shares with
//! "soft" semantics (the CIFS default; `soft` for NFS) so that a lost NAS
//! returns I/O errors, which the recorder handles.

use crate::store::{FsStore, SegmentFile, SegmentStore};
use rustnvr_core::CameraId;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// File written then deleted at the root of a destination to check that it
/// accepts writes.
const WRITE_PROBE: &str = ".rustnvr-write-test";

/// A place where segments can be written.
#[derive(Clone, Debug)]
pub struct Destination {
    /// Identifier (`local` for the local recordings directory).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Root directory.
    pub root: PathBuf,
    /// Only use `root` when it is on a mounted filesystem other than `/`.
    pub require_mount: bool,
}

#[cfg(unix)]
// `st_dev` is not `u64` on every Unix platform.
#[allow(clippy::unnecessary_cast)]
fn device_of(path: &Path) -> io::Result<u64> {
    rustix::fs::stat(path)
        .map(|s| s.st_dev as u64)
        .map_err(io::Error::from)
}

impl Destination {
    /// Why the destination cannot be used right now, if it cannot.
    pub fn unavailable_reason(&self) -> Option<String> {
        match std::fs::metadata(&self.root) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Some(format!("{} n'est pas un dossier", self.root.display())),
            Err(e) => return Some(format!("{} inaccessible : {e}", self.root.display())),
        }
        #[cfg(unix)]
        if self.require_mount {
            match (device_of(&self.root), device_of(Path::new("/"))) {
                (Ok(dest), Ok(system)) if dest == system => {
                    return Some(format!(
                        "{} est sur le disque système : le partage réseau n'est pas monté",
                        self.root.display()
                    ));
                }
                (Err(e), _) | (_, Err(e)) => return Some(format!("{} : {e}", self.root.display())),
                _ => {}
            }
        }
        None
    }

    /// Whether the destination can be written now.
    pub fn is_available(&self) -> bool {
        self.unavailable_reason().is_none()
    }

    /// Like [`Self::unavailable_reason`], also writing (then deleting) a
    /// small file: a share mounted read-only or refusing writes is found
    /// before a recording fails. Blocking; may hang on a hung network share.
    pub fn problem(&self) -> Option<String> {
        if let Some(reason) = self.unavailable_reason() {
            return Some(reason);
        }
        let probe = self.root.join(WRITE_PROBE);
        match std::fs::write(&probe, b"rustnvr") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                None
            }
            Err(e) => Some(format!(
                "{} : écriture impossible ({e})",
                self.root.display()
            )),
        }
    }
}

/// Segment store writing to a destination, falling back to another one
/// (the local disk) while the destination is unavailable.
///
/// Availability is checked when each segment starts. A failure in the middle
/// of a segment is handled by the recorder (the segment is closed, the next
/// one starts after the retry delay, on the fallback if needed).
pub struct DestinationStore {
    camera_label: String,
    /// The destination's name, for the log of a segment moved off it.
    primary_label: String,
    primary: Destination,
    primary_store: FsStore,
    fallback: Option<FsStore>,
    using_fallback: Arc<AtomicBool>,
}

impl DestinationStore {
    /// Writes to `primary`, or to `fallback` while `primary` is unavailable.
    pub fn new(camera: &CameraId, primary: Destination, fallback: Option<PathBuf>) -> Self {
        Self {
            camera_label: camera.to_string(),
            primary_label: primary.id.clone(),
            primary_store: FsStore::new(&primary.root),
            primary,
            fallback: fallback.map(FsStore::new),
            using_fallback: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Shared flag telling whether segments currently go to the fallback.
    pub fn fallback_flag(&self) -> Arc<AtomicBool> {
        self.using_fallback.clone()
    }

    fn switch(&self, to_fallback: bool, reason: &str) {
        if self.using_fallback.swap(to_fallback, Ordering::Relaxed) != to_fallback {
            if to_fallback {
                tracing::warn!(camera = %self.camera_label, destination = %self.primary.id, %reason, "destination unavailable, recording to the local disk meanwhile");
            } else {
                tracing::info!(camera = %self.camera_label, destination = %self.primary.id, "destination available again");
            }
        }
    }
}

impl SegmentStore for DestinationStore {
    fn create(&mut self, camera: &CameraId, start: SystemTime) -> io::Result<Box<dyn SegmentFile>> {
        let failure = match self.primary.unavailable_reason() {
            None => match self.primary_store.create(camera, start) {
                Ok(f) => {
                    self.switch(false, "");
                    // With a fallback, a failure in the middle of the
                    // segment moves it there instead of losing its end.
                    return Ok(match &self.fallback {
                        Some(fallback) => {
                            let flag = self.using_fallback.clone();
                            let (camera_label, destination) =
                                (self.camera_label.clone(), self.primary_label.clone());
                            Box::new(crate::rescue::RescuedFile::new(
                                f,
                                fallback.clone(),
                                camera,
                                start,
                                Box::new(move |error| {
                                    if !flag.swap(true, Ordering::Relaxed) {
                                        tracing::warn!(camera = %camera_label, %destination, %error, "destination unavailable, recording to the local disk meanwhile");
                                    }
                                }),
                            ))
                        }
                        None => f,
                    });
                }
                Err(e) => e,
            },
            Some(reason) => io::Error::new(io::ErrorKind::NotFound, reason),
        };
        if self.fallback.is_some() {
            self.switch(true, &failure.to_string());
        }
        match &mut self.fallback {
            Some(fallback) => fallback.create(camera, start),
            None => Err(io::Error::new(
                failure.kind(),
                format!("destination {:?} : {failure}", self.primary.id),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(root: &Path, require_mount: bool) -> Destination {
        Destination {
            id: "nas".into(),
            name: "NAS".into(),
            root: root.to_owned(),
            require_mount,
        }
    }

    #[test]
    fn missing_directory_is_unavailable() {
        let d = dest(Path::new("/nonexistent/rustnvr-nas"), false);
        assert!(
            d.unavailable_reason()
                .is_some_and(|r| r.contains("inaccessible"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_on_the_system_disk_is_not_a_mounted_share() {
        // The root directory itself is, by definition, on the system disk.
        let d = dest(Path::new("/"), true);
        assert!(
            d.unavailable_reason()
                .is_some_and(|r| r.contains("pas monté")),
            "{:?}",
            d.unavailable_reason()
        );
        assert!(dest(Path::new("/"), false).is_available());
    }

    #[test]
    fn problem_checks_writes_and_leaves_nothing() {
        let dir = tempfile::tempdir().expect("tmp");
        assert_eq!(dest(dir.path(), false).problem(), None);
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 0);
        let missing = dest(&dir.path().join("gone"), false);
        assert!(
            missing
                .problem()
                .is_some_and(|r| r.contains("inaccessible"))
        );
        // A file where the directory should be: not writable as a directory.
        let file = dir.path().join("file");
        std::fs::write(&file, b"").expect("file");
        assert!(dest(&file, false).problem().is_some());
    }

    #[test]
    fn falls_back_to_local_and_returns_when_available() {
        let nas = tempfile::tempdir().expect("tmp");
        let local = tempfile::tempdir().expect("tmp");
        let missing = nas.path().join("share");
        let cam = CameraId::new("cam").expect("id");
        let mut store =
            DestinationStore::new(&cam, dest(&missing, false), Some(local.path().to_owned()));
        let flag = store.fallback_flag();

        let f = store.create(&cam, SystemTime::now()).expect("fallback");
        assert!(f.path().starts_with(local.path()));
        assert!(flag.load(Ordering::Relaxed));
        f.abandon();

        std::fs::create_dir(&missing).expect("share appears");
        let f = store.create(&cam, SystemTime::now()).expect("primary");
        assert!(f.path().starts_with(&missing));
        assert!(!flag.load(Ordering::Relaxed));
        f.abandon();

        // Without fallback the error names the destination.
        let mut alone = DestinationStore::new(&cam, dest(&nas.path().join("gone"), false), None);
        let err = alone.create(&cam, SystemTime::now()).err().expect("error");
        assert!(err.to_string().contains("\"nas\""), "{err}");
    }
}
