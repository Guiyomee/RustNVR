//! The signed-in account's own security: its two-step login (TOTP) and its
//! API tokens. Every account, viewers too, manages its own; API tokens
//! cannot reach these routes (see `auth::require_auth`).

use crate::auth::{API_TOKEN_PREFIX, Principal, check_password, new_token, token_hash};
use crate::routes::ApiError;
use crate::state::{ApiState, unix_ms};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use rustnvr_storage::Database;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// A secret being set up waits this long for its first code.
const PENDING_FOR: Duration = Duration::from_secs(10 * 60);
/// Most API tokens per account.
const MAX_TOKENS: usize = 50;
/// Longest name of an API token, in characters.
const MAX_TOKEN_NAME: usize = 60;

fn unavailable(e: impl std::fmt::Display) -> ApiError {
    ApiError::unavailable(e.to_string())
}

fn database(state: &ApiState) -> Result<&Database, ApiError> {
    state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))
}

/// The name of the signed-in account (not open access).
fn account(principal: Option<axum::Extension<Principal>>) -> Result<String, ApiError> {
    principal
        .and_then(|p| p.0.username)
        .ok_or_else(|| ApiError::bad_request("aucun compte connecté"))
}

/// `GET /auth/totp`.
#[derive(Serialize)]
pub(crate) struct TotpStatus {
    /// The two-step login is on.
    enabled: bool,
    /// Recovery codes not used yet.
    recovery_codes_left: i64,
}

pub(crate) async fn totp_status(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
) -> Result<Json<TotpStatus>, ApiError> {
    let db = database(&state)?;
    let username = account(principal)?;
    let enabled = db
        .user_totp(&username)
        .await
        .map_err(unavailable)?
        .is_some();
    Ok(Json(TotpStatus {
        enabled,
        recovery_codes_left: if enabled {
            db.recovery_codes_left(&username)
                .await
                .map_err(unavailable)?
        } else {
            0
        },
    }))
}

/// `POST /auth/totp/setup`: a new secret to scan, not active until a first
/// code confirms it.
#[derive(Serialize)]
pub(crate) struct TotpSetup {
    /// The secret, for apps that take it typed.
    secret: String,
    /// The `otpauth://` address the QR code holds.
    uri: String,
    /// The QR code, as SVG.
    qr_svg: String,
}

pub(crate) async fn totp_setup(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
) -> Result<Json<TotpSetup>, ApiError> {
    let db = database(&state)?;
    let username = account(principal)?;
    if db
        .user_totp(&username)
        .await
        .map_err(unavailable)?
        .is_some()
    {
        return Err(ApiError::bad_request(
            "la double authentification est déjà active",
        ));
    }
    let secret = crate::totp::new_secret().map_err(unavailable)?;
    let uri = crate::totp::otpauth_uri(&username, &secret);
    let qr_svg = crate::totp::qr_svg(&uri).map_err(unavailable)?;
    let mut pending = state.totp_pending.lock().unwrap_or_else(|e| e.into_inner());
    pending.retain(|_, (_, since)| since.elapsed() < PENDING_FOR);
    pending.insert(username, (secret.clone(), Instant::now()));
    Ok(Json(TotpSetup {
        secret,
        uri,
        qr_svg,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeInput {
    code: String,
}

/// `POST /auth/totp/enable`: the first code of the app turns the two-step
/// login on; the recovery codes are given once, here.
#[derive(Serialize)]
pub(crate) struct TotpEnabled {
    recovery_codes: Vec<String>,
}

pub(crate) async fn totp_enable(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
    Json(input): Json<CodeInput>,
) -> Result<Json<TotpEnabled>, ApiError> {
    let db = database(&state)?;
    let username = account(principal)?;
    let secret = state
        .totp_pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&username)
        .filter(|(_, since)| since.elapsed() < PENDING_FOR)
        .map(|(secret, _)| secret.clone())
        .ok_or_else(|| {
            ApiError::bad_request("aucune activation en cours : recommencez depuis le début")
        })?;
    let now = unix_ms(SystemTime::now()) / 1000;
    let step = crate::totp::matching_step(&secret, &input.code, now).ok_or_else(|| {
        ApiError::bad_request("code incorrect : vérifiez l'heure du téléphone et réessayez")
    })?;
    let codes = crate::totp::new_recovery_codes().map_err(unavailable)?;
    let hashes: Vec<String> = codes
        .iter()
        .map(|c| token_hash(&crate::totp::normalize_recovery(c)))
        .collect();
    if !db
        .set_user_totp(&username, Some(&secret), &hashes)
        .await
        .map_err(unavailable)?
    {
        return Err(ApiError::bad_request("ce compte n'existe pas dans la base"));
    }
    // The code just typed cannot be used again to sign in.
    let _ = db.use_totp_step(&username, step).await;
    state
        .totp_pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&username);
    tracing::info!(user = %username, "two-step login turned on");
    Ok(Json(TotpEnabled {
        recovery_codes: codes,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordCheck {
    password: String,
}

/// `POST /auth/totp/disable`: the account turns its two-step login off,
/// with its password.
pub(crate) async fn totp_disable(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
    Json(input): Json<PasswordCheck>,
) -> Result<StatusCode, ApiError> {
    let db = database(&state)?;
    let username = account(principal)?;
    let (hash, _) = db
        .user_secret(&username)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::bad_request("ce compte n'existe pas dans la base"))?;
    if !check_password(hash, input.password).await {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "mot de passe incorrect",
        ));
    }
    db.set_user_totp(&username, None, &[])
        .await
        .map_err(unavailable)?;
    tracing::info!(user = %username, "two-step login turned off");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /users/{name}/totp` (administrators): turns the two-step login of
/// an account off, for someone who lost the phone and the recovery codes.
pub(crate) async fn remove_user_totp(
    State(state): State<Arc<ApiState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let db = database(&state)?;
    if !db
        .set_user_totp(&name, None, &[])
        .await
        .map_err(unavailable)?
    {
        return Err(ApiError::not_found(format!("aucun compte {name:?}")));
    }
    tracing::info!(user = %name, "two-step login removed by an administrator");
    Ok(StatusCode::NO_CONTENT)
}

/// An API token, without its secret.
#[derive(Serialize)]
pub(crate) struct TokenDto {
    id: i64,
    name: String,
    read_only: bool,
    created_ms: i64,
    last_used_ms: Option<i64>,
    /// The token itself, only in the answer that creates it.
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

/// `GET /auth/tokens`: the API tokens of the signed-in account.
pub(crate) async fn tokens(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
) -> Result<Json<Vec<TokenDto>>, ApiError> {
    let db = database(&state)?;
    let username = account(principal)?;
    Ok(Json(
        db.api_tokens(&username)
            .await
            .map_err(unavailable)?
            .into_iter()
            .map(|t| TokenDto {
                id: t.id,
                name: t.name,
                read_only: t.read_only,
                created_ms: t.created_ms,
                last_used_ms: t.last_used_ms,
                token: None,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TokenInput {
    name: String,
    #[serde(default)]
    read_only: bool,
}

/// `POST /auth/tokens`: a new API token; the answer is the only time it is
/// shown.
pub(crate) async fn create_token(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
    Json(input): Json<TokenInput>,
) -> Result<Json<TokenDto>, ApiError> {
    let db = database(&state)?;
    let username = account(principal)?;
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > MAX_TOKEN_NAME {
        return Err(ApiError::bad_request(format!(
            "nom du jeton : 1 à {MAX_TOKEN_NAME} caractères"
        )));
    }
    if db.api_tokens(&username).await.map_err(unavailable)?.len() >= MAX_TOKENS {
        return Err(ApiError::bad_request(format!(
            "{MAX_TOKENS} jetons au plus par compte : révoquez-en un"
        )));
    }
    let (secret, _) = new_token().map_err(unavailable)?;
    let token = format!("{API_TOKEN_PREFIX}{secret}");
    let now = unix_ms(SystemTime::now());
    let id = db
        .create_api_token(&username, name, &token_hash(&token), input.read_only, now)
        .await
        .map_err(unavailable)?;
    tracing::info!(user = %username, token = %name, read_only = input.read_only, "API token created");
    Ok(Json(TokenDto {
        id,
        name: name.to_owned(),
        read_only: input.read_only,
        created_ms: now,
        last_used_ms: None,
        token: Some(token),
    }))
}

/// `DELETE /auth/tokens/{id}`: revokes an API token of the signed-in account.
pub(crate) async fn delete_token(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let db = database(&state)?;
    let username = account(principal)?;
    if !db
        .delete_api_token(&username, id)
        .await
        .map_err(unavailable)?
    {
        return Err(ApiError::not_found(format!("aucun jeton {id}")));
    }
    tracing::info!(user = %username, token = id, "API token revoked");
    Ok(StatusCode::NO_CONTENT)
}
