//! A segment written to a destination that may fail in the middle (a
//! network share): what was written since the last `fsync` is kept in
//! memory, and the file is synced every few seconds. When a write, a sync or
//! the closing fails, the segment goes on in a new file on the fallback (the
//! local disk) that starts with that copy, so the recorder does not even
//! notice. The part already on the destination is recovered when it comes
//! back (`recover_stale_tree`); the new file starts where it ends
//! ([`SegmentFile::skipped`]).

use crate::scan::scan;
use crate::store::{FsStore, SegmentFile, SegmentStore};
use rustnvr_core::CameraId;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often the file on the destination is synced (the most that can be
/// lost when it fails, and kept in memory meanwhile).
const SYNC_EVERY: Duration = Duration::from_secs(5);
/// Most bytes kept in memory: beyond (a very high bit rate, a sync that
/// keeps failing silently) nothing is kept until the next sync.
const MAX_KEPT: usize = 16 * 1024 * 1024;

/// What to do when the segment moves to the fallback (switch the flag, log).
pub(crate) type OnRescue = Box<dyn Fn(&io::Error) + Send>;

/// See the module documentation.
pub(crate) struct RescuedFile {
    inner: Box<dyn SegmentFile>,
    /// Where to go on; `None` once used.
    fallback: Option<FsStore>,
    camera: CameraId,
    start: SystemTime,
    /// `ftyp` + `moov`, once known (after the first sync).
    init: Option<Vec<u8>>,
    /// Bytes written since the last sync (the init too, before the first).
    kept: Vec<u8>,
    keeping: bool,
    last_sync: Instant,
    /// The segment moved to the fallback: start of the segment not in it.
    skipped: Option<Duration>,
    on_rescue: OnRescue,
}

impl RescuedFile {
    pub(crate) fn new(
        inner: Box<dyn SegmentFile>,
        fallback: FsStore,
        camera: &CameraId,
        start: SystemTime,
        on_rescue: OnRescue,
    ) -> Self {
        Self {
            inner,
            fallback: Some(fallback),
            camera: camera.clone(),
            start,
            init: None,
            kept: Vec::new(),
            keeping: true,
            last_sync: Instant::now(),
            skipped: None,
            on_rescue,
        }
    }

    fn keep(&mut self, buf: &[u8]) {
        if !self.keeping {
            return;
        }
        if self.kept.len() + buf.len() > MAX_KEPT {
            self.keeping = false;
            self.kept = Vec::new();
            tracing::debug!(camera = %self.camera, "segment copy too large: not kept until the next sync");
            return;
        }
        self.kept.extend_from_slice(buf);
    }

    /// Everything written since the last sync is on the destination.
    fn synced(&mut self) {
        if self.init.is_none() {
            let init_len = scan(&mut io::Cursor::new(&self.kept))
                .ok()
                .and_then(|r| usize::try_from(r.init_len).ok())
                .filter(|l| *l > 0 && *l <= self.kept.len());
            match init_len {
                Some(len) => self.init = Some(self.kept[..len].to_vec()),
                // Without the init the copy is useless: no rescue.
                None => self.fallback = None,
            }
        }
        self.kept.clear();
        self.keeping = self.init.is_some();
        self.last_sync = Instant::now();
    }

    /// Syncs the destination file now, or moves to the fallback.
    fn sync_or_rescue(&mut self) -> io::Result<()> {
        match self.inner.sync() {
            Ok(()) => {
                self.synced();
                Ok(())
            }
            Err(e) => self.rescue(e),
        }
    }

    /// Goes on in a new file on the fallback, starting with the copy; the
    /// original error when that is not possible.
    fn rescue(&mut self, error: io::Error) -> io::Result<()> {
        if !self.keeping {
            return Err(error);
        }
        let Some(mut fallback) = self.fallback.take() else {
            return Err(error);
        };
        let mut content = self.init.clone().unwrap_or_default();
        content.extend_from_slice(&self.kept);
        let Some(skipped) = first_decode_time(&content) else {
            return Err(error);
        };
        let moved = fallback
            .create(&self.camera, self.start + skipped)
            .and_then(|mut file| file.write_all(&content).map(|()| file));
        let file = match moved {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!(camera = %self.camera, error = %e, "segment could not be moved to the local disk");
                return Err(error);
            }
        };
        let old = std::mem::replace(&mut self.inner, file);
        // Off the recorder: a lost share may take long to give up.
        std::thread::spawn(move || {
            old.abandon();
        });
        tracing::warn!(camera = %self.camera, error = %error, kept_bytes = self.kept.len(), skipped_ms = skipped.as_millis() as u64, "destination failed in the middle of a segment: it goes on on the local disk");
        (self.on_rescue)(&error);
        self.kept = Vec::new();
        self.keeping = false;
        self.skipped = Some(skipped);
        Ok(())
    }
}

/// Time of the first fragment of an fMP4 file (its init, then fragments,
/// the last one possibly incomplete), from the segment start: its `tfdt`
/// in the timescale of the init. Zero without any fragment yet.
fn first_decode_time(content: &[u8]) -> Option<Duration> {
    let report = scan(&mut io::Cursor::new(content)).ok()?;
    let init_len = usize::try_from(report.init_len).ok().filter(|l| *l > 0)?;
    let timescale = u64::from(report.timescale.filter(|t| *t > 0)?);
    let rest = content.get(init_len..)?;
    let Some(at) = rest.windows(4).position(|w| w == b"tfdt") else {
        return Some(Duration::ZERO);
    };
    let body = rest.get(at + 4..)?;
    let ticks = match body.first()? {
        1 => u64::from_be_bytes(body.get(4..12)?.try_into().ok()?),
        _ => u64::from(u32::from_be_bytes(body.get(4..8)?.try_into().ok()?)),
    };
    Some(Duration::from_nanos(
        u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(timescale)).ok()?,
    ))
}

impl Write for RescuedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_all(buf).map(|()| buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.skipped.is_some() {
            return self.inner.write_all(buf);
        }
        let result = self.inner.write_all(buf);
        self.keep(buf);
        match result {
            Ok(()) => Ok(()),
            Err(e) => self.rescue(e),
        }
    }

    /// Called after each complete fragment: the copy then starts at a
    /// fragment.
    fn flush(&mut self) -> io::Result<()> {
        if self.skipped.is_some() {
            return self.inner.flush();
        }
        if let Err(e) = self.inner.flush() {
            return self.rescue(e);
        }
        if self.last_sync.elapsed() >= SYNC_EVERY {
            return self.sync_or_rescue();
        }
        Ok(())
    }
}

impl SegmentFile for RescuedFile {
    fn path(&self) -> &Path {
        self.inner.path()
    }

    fn sync(&mut self) -> io::Result<()> {
        if self.skipped.is_some() {
            return self.inner.sync();
        }
        self.sync_or_rescue()
    }

    fn settle(&mut self) -> io::Result<()> {
        if self.skipped.is_some() {
            return self.inner.settle();
        }
        if let Err(e) = self.inner.flush() {
            return self.rescue(e);
        }
        self.sync_or_rescue()
    }

    fn skipped(&self) -> Duration {
        self.skipped.unwrap_or_default()
    }

    fn finish(self: Box<Self>) -> io::Result<PathBuf> {
        self.inner.finish()
    }

    fn abandon(self: Box<Self>) -> Option<PathBuf> {
        self.inner.abandon()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmp4::{self, SampleMeta};
    use rustnvr_core::{VideoCodec, VideoParams};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// A destination file that fails once `fail` is set.
    struct Breaking {
        fail: Arc<AtomicBool>,
        written: Arc<Mutex<Vec<u8>>>,
        path: PathBuf,
    }

    impl Write for Breaking {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.fail.load(Ordering::Relaxed) {
                return Err(io::Error::other("share gone"));
            }
            self.written.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SegmentFile for Breaking {
        fn path(&self) -> &Path {
            &self.path
        }
        fn finish(self: Box<Self>) -> io::Result<PathBuf> {
            Ok(self.path.clone())
        }
        fn abandon(self: Box<Self>) -> Option<PathBuf> {
            None
        }
    }

    fn params() -> Arc<VideoParams> {
        let mut entry = vec![0, 0, 0, 16];
        entry.extend_from_slice(b"avc1");
        entry.extend_from_slice(&[0; 8]);
        Arc::new(VideoParams {
            codec: VideoCodec::H264,
            rfc6381_codec: "avc1.42C01E".into(),
            width: 64,
            height: 48,
            clock_rate: 90_000,
            sample_entry: bytes::Bytes::from(entry),
        })
    }

    /// One fragment of one key frame of 1 s at `decode_time`.
    fn fragment(sequence: u32, decode_time: u64) -> Vec<u8> {
        let mut f = fmp4::fragment_header(
            sequence,
            decode_time,
            &[SampleMeta {
                duration: 90_000,
                size: 4,
                is_key: true,
            }],
        );
        f.extend_from_slice(&[0x65; 4]);
        f
    }

    #[test]
    fn a_failing_destination_goes_on_on_the_fallback() {
        let dir = tempfile::tempdir().expect("tmp");
        let fail = Arc::new(AtomicBool::new(false));
        let written = Arc::new(Mutex::new(Vec::new()));
        let rescued = Arc::new(AtomicBool::new(false));
        let flag = rescued.clone();
        let camera = CameraId::new("jardin").expect("id");
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_600_000);
        let mut file = RescuedFile::new(
            Box::new(Breaking {
                fail: fail.clone(),
                written: written.clone(),
                path: PathBuf::from("/nas/x.mp4.part"),
            }),
            FsStore::new(dir.path()),
            &camera,
            start,
            Box::new(move |_| flag.store(true, Ordering::Relaxed)),
        );
        file.write_all(&fmp4::init_segment_av(&params(), None))
            .expect("init");
        file.write_all(&fragment(1, 0)).expect("fragment 1");
        file.flush().expect("flush");
        // Synced: the copy now starts after the first fragment.
        file.sync().expect("sync");
        file.write_all(&fragment(2, 90_000)).expect("fragment 2");
        file.flush().expect("flush");
        // The share goes in the middle of the third fragment.
        let third = fragment(3, 180_000);
        let (head, tail) = third.split_at(10);
        file.write_all(head).expect("start of fragment 3");
        fail.store(true, Ordering::Relaxed);
        file.write_all(tail).expect("moved to the fallback");
        file.settle().expect("settled on the fallback");
        assert!(rescued.load(Ordering::Relaxed));
        assert_eq!(file.skipped(), Duration::from_secs(1));
        let path = Box::new(file).finish().expect("finish");
        assert!(path.starts_with(dir.path()), "{}", path.display());
        // Named after its own start, one second into the segment.
        assert!(
            path.to_string_lossy().ends_with("20261102T061321.000Z.mp4"),
            "{}",
            path.display()
        );
        let report = scan(&mut std::fs::File::open(&path).expect("open")).expect("scan");
        assert_eq!(report.fragments.len(), 2, "fragments 2 and 3");
        assert_eq!(report.fragments[0].base_decode_time, 90_000);
        assert_eq!(report.valid_len, report.file_len);
    }

    #[test]
    fn without_a_failure_nothing_moves() {
        let dir = tempfile::tempdir().expect("tmp");
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut file = RescuedFile::new(
            Box::new(Breaking {
                fail: Arc::new(AtomicBool::new(false)),
                written: written.clone(),
                path: PathBuf::from("/nas/x.mp4.part"),
            }),
            FsStore::new(dir.path()),
            &CameraId::new("jardin").expect("id"),
            SystemTime::now(),
            Box::new(|_| panic!("no rescue")),
        );
        file.write_all(&fmp4::init_segment_av(&params(), None))
            .expect("init");
        file.write_all(&fragment(1, 0)).expect("fragment");
        file.settle().expect("settle");
        assert_eq!(file.skipped(), Duration::ZERO);
        assert_eq!(
            Box::new(file).finish().expect("finish"),
            PathBuf::from("/nas/x.mp4.part")
        );
        assert!(std::fs::read_dir(dir.path()).expect("dir").next().is_none());
        assert!(!written.lock().expect("lock").is_empty());
    }
}
