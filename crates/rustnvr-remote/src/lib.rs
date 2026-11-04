//! Remote destinations: finished segments are uploaded to S3-compatible
//! object storage or to an SFTP server.
//!
//! A camera recording to a remote destination writes its segments into a
//! local *outbox* (the destination's `path`), exactly like a local disk. The
//! [`spawn_uploader`] task sends each finished segment (`*.mp4`, never the
//! `.mp4.part` being written), oldest first, to
//! `<prefix>/<camera>/<YYYY-MM-DD>/<file>`, then deletes it from the outbox.
//! The outbox is the queue: segments not yet sent survive restarts and
//! network outages (the outbox retention and free-space reserve apply).

mod s3;
mod sftp;

use rustnvr_core::Secret;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use sftp::fingerprint_file;

/// Pause between outbox scans when everything was sent.
const IDLE_SCAN: Duration = Duration::from_secs(5);
const FIRST_RETRY: Duration = Duration::from_secs(5);
const MAX_RETRY: Duration = Duration::from_secs(300);

/// S3-compatible bucket.
#[derive(Clone, Debug)]
pub struct S3Target {
    /// Endpoint URL for S3-compatible services (MinIO, Backblaze, Wasabi…);
    /// `None` for Amazon S3.
    pub endpoint: Option<String>,
    /// Region (`us-east-1` for most S3-compatible services).
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Key prefix (no leading or trailing `/`), may be empty.
    pub prefix: String,
    /// Access key identifier.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: Secret,
}

/// SFTP server.
#[derive(Clone, Debug)]
pub struct SftpTarget {
    /// Host name or address.
    pub host: String,
    /// Port (22).
    pub port: u16,
    /// User name.
    pub username: String,
    /// Password, if password authentication is used.
    pub password: Option<Secret>,
    /// Private key file (OpenSSH format, unencrypted), if key authentication is used.
    pub private_key: Option<PathBuf>,
    /// Remote directory (created if needed).
    pub remote_dir: String,
    /// Expected server key fingerprint (`SHA256:…`). Without it, the key seen
    /// on the first connection is kept and required afterwards.
    pub host_key: Option<String>,
}

/// Where to upload.
#[derive(Clone, Debug)]
pub enum RemoteTarget {
    /// S3-compatible object storage.
    S3(S3Target),
    /// SFTP server.
    Sftp(SftpTarget),
}

/// State of an uploader.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UploadStatus {
    /// Finished segments waiting in the outbox (at the last scan).
    pub pending: usize,
    /// Segments sent since start.
    pub uploaded: u64,
    /// Bytes sent since start.
    pub uploaded_bytes: u64,
    /// When the last segment was sent (ms since the Unix epoch).
    pub last_upload_ms: Option<i64>,
    /// Last failure, cleared after a successful upload.
    pub error: Option<String>,
    /// SFTP: fingerprint of the server key in use.
    pub host_key: Option<String>,
}

/// A running uploader.
pub struct UploaderHandle {
    task: JoinHandle<()>,
    cancel: CancellationToken,
    /// Current state.
    pub status: watch::Receiver<UploadStatus>,
}

impl UploaderHandle {
    /// Stops the uploader (an upload in progress is abandoned; the segment
    /// stays in the outbox and is sent again later).
    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

/// A finished segment of the outbox.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pending {
    pub path: PathBuf,
    /// `<camera>/<day>/<file>`.
    pub key: String,
    pub bytes: u64,
}

/// Finished segments of `outbox` (`<camera>/<day>/<start>.mp4`), oldest first.
/// Blocking.
pub(crate) fn pending(outbox: &Path) -> Vec<Pending> {
    let mut out = Vec::new();
    let dirs = |p: &Path| -> Vec<std::fs::DirEntry> {
        std::fs::read_dir(p).map_or_else(
            |_| Vec::new(),
            |e| {
                e.flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .collect()
            },
        )
    };
    for cam in dirs(outbox) {
        for day in dirs(&cam.path()) {
            let Ok(files) = std::fs::read_dir(day.path()) else {
                continue;
            };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".mp4") || !f.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                out.push(Pending {
                    key: format!(
                        "{}/{}/{name}",
                        cam.file_name().to_string_lossy(),
                        day.file_name().to_string_lossy()
                    ),
                    bytes: f.metadata().map_or(0, |m| m.len()),
                    path: f.path(),
                });
            }
        }
    }
    // File names start with the UTC start time: the key orders by day then time.
    out.sort_by(|a, b| {
        let tail = |k: &str| {
            k.split_once('/')
                .map_or(String::new(), |(_, t)| t.to_owned())
        };
        tail(&a.key).cmp(&tail(&b.key))
    });
    out
}

/// Removes a sent segment and its day directory once empty.
fn remove_sent(p: &Pending) {
    let _ = std::fs::remove_file(&p.path);
    if let Some(day) = p.path.parent() {
        let _ = std::fs::remove_dir(day);
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A connection able to store one file under a key.
trait Sink: Send {
    fn upload(
        &mut self,
        local: &Path,
        key: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

/// Starts uploading the finished segments of `outbox` to `target`.
pub fn spawn_uploader(
    id: String,
    target: RemoteTarget,
    outbox: PathBuf,
    cancel: CancellationToken,
) -> UploaderHandle {
    let (status_tx, status) = watch::channel(UploadStatus::default());
    let token = cancel.clone();
    let task = tokio::spawn(async move {
        match target {
            RemoteTarget::S3(t) => match s3::S3Sink::new(&t) {
                Ok(sink) => run(&id, sink, &outbox, &status_tx, &token).await,
                Err(e) => {
                    tracing::warn!(destination = %id, error = %e, "S3 destination not started");
                    status_tx.send_modify(|s| s.error = Some(e));
                    token.cancelled().await;
                }
            },
            RemoteTarget::Sftp(t) => {
                let known = outbox.join(sftp::KNOWN_HOST_FILE);
                let sink = sftp::SftpSink::new(t, known, status_tx.clone());
                run(&id, sink, &outbox, &status_tx, &token).await;
            }
        }
    });
    UploaderHandle {
        task,
        cancel,
        status,
    }
}

async fn run<S: Sink>(
    id: &str,
    mut sink: S,
    outbox: &Path,
    status: &watch::Sender<UploadStatus>,
    cancel: &CancellationToken,
) {
    let mut retry = FIRST_RETRY;
    loop {
        let dir = outbox.to_owned();
        let queue = tokio::task::spawn_blocking(move || pending(&dir))
            .await
            .unwrap_or_default();
        status.send_modify(|s| s.pending = queue.len());
        let mut failed = false;
        for p in queue {
            if cancel.is_cancelled() {
                return;
            }
            let started = std::time::Instant::now();
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                r = sink.upload(&p.path, &p.key) => r,
            };
            match result {
                Ok(()) => {
                    let path = p.clone();
                    let _ = tokio::task::spawn_blocking(move || remove_sent(&path)).await;
                    tracing::debug!(destination = %id, key = %p.key, bytes = p.bytes, ms = started.elapsed().as_millis() as u64, "segment uploaded");
                    status.send_modify(|s| {
                        s.pending = s.pending.saturating_sub(1);
                        s.uploaded += 1;
                        s.uploaded_bytes += p.bytes;
                        s.last_upload_ms = Some(now_ms());
                        s.error = None;
                    });
                    retry = FIRST_RETRY;
                }
                Err(e) => {
                    if status.borrow().error.as_deref() != Some(e.as_str()) {
                        tracing::warn!(destination = %id, key = %p.key, error = %e, retry_in_s = retry.as_secs(), "upload failed; the segment stays in the outbox");
                    }
                    status.send_modify(|s| s.error = Some(e));
                    failed = true;
                    break;
                }
            }
        }
        let wait = if failed { retry } else { IDLE_SCAN };
        if failed {
            retry = (retry * 2).min(MAX_RETRY);
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// Remote object key: `<prefix>/<key>` (without an empty prefix).
pub(crate) fn join_key(prefix: &str, key: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}/{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_finished_segments_oldest_first() {
        let dir = tempfile::tempdir().expect("tmp");
        let mk = |cam: &str, day: &str, name: &str| {
            let d = dir.path().join(cam).join(day);
            std::fs::create_dir_all(&d).expect("dir");
            std::fs::write(d.join(name), b"x").expect("write");
        };
        mk("b", "2026-11-02", "20261102T100000.000Z.mp4");
        mk("a", "2026-11-02", "20261102T090000.000Z.mp4");
        mk("a", "2026-11-02", "20261102T110000.000Z.mp4.part");
        mk("a", "2026-11-01", "20261101T235900.000Z.mp4");
        std::fs::write(dir.path().join(sftp::KNOWN_HOST_FILE), b"k").expect("write");
        let keys: Vec<String> = pending(dir.path()).into_iter().map(|p| p.key).collect();
        assert_eq!(
            keys,
            vec![
                "a/2026-11-01/20261101T235900.000Z.mp4",
                "a/2026-11-02/20261102T090000.000Z.mp4",
                "b/2026-11-02/20261102T100000.000Z.mp4",
            ]
        );
        assert_eq!(join_key("/nvr/", "a/b.mp4"), "nvr/a/b.mp4");
        assert_eq!(join_key("", "a/b.mp4"), "a/b.mp4");
    }
}
