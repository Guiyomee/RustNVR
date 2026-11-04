//! Disk and recordings usage, per destination.

use crate::dto::{CameraStorageDto, DayStorageDto, DestinationDto, DiskDto, StorageDto};
use crate::recordings::{parse_segment_name, valid_day};
use crate::state::{ApiState, CameraSource, DestinationInfo, unix_ms};
use rustnvr_core::config::RemoteConfig;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

const DAY_MS: f64 = 24.0 * 3600.0 * 1000.0;

#[cfg(unix)]
/// Size, usage and free space of the filesystem holding `path`. Blocking.
pub fn disk_usage(path: &Path) -> Option<DiskDto> {
    let s = rustix::fs::statvfs(path).ok()?;
    let block = s.f_frsize;
    let total = s.f_blocks.saturating_mul(block);
    let free = s.f_bavail.saturating_mul(block);
    Some(DiskDto {
        total_bytes: total,
        used_bytes: total.saturating_sub(s.f_bfree.saturating_mul(block)),
        free_bytes: free,
    })
}

#[cfg(not(unix))]
/// Not available on this platform.
pub fn disk_usage(_: &Path) -> Option<DiskDto> {
    None
}

/// Whether a filesystem is short of space: usage above `percent`, or free
/// space below `min_free_gb`.
pub fn is_low_space(disk: &DiskDto, percent: u8, min_free_gb: Option<u32>) -> bool {
    let used = disk.used_bytes as f64 * 100.0 / disk.total_bytes.max(1) as f64;
    used >= f64::from(percent)
        || min_free_gb.is_some_and(|g| disk.free_bytes < u64::from(g) * 1_000_000_000)
}

/// What was found walking the recordings of a destination.
#[derive(Default)]
struct Walked {
    bytes: u64,
    /// Start of the oldest and of the newest segment.
    oldest_ms: Option<i64>,
    newest_ms: Option<i64>,
}

impl Walked {
    fn add(&mut self, start_ms: i64, bytes: u64) {
        self.bytes += bytes;
        self.oldest_ms = Some(self.oldest_ms.map_or(start_ms, |o| o.min(start_ms)));
        self.newest_ms = Some(self.newest_ms.map_or(start_ms, |n| n.max(start_ms)));
    }

    /// Average bytes written per day, over at least one hour of recordings.
    fn rate_per_day(&self) -> Option<f64> {
        match (self.oldest_ms, self.newest_ms) {
            (Some(o), Some(n)) if n - o >= 3_600_000 && self.bytes > 0 => {
                Some(self.bytes as f64 / ((n - o) as f64 / DAY_MS))
            }
            _ => None,
        }
    }
}

/// Days until `disk` is full at `rate` bytes per day.
fn days_until_full(disk: Option<&DiskDto>, rate: Option<f64>) -> Option<f64> {
    match (disk, rate) {
        (Some(d), Some(rate)) if rate > 0.0 => Some(d.free_bytes as f64 / rate),
        _ => None,
    }
}

/// State of a destination. Blocking (filesystem calls): call from a blocking thread.
/// `walked`: its recordings, when they were walked (`GET /storage`).
fn destination_dto(
    d: &DestinationInfo,
    cameras: &[CameraSource],
    walked: Option<&Walked>,
    low_space_percent: u8,
) -> DestinationDto {
    let reason = d.destination().unavailable_reason();
    let disk = if reason.is_none() {
        disk_usage(&d.root)
    } else {
        None
    };
    DestinationDto {
        id: d.id.clone(),
        name: d.name.clone(),
        kind: match (&d.remote, d.is_local()) {
            (Some(r), _) => r.kind(),
            (None, true) => "local",
            (None, false) => "nas",
        },
        path: d.root.display().to_string(),
        managed: d.managed || d.is_local(),
        require_mount: d.require_mount,
        available: reason.is_none(),
        disk: disk.clone(),
        unavailable_reason: reason,
        retention_days: d.retention.retention_days,
        max_size_gb: d.retention.max_size_gb,
        min_free_gb: d.retention.min_free_gb,
        low_space: disk
            .as_ref()
            .is_some_and(|k| is_low_space(k, low_space_percent, d.retention.min_free_gb)),
        remote: d.remote.as_ref().map(remote_json),
        upload: d.upload.as_ref().map(|u| {
            let s = u.borrow();
            serde_json::json!({
                "pending": s.pending,
                "uploaded": s.uploaded,
                "uploaded_bytes": s.uploaded_bytes,
                "last_upload_ms": s.last_upload_ms,
                "error": s.error,
                "host_key": s.host_key,
            })
        }),
        used_by: cameras
            .iter()
            .filter(|c| c.record_destinations.contains(&d.id))
            .map(|c| c.id.to_string())
            .collect(),
        recordings_bytes: walked.map(|w| w.bytes),
        // A remote destination's disk is only its local outbox.
        estimated_days_until_full: walked
            .filter(|_| d.remote.is_none())
            .and_then(|w| days_until_full(disk.as_ref(), w.rate_per_day())),
    }
}

/// Every destination with its state, without walking recordings.
pub(crate) fn destinations(state: &ApiState) -> Vec<DestinationDto> {
    let cameras = state.cameras();
    state
        .destinations()
        .iter()
        .map(|d| destination_dto(d, &cameras, None, state.low_space_percent()))
        .collect()
}

/// Walks the recordings of every available destination. Blocking.
pub(crate) fn compute(state: &ApiState) -> StorageDto {
    let cams = state.cameras();
    let dests = state.destinations();
    let mut days: BTreeMap<String, u64> = BTreeMap::new();
    let mut cameras: Vec<CameraStorageDto> = cams
        .iter()
        .map(|c| CameraStorageDto {
            camera: c.id.to_string(),
            bytes: 0,
            segments: 0,
            oldest_ms: None,
            newest_ms: None,
        })
        .collect();
    let mut destination_dtos = Vec::new();
    let mut local_rate = None;
    for d in &dests {
        let mut walked = Walked::default();
        if d.destination().is_available() {
            for c in cameras.iter_mut() {
                let Ok(day_dirs) = std::fs::read_dir(d.root.join(&c.camera)) else {
                    continue;
                };
                for day in day_dirs.flatten() {
                    let day_name = day.file_name().to_string_lossy().into_owned();
                    if !valid_day(&day_name) {
                        continue;
                    }
                    let Ok(files) = std::fs::read_dir(day.path()) else {
                        continue;
                    };
                    for f in files.flatten() {
                        let name = f.file_name();
                        let Some(start) = name.to_str().and_then(parse_segment_name) else {
                            continue;
                        };
                        let Ok(meta) = f.metadata() else { continue };
                        c.bytes += meta.len();
                        c.segments += 1;
                        c.oldest_ms = Some(c.oldest_ms.map_or(start, |o: i64| o.min(start)));
                        c.newest_ms = Some(c.newest_ms.map_or(start, |n: i64| n.max(start)));
                        *days.entry(day_name.clone()).or_default() += meta.len();
                        walked.add(start, meta.len());
                    }
                }
            }
        }
        if d.is_local() {
            local_rate = walked.rate_per_day();
        }
        destination_dtos.push(destination_dto(
            d,
            &cams,
            Some(&walked),
            state.low_space_percent(),
        ));
    }
    let recordings_bytes: u64 = cameras.iter().map(|c| c.bytes).sum();
    let disk = disk_usage(&state.recordings_dir);

    // Fill rate of the local disk, from its own recordings.
    let fill_rate = local_rate;
    let estimated_days_until_full = days_until_full(disk.as_ref(), fill_rate);
    let retention_configured = dests
        .iter()
        .any(|d| d.retention.retention_days.is_some() || d.retention.max_size_gb.is_some());
    StorageDto {
        recordings_dir: state.recordings_dir.display().to_string(),
        destinations: destination_dtos,
        default_destination: state.default_destination(),
        disk,
        recordings_bytes,
        cameras,
        days: days
            .into_iter()
            .map(|(day, bytes)| DayStorageDto { day, bytes })
            .collect(),
        fill_rate_bytes_per_day: fill_rate,
        estimated_days_until_full,
        retention_configured,
        computed_at_ms: unix_ms(SystemTime::now()),
    }
}

/// Public settings of a remote destination: never a secret, only whether
/// one is set.
fn remote_json(r: &RemoteConfig) -> serde_json::Value {
    match r {
        RemoteConfig::S3(s) => serde_json::json!({
            "type": "s3",
            "endpoint": s.endpoint,
            "region": s.region,
            "bucket": s.bucket,
            "prefix": s.prefix,
            "access_key_id": s.access_key_id,
            "has_secret": s.secret_access_key.is_some() || s.secret_access_key_env.is_some(),
        }),
        RemoteConfig::Sftp(s) => serde_json::json!({
            "type": "sftp",
            "host": s.host,
            "port": s.port,
            "username": s.username,
            "remote_dir": s.remote_dir,
            "host_key": s.host_key,
            "private_key": s.private_key.as_ref().map(|p| p.display().to_string()),
            "has_password": s.password.is_some() || s.password_env.is_some(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_days_until_full_from_its_own_recordings() {
        let mut w = Walked::default();
        w.add(0, 500);
        // Less than one hour of recordings: no estimate.
        w.add(1_800_000, 500);
        assert_eq!(w.rate_per_day(), None);
        // 2 GB written over half a day: 4 GB per day.
        let mut w = Walked::default();
        w.add(0, 1_000_000_000);
        w.add(43_200_000, 1_000_000_000);
        assert_eq!(w.rate_per_day(), Some(4e9));
        let disk = DiskDto {
            total_bytes: 100_000_000_000,
            used_bytes: 60_000_000_000,
            free_bytes: 40_000_000_000,
        };
        assert_eq!(days_until_full(Some(&disk), w.rate_per_day()), Some(10.0));
        assert_eq!(days_until_full(None, w.rate_per_day()), None);
        assert_eq!(days_until_full(Some(&disk), None), None);
    }
}
