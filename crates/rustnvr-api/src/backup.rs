//! Backup and restore of the configuration (administrators only): the file
//! is made and read by the NVR ([`crate::NvrControl::backup`]); the API only
//! carries it, base64 inside JSON for the uploads.

use crate::control::{BackupPreview, BackupRestored};
use crate::routes::{ApiError, control};
use crate::state::ApiState;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::Deserialize;
use std::sync::Arc;

/// Largest backup accepted (they weigh a few kilobytes).
const MAX_BACKUP: usize = 1024 * 1024;

type Admin = Option<axum::Extension<crate::auth::Principal>>;

/// Backups hold every password: administrators only.
fn admin_only(principal: &Admin) -> Result<(), ApiError> {
    if principal.as_ref().is_some_and(|p| !p.admin()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "les sauvegardes sont réservées aux administrateurs",
        ));
    }
    Ok(())
}

/// `POST /backup` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupRequest {
    passphrase: String,
}

/// `POST /backup`: the encrypted backup, as a file to download.
pub(crate) async fn create(
    State(state): State<Arc<ApiState>>,
    principal: Admin,
    Json(input): Json<BackupRequest>,
) -> Result<Response, ApiError> {
    admin_only(&principal)?;
    let data = control(&state)?.backup(input.passphrase).await?;
    let day = jiff::Zoned::now().strftime("%Y%m%d-%H%M").to_string();
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"rustnvr-config-{day}.rnvrbak\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        data,
    )
        .into_response())
}

/// `POST /backup/inspect` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InspectRequest {
    /// The backup file, base64.
    data: String,
    passphrase: String,
}

fn decode(data: &str) -> Result<Vec<u8>, ApiError> {
    if data.len() > MAX_BACKUP * 4 / 3 + 4 {
        return Err(ApiError::bad_request(
            "fichier trop gros pour une sauvegarde",
        ));
    }
    base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| ApiError::bad_request("fichier illisible (base64 attendu)"))
}

/// `POST /backup/inspect`: what restoring this backup would do.
pub(crate) async fn inspect(
    State(state): State<Arc<ApiState>>,
    principal: Admin,
    Json(input): Json<InspectRequest>,
) -> Result<Json<BackupPreview>, ApiError> {
    admin_only(&principal)?;
    let data = decode(&input.data)?;
    Ok(Json(
        control(&state)?
            .backup_inspect(data, input.passphrase)
            .await?,
    ))
}

/// `POST /backup/restore` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreRequest {
    data: String,
    passphrase: String,
    /// Files to write back (names of [`BackupPreview::files`]).
    #[serde(default)]
    files: Vec<String>,
    /// Restore the detection settings of the cameras too.
    #[serde(default)]
    motion: bool,
}

/// `POST /backup/restore`: writes back the chosen parts; used from the next
/// start of the server.
pub(crate) async fn restore(
    State(state): State<Arc<ApiState>>,
    principal: Admin,
    Json(input): Json<RestoreRequest>,
) -> Result<Json<BackupRestored>, ApiError> {
    admin_only(&principal)?;
    if input.files.is_empty() && !input.motion {
        return Err(ApiError::bad_request("rien à restaurer"));
    }
    let data = decode(&input.data)?;
    let restored = control(&state)?
        .backup_restore(data, input.passphrase, input.files, input.motion)
        .await?;
    state.set_config_restored();
    Ok(Json(restored))
}
