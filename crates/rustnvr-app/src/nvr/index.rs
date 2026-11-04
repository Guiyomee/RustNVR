//! Recording index (SQLite): every finished segment is added as soon as the
//! recorder closes it; the index is reconciled with the files at startup and
//! every few minutes (segments deleted by retention or uploaded from an
//! outbox, files copied or removed by hand, interruptions). The files remain
//! the reference: the index can always be rebuilt from them.

use super::Shared;
use rustnvr_core::{NvrEvent, SegmentInfo};
use rustnvr_recorder::Destination;
use rustnvr_recorder::scan::scan;
use rustnvr_recorder::store::parse_segment_start_ms;
use rustnvr_storage::{Database, SegmentRecord};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Reconciliation interval.
const RECONCILE_EVERY: Duration = Duration::from_secs(600);

fn unix_ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Index record of a segment finished by a recorder, if it lies under one
/// of `destinations` (longest root first).
fn record_of(info: &SegmentInfo, destinations: &[Destination]) -> Option<SegmentRecord> {
    let dest = destinations
        .iter()
        .filter(|d| info.path.starts_with(&d.root))
        .max_by_key(|d| d.root.as_os_str().len())?;
    let start_ms = unix_ms(info.start);
    Some(SegmentRecord {
        destination: dest.id.clone(),
        camera: info.camera.to_string(),
        day: info.path.parent()?.file_name()?.to_str()?.to_owned(),
        file: info.path.file_name()?.to_str()?.to_owned(),
        start_ms,
        end_ms: start_ms + i64::try_from(info.duration.as_millis()).unwrap_or(0),
        bytes: info.bytes,
    })
}

/// A finished segment found on disk.
struct OnDisk {
    camera: String,
    day: String,
    file: String,
    bytes: u64,
}

/// Finished segments under `root` (`<camera>/<day>/<start>.mp4`). Blocking.
fn walk(root: &Path) -> Vec<OnDisk> {
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
    let mut out = Vec::new();
    for cam in dirs(root) {
        let camera = cam.file_name().to_string_lossy().into_owned();
        if rustnvr_core::CameraId::new(camera.as_str()).is_err() {
            continue; // e.g. the data directory `.rustnvr`
        }
        for day in dirs(&cam.path()) {
            let Ok(files) = std::fs::read_dir(day.path()) else {
                continue;
            };
            for f in files.flatten() {
                let file = f.file_name().to_string_lossy().into_owned();
                if parse_segment_start_ms(&file).is_none() {
                    continue;
                }
                let Ok(meta) = f.metadata() else { continue };
                if meta.is_file() {
                    out.push(OnDisk {
                        camera: camera.clone(),
                        day: day.file_name().to_string_lossy().into_owned(),
                        file,
                        bytes: meta.len(),
                    });
                }
            }
        }
    }
    out
}

/// Duration of a segment in milliseconds, read from its fragments. Blocking.
fn duration_ms(path: &Path) -> Option<i64> {
    let report = scan(&mut std::fs::File::open(path).ok()?).ok()?;
    let timescale = u64::from(report.timescale?.max(1));
    i64::try_from(report.duration() * 1000 / timescale).ok()
}

/// Brings the index of one destination in line with its files.
async fn reconcile_destination(db: &Database, dest: &Destination) -> (u64, u64) {
    let root = dest.root.clone();
    let Ok(files) = tokio::task::spawn_blocking(move || walk(&root)).await else {
        return (0, 0);
    };
    let indexed = match db.destination_segments(&dest.id).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(destination = %dest.id, error = %e, "index: cannot read");
            return (0, 0);
        }
    };
    let on_disk: HashSet<(String, String)> = files
        .iter()
        .map(|f| (f.camera.clone(), f.file.clone()))
        .collect();
    let known: HashSet<(String, String)> = indexed
        .iter()
        .map(|r| (r.camera.clone(), r.file.clone()))
        .collect();
    let mut removed = 0;
    for r in indexed
        .iter()
        .filter(|r| !on_disk.contains(&(r.camera.clone(), r.file.clone())))
    {
        if db
            .delete_segment(&dest.id, &r.camera, &r.file)
            .await
            .is_ok()
        {
            removed += 1;
        }
    }
    let mut added = 0;
    for f in files
        .into_iter()
        .filter(|f| !known.contains(&(f.camera.clone(), f.file.clone())))
    {
        let path = dest.root.join(&f.camera).join(&f.day).join(&f.file);
        let Some(start_ms) = parse_segment_start_ms(&f.file) else {
            continue;
        };
        let Ok(Some(ms)) = tokio::task::spawn_blocking(move || duration_ms(&path)).await else {
            continue; // unreadable or empty segment: not playable anyway
        };
        let record = SegmentRecord {
            destination: dest.id.clone(),
            camera: f.camera,
            day: f.day,
            file: f.file,
            start_ms,
            end_ms: start_ms + ms,
            bytes: f.bytes,
        };
        if db.upsert_segment(&record).await.is_ok() {
            added += 1;
        }
    }
    (added, removed)
}

/// Reconciles every available destination with the index.
pub(super) async fn reconcile(shared: &Arc<Shared>, db: &Database) {
    let started = Instant::now();
    let destinations: Vec<Destination> = shared
        .state
        .lock()
        .await
        .storage
        .all()
        .into_iter()
        .map(|(d, _)| d)
        .collect();
    let ids: Vec<String> = destinations.iter().map(|d| d.id.clone()).collect();
    if let Err(e) = db.retain_destinations(&ids).await {
        warn!(error = %e, "index: cannot forget removed destinations");
    }
    let (mut added, mut removed) = (0, 0);
    for dest in destinations.iter().filter(|d| d.is_available()) {
        let (a, r) = reconcile_destination(db, dest).await;
        added += a;
        removed += r;
    }
    if added > 0 || removed > 0 {
        info!(
            added,
            removed,
            ms = started.elapsed().as_millis() as u64,
            "recording index updated from the files"
        );
    }
}

/// Indexes finished segments as they come, and reconciles regularly.
pub(super) async fn run_indexer(shared: Arc<Shared>, db: Database, cancel: CancellationToken) {
    let mut events = shared.engine.events.subscribe();
    reconcile(&shared, &db).await;
    let mut ticker = tokio::time::interval(RECONCILE_EVERY);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = ticker.tick() => reconcile(&shared, &db).await,
            event = events.recv() => match event {
                Ok(NvrEvent::SegmentFinished(info)) => {
                    let destinations: Vec<Destination> = shared
                        .state
                        .lock()
                        .await
                        .storage
                        .all()
                        .into_iter()
                        .map(|(d, _)| d)
                        .collect();
                    if let Some(record) = record_of(&info, &destinations)
                        && let Err(e) = db.upsert_segment(&record).await
                    {
                        warn!(error = %e, "index: cannot add segment");
                    }
                }
                Ok(_) => {}
                // Missed segments are found by the next reconciliation.
                Err(RecvError::Lagged(_)) => ticker.reset_immediately(),
                Err(RecvError::Closed) => return,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::{CameraId, VideoCodec};

    #[test]
    fn maps_finished_segments_to_their_destination() {
        let dests = vec![
            Destination {
                id: "local".into(),
                name: String::new(),
                root: "/rec".into(),
                require_mount: false,
            },
            Destination {
                id: "nas".into(),
                name: String::new(),
                root: "/rec/nas".into(),
                require_mount: true,
            },
        ];
        let info = SegmentInfo {
            camera: CameraId::new("a").expect("id"),
            path: "/rec/nas/a/2026-11-02/20261102T100000.000Z.mp4".into(),
            start: UNIX_EPOCH + Duration::from_secs(1_793_613_600),
            duration: Duration::from_millis(59_500),
            bytes: 42,
            frames: 1,
            fragments: 1,
            codec: VideoCodec::H264,
            width: 1,
            height: 1,
        };
        let r = record_of(&info, &dests).expect("record");
        assert_eq!(
            (r.destination.as_str(), r.day.as_str()),
            ("nas", "2026-11-02")
        );
        assert_eq!(r.end_ms - r.start_ms, 59_500);
    }
}
