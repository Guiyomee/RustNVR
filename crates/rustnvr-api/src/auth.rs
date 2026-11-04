//! Authentication: accounts with sessions (cookie), HTTP Basic for scripts.
//!
//! - The interface files are public (they contain no data): the login page
//!   must load. Everything under `/api/` requires a session or Basic
//!   credentials, except the login routes.
//! - Accounts live in the database: password hashed with Argon2, session
//!   token random (256 bits), stored as a SHA-256 hash, sent in an
//!   `HttpOnly`, `SameSite=Strict` cookie.
//! - Role `viewer`: read only (GET); `admin`: everything. Requests that change
//!   something must also come from this origin (`Origin` check, in addition
//!   to `SameSite`).
//! - Five failed logins in 15 minutes from one address or for one account
//!   block further attempts for 15 minutes.
//! - Without any account and without `web.username`, access is open (the
//!   interface then offers to create the first administrator); RustNVR only
//!   listens on loopback in that case (see `WebConfig`).
//!
//! Without a database, the `web.username`/`web.password` of the
//! configuration are checked with HTTP Basic, as before accounts existed.

use crate::routes::ApiError;
use crate::state::{ApiState, unix_ms};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use axum::Json;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Name of the session cookie.
pub(crate) const SESSION_COOKIE: &str = "rustnvr_session";
/// Sessions last this long.
const SESSION_DAYS: i64 = 30;
const MAX_FAILURES: u32 = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
const MIN_PASSWORD: usize = 8;

/// Who is making a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Principal {
    /// Account name (`None` when access is open).
    pub username: Option<String>,
    /// `admin` or `viewer`.
    pub role: &'static str,
}

impl Principal {
    pub(crate) fn admin(&self) -> bool {
        self.role == "admin"
    }
}

fn role_name(role: &str) -> &'static str {
    if role == "admin" { "admin" } else { "viewer" }
}

/// Compares without short-circuiting on the first differing byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Argon2 hash (PHC string) of a password. CPU-bound: call from a blocking thread.
pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Checks a password against its stored hash. CPU-bound.
fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// A new session token and the hash stored in the database.
fn new_token() -> Result<(String, String), String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = token_hash(&token);
    Ok((token, hash))
}

fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let encoded = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Basic ")?;
    let decoded = B64.decode(encoded.trim()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (u, p) = text.split_once(':')?;
    Some((u.to_owned(), p.to_owned()))
}

/// Failed login attempts, by client address and by account.
#[derive(Default)]
pub(crate) struct LoginGuard {
    failures: std::sync::Mutex<HashMap<String, (u32, Instant)>>,
}

impl LoginGuard {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u32, Instant)>> {
        self.failures.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Seconds before `keys` may try again, if blocked.
    fn blocked(&self, keys: &[String]) -> Option<u64> {
        let mut map = self.lock();
        map.retain(|_, (_, since)| since.elapsed() < FAILURE_WINDOW);
        keys.iter()
            .filter_map(|k| map.get(k))
            .filter(|(n, _)| *n >= MAX_FAILURES)
            .map(|(_, since)| {
                FAILURE_WINDOW
                    .saturating_sub(since.elapsed())
                    .as_secs()
                    .max(1)
            })
            .max()
    }

    fn failed(&self, keys: &[String]) {
        let mut map = self.lock();
        for k in keys {
            let e = map.entry(k.clone()).or_insert((0, Instant::now()));
            e.0 += 1;
        }
    }

    fn succeeded(&self, keys: &[String]) {
        let mut map = self.lock();
        for k in keys {
            map.remove(k);
        }
    }
}

impl ApiState {
    /// Whether requests must be authenticated.
    pub(crate) fn auth_required(&self) -> bool {
        self.has_users.load(std::sync::atomic::Ordering::Relaxed) || self.auth.is_some()
    }

    /// Records whether accounts exist (set at startup and on changes).
    pub fn set_has_users(&self, has: bool) {
        self.has_users
            .store(has, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The principal of a request, if its credentials are valid.
async fn identify(state: &ApiState, headers: &HeaderMap) -> Option<Principal> {
    if !state.auth_required() {
        return Some(Principal {
            username: None,
            role: "admin",
        });
    }
    let db = state.database.get();
    if let (Some(db), Some(token)) = (db, cookie(headers, SESSION_COOKIE))
        && let Ok(Some((username, role))) = db
            .session_user(&token_hash(&token), unix_ms(SystemTime::now()))
            .await
    {
        return Some(Principal {
            username: Some(username),
            role: role_name(&role),
        });
    }
    let (user, password) = basic_credentials(headers)?;
    if let Some(db) = db
        && state.has_users.load(std::sync::atomic::Ordering::Relaxed)
    {
        let (hash, role) = db.user_secret(&user).await.ok()??;
        let ok = tokio::task::spawn_blocking(move || verify_password(&hash, &password))
            .await
            .unwrap_or(false);
        return ok.then(|| Principal {
            username: Some(user),
            role: role_name(&role),
        });
    }
    // Configuration credentials (no database, or no account yet).
    let (u, p) = state.auth.as_ref()?;
    // Evaluate both comparisons to avoid revealing which one failed.
    let ok = constant_time_eq(user.as_bytes(), u.as_bytes())
        & constant_time_eq(password.as_bytes(), p.expose().as_bytes());
    ok.then_some(Principal {
        username: Some(user),
        role: "admin",
    })
}

/// Routes reachable without credentials.
fn public(path: &str) -> bool {
    !path.starts_with("/api/")
        || matches!(
            path,
            "/api/v1/auth/status" | "/api/v1/auth/login" | "/api/v1/auth/setup"
        )
}

/// Requests allowed to viewers: reading, their own session and password,
/// and opening a WebRTC live view (a POST, but it changes nothing).
fn viewer_may(method: &Method, path: &str) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
        || matches!(path, "/api/v1/auth/logout" | "/api/v1/auth/password")
        || (*method == Method::POST
            && path
                .strip_prefix("/api/v1/cameras/")
                .and_then(|p| {
                    p.strip_suffix("/live/webrtc")
                        .or_else(|| p.strip_suffix("/export"))
                })
                .is_some_and(|id| !id.is_empty() && !id.contains('/')))
}

/// True when a state-changing request comes from another site.
fn cross_origin(headers: &HeaderMap) -> bool {
    let (Some(origin), Some(host)) = (
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        headers.get(header::HOST).and_then(|v| v.to_str().ok()),
    ) else {
        return false; // non-browser clients send no Origin
    };
    origin
        .split_once("://")
        .is_none_or(|(_, rest)| rest.trim_end_matches('/') != host)
}

/// Middleware: authenticates `/api/` requests and enforces roles.
pub(crate) async fn require_auth(
    State(state): State<Arc<ApiState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_owned();
    let mutating = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if mutating && path.starts_with("/api/") && cross_origin(req.headers()) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "cross-origin request refused",
        )
        .into_response();
    }
    // Without accounts there is no login page: everything stays behind Basic.
    if public(&path) && state.database.get().is_some() {
        return next.run(req).await;
    }
    let Some(principal) = identify(&state, req.headers()).await else {
        tracing::debug!(%path, "rejected unauthenticated request");
        let mut response = ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "authentication required",
        )
        .into_response();
        // Without accounts (configuration credentials only), let browsers
        // ask for them as before; with accounts, the interface shows its
        // login page instead of the browser prompt.
        if state.database.get().is_none() {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"RustNVR\", charset=\"UTF-8\""),
            );
        }
        return response;
    };
    if !principal.admin() && !viewer_may(req.method(), &path) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "ce compte ne peut que consulter (rôle lecteur)",
        )
        .into_response();
    }
    req.extensions_mut().insert(principal);
    next.run(req).await
}

fn session_cookie(token: &str, secure: bool) -> HeaderValue {
    let max_age = SESSION_DAYS * 86_400;
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static("rustnvr_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// Over built-in HTTPS or behind an HTTPS reverse proxy, the cookie is marked `Secure`.
fn over_https(state: &ApiState, headers: &HeaderMap) -> bool {
    if state.https.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

/// `GET /auth/status` (public).
#[derive(Serialize)]
pub(crate) struct StatusDto {
    /// Authentication is required.
    required: bool,
    /// Accounts can be managed (database available).
    accounts: bool,
    /// No account yet: the first administrator can be created.
    setup: bool,
    /// The current user, if logged in.
    user: Option<Principal>,
}

pub(crate) async fn status(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Json<StatusDto> {
    let has_users = state.has_users.load(std::sync::atomic::Ordering::Relaxed);
    Json(StatusDto {
        required: state.auth_required(),
        accounts: state.database.get().is_some(),
        setup: state.database.get().is_some() && !has_users,
        user: identify(&state, &headers).await,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginInput {
    username: String,
    password: String,
}

async fn open_session(
    state: &ApiState,
    username: &str,
    role: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("no database"))?;
    let (token, hash) = new_token().map_err(ApiError::unavailable)?;
    let now = unix_ms(SystemTime::now());
    db.create_session(&hash, username, now, now + SESSION_DAYS * 86_400_000)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    let mut response = Json(Principal {
        username: Some(username.to_owned()),
        role: role_name(role),
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&token, over_https(state, headers)),
    );
    Ok(response)
}

/// `POST /auth/login`.
pub(crate) async fn login(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<LoginInput>,
) -> Result<Response, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    let keys = [
        format!("ip:{}", peer.ip()),
        format!("user:{}", input.username),
    ];
    if let Some(wait) = state.login_guard.blocked(&keys) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            format!(
                "trop de tentatives : réessayez dans {} min",
                wait.div_ceil(60)
            ),
        ));
    }
    let secret = db
        .user_secret(&input.username)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    let password = input.password.clone();
    let verified = match secret {
        Some((hash, role)) => {
            tokio::task::spawn_blocking(move || verify_password(&hash, &password))
                .await
                .unwrap_or(false)
                .then_some(role)
        }
        None => {
            // Same cost as a real check: do not reveal which accounts exist.
            let _ = tokio::task::spawn_blocking(move || hash_password(&password)).await;
            None
        }
    };
    let Some(role) = verified else {
        state.login_guard.failed(&keys);
        tracing::warn!(user = %input.username, client = %peer.ip(), "failed login");
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "bad_credentials",
            "identifiant ou mot de passe incorrect",
        ));
    };
    state.login_guard.succeeded(&keys);
    tracing::info!(user = %input.username, client = %peer.ip(), "login");
    open_session(&state, &input.username, &role, &headers).await
}

/// `POST /auth/logout`.
pub(crate) async fn logout(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    if let (Some(db), Some(token)) = (state.database.get(), cookie(&headers, SESSION_COOKIE)) {
        let _ = db.delete_session(&token_hash(&token)).await;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_cookie());
    response
}

fn check_new_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(ApiError::bad_request(format!(
            "le mot de passe doit faire au moins {MIN_PASSWORD} caractères"
        )));
    }
    Ok(())
}

fn check_username(name: &str) -> Result<(), ApiError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "identifiant : 1 à 64 caractères parmi a-z, 0-9, . _ - @",
        ))
    }
}

async fn hash(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?
        .map_err(ApiError::unavailable)
}

/// `POST /auth/setup`: creates the first administrator (only without accounts).
pub(crate) async fn setup(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(input): Json<LoginInput>,
) -> Result<Response, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    if state.has_users.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            "un compte existe déjà",
        ));
    }
    // With configuration credentials, only their owner may create accounts.
    if state.auth.is_some() && identify(&state, &headers).await.is_none() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "authentication required",
        ));
    }
    check_username(&input.username)?;
    check_new_password(&input.password)?;
    let hash = hash(input.password).await?;
    let created = db
        .create_user(&input.username, &hash, "admin", unix_ms(SystemTime::now()))
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    if !created {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            "identifiant déjà utilisé",
        ));
    }
    state.set_has_users(true);
    tracing::info!(user = %input.username, "first administrator created");
    open_session(&state, &input.username, "admin", &headers).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordInput {
    current: String,
    new: String,
}

/// `PUT /auth/password`: changes one's own password (other sessions end).
pub(crate) async fn change_password(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
    Json(input): Json<PasswordInput>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    let username = principal
        .and_then(|p| p.0.username)
        .ok_or_else(|| ApiError::bad_request("aucun compte connecté"))?;
    let (stored, _) = db
        .user_secret(&username)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?
        .ok_or_else(|| ApiError::bad_request("ce compte n'existe pas dans la base"))?;
    let current = input.current;
    let ok = tokio::task::spawn_blocking(move || verify_password(&stored, &current))
        .await
        .unwrap_or(false);
    if !ok {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "mot de passe actuel incorrect",
        ));
    }
    check_new_password(&input.new)?;
    let hash = hash(input.new).await?;
    db.update_user(&username, Some(&hash), None)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    db.delete_sessions_of(&username)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    tracing::info!(user = %username, "password changed");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub(crate) struct UserDto {
    username: String,
    role: &'static str,
    created_ms: i64,
}

/// `GET /users` (admin).
pub(crate) async fn users(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    let list = db
        .users()
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    Ok(Json(
        list.into_iter()
            .map(|u| UserDto {
                role: role_name(&u.role),
                username: u.username,
                created_ms: u.created_ms,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserInput {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

fn parse_role(role: Option<&str>) -> Result<Option<&'static str>, ApiError> {
    match role {
        None => Ok(None),
        Some("admin") => Ok(Some("admin")),
        Some("viewer") => Ok(Some("viewer")),
        Some(_) => Err(ApiError::bad_request("rôle : admin ou viewer")),
    }
}

async fn admins(state: &ApiState) -> Result<usize, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    Ok(db
        .users()
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?
        .iter()
        .filter(|u| u.role == "admin")
        .count())
}

/// `POST /users` (admin).
pub(crate) async fn create_user(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<UserInput>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    let username = input.username.unwrap_or_default();
    check_username(&username)?;
    let password = input
        .password
        .ok_or_else(|| ApiError::bad_request("mot de passe obligatoire"))?;
    check_new_password(&password)?;
    let role = parse_role(input.role.as_deref())?.unwrap_or("viewer");
    let hash = hash(password).await?;
    if !db
        .create_user(&username, &hash, role, unix_ms(SystemTime::now()))
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            "identifiant déjà utilisé",
        ));
    }
    state.set_has_users(true);
    tracing::info!(user = %username, role, "account created");
    Ok(StatusCode::CREATED)
}

/// `PUT /users/{name}` (admin): new password and/or role.
pub(crate) async fn update_user(
    State(state): State<Arc<ApiState>>,
    Path(name): Path<String>,
    Json(input): Json<UserInput>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    let role = parse_role(input.role.as_deref())?;
    if role == Some("viewer") {
        let (_, current) = db
            .user_secret(&name)
            .await
            .map_err(|e| ApiError::unavailable(e.to_string()))?
            .ok_or_else(|| ApiError::not_found(format!("aucun compte {name:?}")))?;
        if current == "admin" && admins(&state).await? <= 1 {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "conflict",
                "il faut garder au moins un administrateur",
            ));
        }
    }
    let hash = match input.password {
        Some(p) => {
            check_new_password(&p)?;
            Some(hash(p).await?)
        }
        None => None,
    };
    if !db
        .update_user(&name, hash.as_deref(), role)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?
    {
        return Err(ApiError::not_found(format!("aucun compte {name:?}")));
    }
    if hash.is_some() || role.is_some() {
        // Sessions carry the old rights: log the account out everywhere.
        let _ = db.delete_sessions_of(&name).await;
    }
    tracing::info!(user = %name, "account changed");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /users/{name}` (admin).
pub(crate) async fn delete_user(
    State(state): State<Arc<ApiState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("accounts need the database"))?;
    let (_, role) = db
        .user_secret(&name)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("aucun compte {name:?}")))?;
    if role == "admin" && admins(&state).await? <= 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            "il faut garder au moins un administrateur",
        ));
    }
    db.delete_user(&name)
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
    let remaining = db.users().await.map(|u| !u.is_empty()).unwrap_or(true);
    state.set_has_users(remaining);
    tracing::info!(user = %name, "account deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_bytes() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
    }

    #[test]
    fn hashes_and_verifies_passwords() {
        let h = hash_password("correct horse").expect("hash");
        assert!(h.starts_with("$argon2"));
        assert!(verify_password(&h, "correct horse"));
        assert!(!verify_password(&h, "wrong horse"));
        let (t1, h1) = new_token().expect("token");
        let (t2, _) = new_token().expect("token");
        assert_ne!(t1, t2);
        assert_eq!(token_hash(&t1), h1);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn reads_cookies_and_blocks_after_failures() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; rustnvr_session=abc; b=2"),
        );
        assert_eq!(cookie(&h, SESSION_COOKIE).as_deref(), Some("abc"));
        let guard = LoginGuard::default();
        let keys = ["ip:1.2.3.4".to_owned()];
        for _ in 0..MAX_FAILURES {
            assert!(guard.blocked(&keys).is_none());
            guard.failed(&keys);
        }
        assert!(guard.blocked(&keys).is_some());
        guard.succeeded(&keys);
        assert!(guard.blocked(&keys).is_none());
    }

    #[test]
    fn detects_cross_origin_requests() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("nvr.local:8080"));
        assert!(!cross_origin(&h), "no Origin: not a browser form");
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://nvr.local:8080"),
        );
        assert!(!cross_origin(&h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(cross_origin(&h));
        assert!(viewer_may(&Method::GET, "/api/v1/cameras"));
        assert!(!viewer_may(&Method::PUT, "/api/v1/cameras/a"));
        assert!(viewer_may(&Method::POST, "/api/v1/auth/logout"));
        assert!(viewer_may(
            &Method::POST,
            "/api/v1/cameras/jardin/live/webrtc"
        ));
        assert!(!viewer_may(
            &Method::POST,
            "/api/v1/cameras/a/b/live/webrtc"
        ));
        assert!(!viewer_may(&Method::POST, "/api/v1/cameras"));
        assert!(!viewer_may(&Method::PUT, "/api/v1/cameras/jardin/audio"));
        // Exporting an extract changes nothing.
        assert!(viewer_may(&Method::POST, "/api/v1/cameras/jardin/export"));
        assert!(!viewer_may(&Method::PUT, "/api/v1/cameras/jardin/export"));
    }
}
