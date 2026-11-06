//! Backup and restore of the configuration, for the API (the format and the
//! checks are in [`crate::backup`]).

use super::Shared;
use crate::backup::{self, FileState};
use rustnvr_api::{BackupCamera, BackupFile, BackupPreview, BackupRestored, ControlError};
use rustnvr_core::{CameraId, MotionSettings};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing::{info, warn};

fn config_file(shared: &Shared) -> Result<PathBuf, ControlError> {
    shared.config_file.clone().ok_or_else(|| {
        ControlError::Internal("la configuration ne vient pas d'un fichier".to_owned())
    })
}

fn blocking_failed(_: tokio::task::JoinError) -> ControlError {
    ControlError::Internal("sauvegarde interrompue".to_owned())
}

/// The backup of the configuration, encrypted with `passphrase`.
pub(super) async fn make(shared: &Shared, passphrase: String) -> Result<Vec<u8>, ControlError> {
    if passphrase.chars().count() < backup::MIN_PASSPHRASE {
        return Err(ControlError::Invalid(format!(
            "phrase secrète : {} caractères au moins",
            backup::MIN_PASSPHRASE
        )));
    }
    let config = config_file(shared)?;
    let motion: BTreeMap<String, MotionSettings> = match &shared.database {
        Some(db) => db
            .motion_settings()
            .await
            .map_err(|e| ControlError::Internal(format!("réglages de détection : {e}")))?
            .into_iter()
            .collect(),
        None => BTreeMap::new(),
    };
    let now = jiff::Timestamp::now().as_millisecond();
    let sealed = tokio::task::spawn_blocking(move || {
        let b = backup::collect(&config, motion, now)
            .map_err(|e| ControlError::Internal(format!("lecture de la configuration : {e}")))?;
        let files = b.files.len();
        let cameras = b.motion.len();
        backup::seal(&b, &passphrase)
            .map(|data| (data, files, cameras))
            .map_err(|e| ControlError::Internal(e.to_string()))
    })
    .await
    .map_err(blocking_failed)??;
    info!(
        files = sealed.1,
        detection_settings = sealed.2,
        "configuration backup made"
    );
    Ok(sealed.0)
}

fn preview_dto(p: backup::Preview) -> BackupPreview {
    BackupPreview {
        created_ms: p.created_ms,
        rustnvr: p.rustnvr,
        files: p
            .files
            .into_iter()
            .map(|(name, state)| BackupFile {
                name,
                state: match state {
                    FileState::Same => "same",
                    FileState::Changed => "changed",
                    FileState::New => "new",
                    FileState::NotInBackup => "not_in_backup",
                },
            })
            .collect(),
        cameras: p
            .cameras
            .into_iter()
            .map(|(id, name)| BackupCamera { id, name })
            .collect(),
        motion: p.motion,
        problems: p.problems,
    }
}

fn opened(data: &[u8], passphrase: &str) -> Result<backup::Backup, ControlError> {
    backup::open(data, passphrase).map_err(|e| ControlError::Invalid(e.to_string()))
}

/// What restoring the backup `data` would do.
pub(super) async fn inspect(
    shared: &Shared,
    data: Vec<u8>,
    passphrase: String,
) -> Result<BackupPreview, ControlError> {
    let config = config_file(shared)?;
    tokio::task::spawn_blocking(move || {
        let b = opened(&data, &passphrase)?;
        Ok(preview_dto(backup::preview(&config, &b)))
    })
    .await
    .map_err(blocking_failed)?
}

/// Writes back the files `files` of the backup `data`, and its detection
/// settings when `motion`; used from the next start.
pub(super) async fn restore(
    shared: &Shared,
    data: Vec<u8>,
    passphrase: String,
    files: Vec<String>,
    motion: bool,
) -> Result<BackupRestored, ControlError> {
    let config = config_file(shared)?;
    if motion && shared.database.is_none() {
        return Err(ControlError::Invalid(
            "pas de base de données : les réglages de détection ne peuvent pas être restaurés"
                .to_owned(),
        ));
    }
    let (written, settings) = tokio::task::spawn_blocking(move || {
        let b = opened(&data, &passphrase)?;
        let written = backup::restore(&config, &b, &files).map_err(ControlError::Invalid)?;
        Ok::<_, ControlError>((written, b.motion))
    })
    .await
    .map_err(blocking_failed)??;
    let mut restored = 0;
    if motion && let Some(db) = &shared.database {
        for (camera, s) in settings {
            let Ok(id) = CameraId::new(&camera) else {
                continue;
            };
            if !s.problems().is_empty() {
                warn!(%camera, "detection settings of the backup are invalid: skipped");
                continue;
            }
            db.set_motion_settings(&id, &s)
                .await
                .map_err(|e| ControlError::Internal(format!("réglages de détection : {e}")))?;
            restored += 1;
        }
    }
    warn!(
        files = ?written,
        detection_settings = restored,
        "configuration backup restored: restart RustNVR to use it"
    );
    Ok(BackupRestored {
        files: written,
        motion: restored,
        restart_needed: true,
    })
}
