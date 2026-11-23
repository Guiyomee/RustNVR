//! Automatic backups of the configuration: the backup of Paramètres →
//! Sauvegarde ([`super::backups::make`]), made every few days into a folder of
//! a storage destination (a NAS share, so that it survives the loss of the
//! server's disk), the oldest ones deleted.
//!
//! The settings, passphrase included, are in [`MANAGED_BACKUP_FILE`] next to
//! the configuration file (owner only); they are part of the backups, so a
//! restored server goes on making them.

use super::Shared;
use super::backups;
use crate::backup::{MIN_PASSPHRASE, write_private_bytes};
use rustnvr_api::{AutoBackupFile, AutoBackupInput, AutoBackupRun, AutoBackupView, ControlError};
use rustnvr_core::config::{AutoBackupConfig, MANAGED_BACKUP_FILE};
use rustnvr_recorder::Destination;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Folder of the backups, at the root of the destination (a dot folder:
/// never taken for a camera by the scans of the recordings).
pub(super) const FOLDER: &str = ".rustnvr-backups";
const PREFIX: &str = "rustnvr-config-";
const SUFFIX: &str = ".rnvrbak";
/// The first check, after the start…
const FIRST_CHECK: Duration = Duration::from_secs(120);
/// … then every…
const CHECK: Duration = Duration::from_secs(3600);
/// Longest wait for a share (a hung NAS must not block the checks).
const SHARE_TIMEOUT: Duration = Duration::from_secs(60);
const DAY_MS: i64 = 86_400_000;
/// A backup is due a little before its day, so that hourly checks do not
/// push it later every week.
const EARLY_MS: i64 = 3_600_000;

/// Settings and last attempt of the automatic backups.
pub(super) struct AutoBackups {
    state: std::sync::Mutex<(AutoBackupConfig, Option<AutoBackupRun>)>,
    /// Wakes the checks up (new settings).
    wake: tokio::sync::Notify,
    /// One backup at a time.
    running: tokio::sync::Mutex<()>,
}

impl AutoBackups {
    /// With the settings of `config_file`'s folder.
    pub(super) fn load(config_file: Option<&Path>) -> Self {
        let settings = config_file
            .map(settings_file)
            .and_then(|path| match std::fs::read_to_string(&path) {
                Ok(text) => match toml::from_str::<AutoBackupConfig>(&text) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "automatic backups: settings unreadable, off");
                        None
                    }
                },
                Err(_) => None,
            })
            .unwrap_or_default();
        Self {
            state: std::sync::Mutex::new((settings, None)),
            wake: tokio::sync::Notify::new(),
            running: tokio::sync::Mutex::new(()),
        }
    }

    fn settings(&self) -> AutoBackupConfig {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .clone()
    }

    fn last(&self) -> Option<AutoBackupRun> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .clone()
    }

    fn set_last(&self, run: AutoBackupRun) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = Some(run);
    }
}

fn settings_file(config_file: &Path) -> PathBuf {
    config_file.with_file_name(MANAGED_BACKUP_FILE)
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// The destination `id` and the folder of the backups on it.
async fn folder(shared: &Shared, id: &str) -> Result<(Destination, PathBuf), String> {
    let state = shared.state.lock().await;
    if state
        .storage
        .destinations
        .iter()
        .any(|d| d.id == id && d.remote.is_some())
    {
        return Err(
            "destination distante (S3, SFTP) : son dossier n'est qu'une file d'envoi, choisissez un disque ou un partage"
                .to_owned(),
        );
    }
    let d = state
        .storage
        .destination(id)
        .ok_or_else(|| format!("aucune destination « {id} »"))?;
    let path = d.root.join(FOLDER);
    Ok((d, path))
}

/// Runs `f` off the runtime, giving up after [`SHARE_TIMEOUT`].
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    match tokio::time::timeout(SHARE_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("tâche interrompue".to_owned()),
        Err(_) => Err(format!(
            "pas de réponse du stockage en {} s",
            SHARE_TIMEOUT.as_secs()
        )),
    }
}

/// The backups of `folder`, newest first. Blocking.
fn list(folder: &Path) -> Vec<AutoBackupFile> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out: Vec<AutoBackupFile> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            if !name.starts_with(PREFIX) || !name.ends_with(SUFFIX) {
                return None;
            }
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let created_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            Some(AutoBackupFile {
                name,
                bytes: meta.len(),
                created_ms,
            })
        })
        .collect();
    // The names carry the time: newest first, whatever the clock of a share.
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// Writes `data` as a new backup in `folder` of `destination`, then deletes
/// the oldest beyond `keep`. Returns the file name. Blocking.
fn write(
    destination: &Destination,
    folder: &Path,
    data: &[u8],
    name: &str,
    keep: u32,
) -> Result<(), String> {
    // A share not mounted, read-only or refusing writes: never its empty
    // mount point on the system disk.
    if let Some(problem) = destination.problem() {
        return Err(problem);
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("{} : {e}", folder.display()))?;
    write_private_bytes(&folder.join(name), data)
        .map_err(|e| format!("{} : {e}", folder.join(name).display()))?;
    for old in list(folder).into_iter().skip(keep.max(1) as usize) {
        if let Err(e) = std::fs::remove_file(folder.join(&old.name)) {
            warn!(file = old.name, error = %e, "cannot delete an old automatic backup");
        }
    }
    Ok(())
}

/// Makes a backup now with the stored settings.
pub(super) async fn run_now(shared: &Shared) -> AutoBackupRun {
    let _one = shared.auto_backup.running.lock().await;
    let settings = shared.auto_backup.settings();
    let at_ms = now_ms();
    let result = make(shared, &settings, at_ms).await;
    let run = match result {
        Ok(name) => {
            info!(
                file = name,
                destination = settings.destination,
                "automatic backup made"
            );
            AutoBackupRun {
                at_ms,
                file: Some(name),
                error: None,
            }
        }
        Err(e) => {
            let again = shared
                .auto_backup
                .last()
                .and_then(|l| l.error)
                .is_some_and(|previous| previous == e);
            if !again {
                warn!(error = %e, "automatic backup failed");
            }
            AutoBackupRun {
                at_ms,
                file: None,
                error: Some(e),
            }
        }
    };
    shared.auto_backup.set_last(run.clone());
    run
}

async fn make(shared: &Shared, settings: &AutoBackupConfig, at_ms: i64) -> Result<String, String> {
    let passphrase = settings
        .passphrase
        .clone()
        .filter(|p| p.chars().count() >= MIN_PASSPHRASE)
        .ok_or_else(|| "pas de phrase secrète enregistrée".to_owned())?;
    if settings.destination.is_empty() {
        return Err("pas de destination choisie".to_owned());
    }
    let (destination, folder) = folder(shared, &settings.destination).await?;
    let data = backups::make(shared, passphrase)
        .await
        .map_err(|e| e.to_string())?;
    let stamp = jiff::Timestamp::from_millisecond(at_ms)
        .unwrap_or_else(|_| jiff::Timestamp::now())
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y%m%d-%H%M%S")
        .to_string();
    let name = format!("{PREFIX}{stamp}{SUFFIX}");
    let keep = settings.keep;
    let file = name.clone();
    blocking(move || write(&destination, &folder, &data, &file, keep)).await?;
    Ok(name)
}

/// When the next backup is due after `newest` (`None`: none yet).
fn due_ms(newest: Option<&AutoBackupFile>, every_days: u32) -> Option<i64> {
    newest.map(|b| b.created_ms + i64::from(every_days) * DAY_MS)
}

/// Makes the backups when they are due, checking every hour (and at once
/// when the settings change).
pub(super) async fn run(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut wait = FIRST_CHECK;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
            _ = shared.auto_backup.wake.notified() => {}
        }
        wait = CHECK;
        let settings = shared.auto_backup.settings();
        if !settings.enabled {
            continue;
        }
        let newest = match folder(&shared, &settings.destination).await {
            Ok((_, f)) => blocking(move || Ok(list(&f).into_iter().next()))
                .await
                .ok()
                .flatten(),
            Err(_) => None,
        };
        if due_ms(newest.as_ref(), settings.every_days).is_some_and(|due| now_ms() < due - EARLY_MS)
        {
            continue;
        }
        run_now(&shared).await;
    }
}

/// What the interface shows.
pub(super) async fn view(shared: &Shared) -> AutoBackupView {
    let s = shared.auto_backup.settings();
    let (folder_path, folder_problem, backups) = if s.destination.is_empty() {
        (None, None, Vec::new())
    } else {
        match folder(shared, &s.destination).await {
            Ok((d, f)) => {
                let shown = f.display().to_string();
                let checked = blocking(move || Ok((d.unavailable_reason(), list(&f)))).await;
                match checked {
                    Ok((problem, list)) => (Some(shown), problem, list),
                    Err(e) => (Some(shown), Some(e), Vec::new()),
                }
            }
            Err(e) => (None, Some(e), Vec::new()),
        }
    };
    AutoBackupView {
        enabled: s.enabled,
        destination: s.destination.clone(),
        every_days: s.every_days,
        keep: s.keep,
        has_passphrase: s.passphrase.is_some(),
        folder: folder_path,
        folder_problem,
        next_ms: s
            .enabled
            .then(|| due_ms(backups.first(), s.every_days).unwrap_or_else(now_ms)),
        backups,
        last: shared.auto_backup.last(),
    }
}

/// Stores new settings (the passphrase kept when none is given).
pub(super) async fn set(
    shared: &Shared,
    input: AutoBackupInput,
) -> Result<AutoBackupView, ControlError> {
    let config_file = shared.config_file.clone().ok_or_else(|| {
        ControlError::Internal("la configuration ne vient pas d'un fichier".to_owned())
    })?;
    let previous = shared.auto_backup.settings();
    let settings = AutoBackupConfig {
        enabled: input.enabled,
        destination: input.destination.trim().to_owned(),
        every_days: input.every_days,
        keep: input.keep,
        passphrase: input
            .passphrase
            .filter(|p| !p.is_empty())
            .or(previous.passphrase),
    };
    let mut problems = settings.problems(MIN_PASSPHRASE);
    if !settings.destination.is_empty()
        && let Err(e) = folder(shared, &settings.destination).await
    {
        problems.push(e);
    }
    if !problems.is_empty() {
        return Err(ControlError::Invalid(problems.join("; ")));
    }
    let body = toml::to_string(&settings).map_err(|e| ControlError::Internal(e.to_string()))?;
    let text = format!(
        "# Sauvegardes automatiques choisies dans l'interface web de RustNVR (fichier\n\
         # géré automatiquement, à garder privé : il contient la phrase secrète).\n\n{body}"
    );
    let path = settings_file(&config_file);
    tokio::task::spawn_blocking(move || write_private_bytes(&path, text.as_bytes()))
        .await
        .map_err(|_| ControlError::Internal("enregistrement interrompu".to_owned()))?
        .map_err(|e| ControlError::Internal(format!("{MANAGED_BACKUP_FILE} : {e}")))?;
    info!(
        enabled = settings.enabled,
        destination = settings.destination,
        every_days = settings.every_days,
        keep = settings.keep,
        "automatic backups changed"
    );
    shared
        .auto_backup
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0 = settings;
    shared.auto_backup.wake.notify_one();
    Ok(view(shared).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(root: &Path) -> Destination {
        Destination {
            id: "nas".into(),
            name: "NAS".into(),
            root: root.to_owned(),
            require_mount: false,
        }
    }

    #[test]
    fn keeps_the_newest_backups_only() {
        let dir = tempfile::tempdir().expect("tmp");
        let d = destination(dir.path());
        let folder = dir.path().join(FOLDER);
        for (i, day) in ["20261101", "20261108", "20261115", "20261122"]
            .iter()
            .enumerate()
        {
            let name = format!("{PREFIX}{day}-033000{SUFFIX}");
            write(&d, &folder, &[i as u8; 10], &name, 3).expect("write");
        }
        // Another file of the folder is left alone.
        std::fs::write(folder.join("notes.txt"), "x").expect("notes");
        let names: Vec<String> = list(&folder).into_iter().map(|b| b.name).collect();
        assert_eq!(
            names,
            [
                "rustnvr-config-20261122-033000.rnvrbak",
                "rustnvr-config-20261115-033000.rnvrbak",
                "rustnvr-config-20261108-033000.rnvrbak",
            ]
        );
        assert!(folder.join("notes.txt").is_file());
        assert_eq!(list(&folder)[0].bytes, 10);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(folder.join(&names[0]))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_missing_share_is_never_written() {
        let dir = tempfile::tempdir().expect("tmp");
        let gone = dir.path().join("nas-not-mounted");
        let d = destination(&gone);
        let err = write(&d, &gone.join(FOLDER), b"x", "rustnvr-config-1.rnvrbak", 4)
            .expect_err("no share");
        assert!(err.contains("inaccessible"), "{err}");
        assert!(!gone.exists());
    }

    #[test]
    fn due_once_the_newest_is_old_enough() {
        assert_eq!(due_ms(None, 7), None);
        let b = AutoBackupFile {
            name: String::new(),
            bytes: 1,
            created_ms: 1_000,
        };
        assert_eq!(due_ms(Some(&b), 7), Some(1_000 + 7 * DAY_MS));
    }
}
