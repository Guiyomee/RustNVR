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
/// Failures allowed from one address before it waits.
const MAX_FAILURES: u32 = 5;
/// Failures allowed on one account, from all addresses together: higher, so
/// that a stranger cannot lock the administrator out with a few tries, low
/// enough to slow down guessing from many addresses.
const MAX_ACCOUNT_FAILURES: u32 = 20;
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

/// True for a viewer account (open access, without accounts, is not one).
pub(crate) fn is_viewer(principal: Option<&axum::Extension<Principal>>) -> bool {
    principal.is_some_and(|p| !p.admin())
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

/// Password hashes computed at a time. Each Argon2 computation takes about
/// 19 MiB and a core: without a bound, many requests at once (logins, Basic
/// credentials on any route, even without an account) exhaust the memory.
const PASSWORD_CHECKS: usize = 4;

fn password_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: std::sync::LazyLock<tokio::sync::Semaphore> =
        std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(PASSWORD_CHECKS));
    &SLOTS
}

/// [`verify_password`] on a blocking thread, a few at a time.
async fn check_password(hash: String, password: String) -> bool {
    let Ok(_slot) = password_slots().acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || verify_password(&hash, &password))
        .await
        .unwrap_or(false)
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

/// How many failures `key` (`ip:…` or `user:…`) may have.
fn limit(key: &str) -> u32 {
    if key.starts_with("user:") {
        MAX_ACCOUNT_FAILURES
    } else {
        MAX_FAILURES
    }
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
            .filter_map(|k| map.get(k).map(|v| (k, v)))
            .filter(|(k, (n, _))| *n >= limit(k))
            .map(|(_, (_, since))| {
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
        let ok = check_password(hash, password).await;
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

/// The address a request comes from: the peer of the connection, or, when
/// that peer is a declared reverse proxy, the client it names.
///
/// `X-Forwarded-For` lists the client then each proxy passed, each proxy
/// appending the address it received the request from: read from the right,
/// the first address that is not a trusted proxy is the client (anything
/// further left was written by that client, or by proxies not trusted).
/// From any other peer the header is ignored: a client could name anyone
/// and never be locked out.
pub(crate) fn client_ip(
    proxies: &[rustnvr_core::config::TrustedProxy],
    peer: std::net::IpAddr,
    headers: &HeaderMap,
) -> std::net::IpAddr {
    let trusted = |ip: std::net::IpAddr| proxies.iter().any(|p| p.contains(ip));
    let peer = peer.to_canonical();
    if !trusted(peer) {
        return peer;
    }
    let named = |name: &str| {
        headers
            .get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(|a| {
                a.trim()
                    .parse::<std::net::IpAddr>()
                    .map(|ip| ip.to_canonical())
            })
            .collect::<Vec<_>>()
    };
    for address in named("x-forwarded-for").into_iter().rev() {
        match address {
            Ok(ip) if trusted(ip) => {}
            Ok(ip) => return ip,
            // Not an address: nothing left of it can be believed.
            Err(_) => return peer,
        }
    }
    // Nginx's `X-Real-IP`, when it is the only one given.
    match named("x-real-ip").pop() {
        Some(Ok(ip)) => ip,
        _ => peer,
    }
}

impl ApiState {
    /// [`client_ip`] with the proxies of `web.trusted_proxies`.
    pub(crate) fn client_ip(
        &self,
        peer: std::net::IpAddr,
        headers: &HeaderMap,
    ) -> std::net::IpAddr {
        client_ip(
            self.trusted_proxies.get().map_or(&[], Vec::as_slice),
            peer,
            headers,
        )
    }
}

/// Routes reachable without credentials.
/// Path of the health check, always reachable without authentication.
const HEALTH_PATH: &str = "/api/v1/health";

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

/// True when a state-changing request (or a WebSocket) comes from a page of
/// another origin.
///
/// The page's origin must name this server as the browser reached it (the
/// `Host` header). Behind a reverse proxy, which says so with its
/// `X-Forwarded-*` headers (a page cannot add them itself), the host may
/// come without its port: Nginx Proxy Manager forwards `Host: $host`, the
/// name only, so only names are compared there (or `X-Forwarded-Host`, when
/// the proxy gives it).
fn cross_origin(headers: &HeaderMap) -> bool {
    let get = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let (Some(origin), Some(host)) = (get(header::ORIGIN), get(header::HOST)) else {
        return false; // non-browser clients send no Origin
    };
    let Some((_, origin)) = origin.split_once("://") else {
        return true; // "null" (sandboxed page, local file)
    };
    let origin = origin.trim_end_matches('/');
    if origin.eq_ignore_ascii_case(host) {
        return false;
    }
    let proxied = [
        "x-forwarded-for",
        "x-forwarded-proto",
        "x-forwarded-host",
        "x-real-ip",
    ]
    .iter()
    .any(|h| headers.contains_key(*h));
    if !proxied {
        return true;
    }
    if let Some(forwarded) = get(header::HeaderName::from_static("x-forwarded-host")) {
        return !origin.eq_ignore_ascii_case(forwarded.split(',').next().unwrap_or("").trim());
    }
    !hostname(origin).eq_ignore_ascii_case(hostname(host))
}

/// `name` of `name:port` (`[v6]:port` too).
fn hostname(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    authority
        .rsplit_once(':')
        .map_or(authority, |(name, _)| name)
}

/// Middleware: authenticates `/api/` requests and enforces roles.
pub(crate) async fn require_auth(
    State(state): State<Arc<ApiState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_owned();
    let mutating = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    // A WebSocket (live view, real-time state) is not bound by the browser's
    // same-origin rule, and the session cookie goes with it to any page of
    // the same site (another port of the same address): checked like a change.
    let websocket = req
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    if (mutating || websocket) && path.starts_with("/api/") && cross_origin(req.headers()) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "cross-origin request refused",
        )
        .into_response();
    }
    // The container health check, with or without accounts.
    if path == HEALTH_PATH {
        return next.run(req).await;
    }
    // Without accounts there is no login page: everything stays behind Basic.
    if public(&path) && state.database.get().is_some() {
        return next.run(req).await;
    }
    // Basic credentials, accepted on every route, are guessed like the login
    // page: same lockout (by address and by account).
    let basic_keys = basic_credentials(req.headers()).map(|(user, _)| {
        let ip = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| state.client_ip(c.0.ip(), req.headers()).to_string())
            .unwrap_or_default();
        [format!("ip:{ip}"), format!("user:{user}")]
    });
    if let Some(keys) = &basic_keys
        && state.auth_required()
        && let Some(wait) = state.login_guard.blocked(keys)
    {
        return ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            format!(
                "trop de tentatives : réessayez dans {} min",
                wait.div_ceil(60)
            ),
        )
        .into_response();
    }
    let identified = identify(&state, req.headers()).await;
    if let Some(keys) = &basic_keys
        && state.auth_required()
    {
        match &identified {
            // A session cookie may come with stale Basic credentials.
            Some(_) => state.login_guard.succeeded(keys),
            None => {
                state.login_guard.failed(keys);
                tracing::warn!(client = %keys[0].trim_start_matches("ip:"), "failed Basic authentication");
            }
        }
    }
    let Some(principal) = identified else {
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

/// The cookie is marked `Secure` when the browser's page is HTTPS.
fn over_https(state: &ApiState, headers: &HeaderMap) -> bool {
    browser_https(
        state.https.load(std::sync::atomic::Ordering::Relaxed),
        headers,
    )
}

/// Whether the browser reached this server over HTTPS. A reverse proxy says
/// so with `X-Forwarded-Proto` (the first one, the browser's): it wins over
/// the built-in HTTPS, which may only be the hop between the proxy and
/// RustNVR (Home Assistant on `http://…:8123` forwarding to RustNVR over
/// HTTPS). A `Secure` cookie on an HTTP page is dropped by the browser: the
/// login would succeed but the session would never come back.
fn browser_https(builtin: bool, headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map_or(builtin, |proto| proto.trim().eq_ignore_ascii_case("https"))
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
    let client = state.client_ip(peer.ip(), &headers);
    let keys = [format!("ip:{client}"), format!("user:{}", input.username)];
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
        Some((hash, role)) => check_password(hash, password).await.then_some(role),
        None => {
            // Same cost as a real check: do not reveal which accounts exist.
            let _slot = password_slots().acquire().await;
            let _ = tokio::task::spawn_blocking(move || hash_password(&password)).await;
            None
        }
    };
    let Some(role) = verified else {
        state.login_guard.failed(&keys);
        tracing::warn!(user = %input.username, %client, "failed login");
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "bad_credentials",
            "identifiant ou mot de passe incorrect",
        ));
    };
    state.login_guard.succeeded(&keys);
    tracing::info!(user = %input.username, %client, "login");
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
    let _slot = password_slots()
        .acquire()
        .await
        .map_err(|e| ApiError::unavailable(e.to_string()))?;
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
    let ok = check_password(stored, current).await;
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
/// `GET /users` (administrators: the interface lists accounts to them only).
pub(crate) async fn users(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<Principal>>,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    if is_viewer(principal.as_ref()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "la liste des comptes est réservée aux administrateurs",
        ));
    }
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

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                *k,
                HeaderValue::from_str(v).unwrap_or(HeaderValue::from_static("")),
            );
        }
        h
    }

    #[test]
    fn origins_are_checked_strictly_unless_a_proxy_dropped_the_port() {
        let direct = |origin: &str, host: &str| {
            cross_origin(&headers(&[("origin", origin), ("host", host)]))
        };
        assert!(!direct("https://192.168.1.211:8080", "192.168.1.211:8080"));
        assert!(!direct("https://NVR.lan", "nvr.lan"));
        // Another service of the same address: refused.
        assert!(direct("https://192.168.1.211:9443", "192.168.1.211:8080"));
        assert!(direct("https://evil.example", "192.168.1.211:8080"));
        assert!(direct("null", "192.168.1.211:8080"));
        // No Origin (scripts, curl): not a browser page.
        assert!(!cross_origin(&headers(&[("host", "nvr.lan")])));
        // Nginx Proxy Manager: `Host: $host`, without the port.
        let npm = |origin: &str, host: &str| {
            cross_origin(&headers(&[
                ("origin", origin),
                ("host", host),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-for", "192.168.1.20"),
            ]))
        };
        assert!(!npm("https://nvr.example.org:8443", "nvr.example.org"));
        assert!(!npm("https://nvr.example.org", "nvr.example.org"));
        assert!(npm("https://evil.example", "nvr.example.org"));
        // A proxy giving X-Forwarded-Host: that one, exactly.
        let fwd = |origin: &str| {
            cross_origin(&headers(&[
                ("origin", origin),
                ("host", "127.0.0.1:8080"),
                ("x-forwarded-host", "nvr.example.org:8443"),
            ]))
        };
        assert!(!fwd("https://nvr.example.org:8443"));
        assert!(fwd("https://nvr.example.org:9000"));
        assert_eq!(hostname("[::1]:8080"), "::1");
        assert_eq!(hostname("nvr.lan"), "nvr.lan");
    }

    #[test]
    fn the_cookie_follows_the_browsers_scheme() {
        let none = HeaderMap::new();
        assert!(browser_https(true, &none), "built-in HTTPS, no proxy");
        assert!(!browser_https(false, &none), "plain HTTP, no proxy");
        let proto = |v: &str| headers(&[("x-forwarded-proto", v)]);
        // Home Assistant on HTTP, RustNVR behind it over HTTPS: no `Secure`.
        assert!(!browser_https(true, &proto("http")));
        // An HTTPS proxy in front of plain HTTP: `Secure`.
        assert!(browser_https(false, &proto("https")));
        assert!(browser_https(false, &proto("HTTPS, http")));
        assert!(!browser_https(true, &proto("http, https")));
    }

    /// One address waits after 5 failures; the account itself only after
    /// 20 failures from all addresses, so a stranger cannot lock it with 5.
    #[test]
    fn addresses_lock_sooner_than_accounts() {
        let guard = LoginGuard::default();
        let from = |ip: u8| [format!("ip:10.0.0.{ip}"), "user:admin".to_owned()];
        for _ in 0..5 {
            guard.failed(&from(1));
        }
        assert!(
            guard.blocked(&from(1)).is_some(),
            "the guessing address waits"
        );
        assert!(
            guard.blocked(&from(2)).is_none(),
            "the owner elsewhere does not"
        );
        for ip in 2..=4 {
            for _ in 0..5 {
                guard.failed(&from(ip));
            }
        }
        assert!(
            guard.blocked(&from(9)).is_some(),
            "20 failures: the account waits"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_client_behind_a_declared_proxy_is_the_one_it_names() {
        use rustnvr_core::config::TrustedProxy;
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        let proxies = [
            TrustedProxy::parse("192.168.1.5").unwrap(),
            TrustedProxy::parse("10.0.0.0/8").unwrap(),
        ];
        let from = |peer: &str, pairs: &[(&'static str, &str)]| {
            client_ip(&proxies, ip(peer), &headers(pairs))
        };
        // No proxy involved.
        assert_eq!(from("203.0.113.7", &[]), ip("203.0.113.7"));
        // A client naming itself someone else is not believed.
        assert_eq!(
            from("203.0.113.7", &[("x-forwarded-for", "198.51.100.1")]),
            ip("203.0.113.7")
        );
        // Through the declared proxy: the client it names.
        assert_eq!(
            from("192.168.1.5", &[("x-forwarded-for", "203.0.113.7")]),
            ip("203.0.113.7")
        );
        // What the client itself wrote on the left is not believed either.
        assert_eq!(
            from(
                "192.168.1.5",
                &[("x-forwarded-for", "198.51.100.1, 203.0.113.7")]
            ),
            ip("203.0.113.7")
        );
        // A chain of declared proxies; IPv4 seen through an IPv6 socket.
        assert_eq!(
            from(
                "::ffff:192.168.1.5",
                &[("x-forwarded-for", "203.0.113.7, 10.1.2.3")]
            ),
            ip("203.0.113.7")
        );
        // Only proxies named, garbage, or nothing: the proxy itself.
        assert_eq!(
            from("192.168.1.5", &[("x-forwarded-for", "10.1.2.3")]),
            ip("192.168.1.5")
        );
        assert_eq!(
            from(
                "192.168.1.5",
                &[("x-forwarded-for", "203.0.113.7, pas-une-adresse")]
            ),
            ip("192.168.1.5")
        );
        assert_eq!(from("192.168.1.5", &[]), ip("192.168.1.5"));
        assert_eq!(
            from("192.168.1.5", &[("x-real-ip", "203.0.113.9")]),
            ip("203.0.113.9")
        );

        // Two clients behind the proxy no longer share one lockout.
        let guard = LoginGuard::default();
        let key = |client: &str| {
            vec![format!(
                "ip:{}",
                from("192.168.1.5", &[("x-forwarded-for", client)])
            )]
        };
        for _ in 0..MAX_FAILURES {
            guard.failed(&key("203.0.113.7"));
        }
        assert!(guard.blocked(&key("203.0.113.7")).is_some());
        assert!(guard.blocked(&key("203.0.113.8")).is_none());

        assert!(TrustedProxy::parse("0.0.0.0/0").is_err());
        assert!(TrustedProxy::parse("192.168.1.0/33").is_err());
        assert!(TrustedProxy::parse("proxy.lan").is_err());
        assert!(
            TrustedProxy::parse("fd00::/8")
                .unwrap()
                .contains(ip("fd12::1"))
        );
        assert!(
            !TrustedProxy::parse("192.168.1.0/24")
                .unwrap()
                .contains(ip("192.168.2.1"))
        );
    }

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
