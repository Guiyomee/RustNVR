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
}

/// Segment store writing to a destination, falling back to another one
/// (the local disk) while the destination is unavailable.
///
/// Availability is checked when each segment starts. A failure in the middle
/// of a segment is handled by the recorder (the segment is closed, the next
/// one starts after the retry delay, on the fallback if needed).
pub struct DestinationStore {
    camera_label: String,
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
                    return Ok(f);
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
