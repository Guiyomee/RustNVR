//! Accounts and sessions through the HTTP API: first administrator, cookie
//! session, viewer role, failed-login lockout, logout, cross-origin refusal.
#![allow(clippy::unwrap_used, missing_docs)]

use rustnvr_app::Nvr;
use rustnvr_core::config::Config;
use rustnvr_core::events::event_bus;
use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

struct Reply {
    status: u16,
    headers: String,
    body: serde_json::Value,
}

/// Minimal HTTP/1.1 client with extra request headers.
async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
    extra: &str,
) -> Reply {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap();
    Reply {
        status: head.split(' ').nth(1).unwrap().parse().unwrap(),
        headers: head.to_owned(),
        body: serde_json::from_str(rest).unwrap_or(serde_json::Value::Null),
    }
}

fn session_cookie(r: &Reply) -> String {
    let line = r
        .headers
        .lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with("set-cookie: rustnvr_session=")
        })
        .unwrap();
    assert!(
        line.contains("HttpOnly") && line.contains("SameSite=Strict"),
        "{line}"
    );
    let value = line.split_once(": ").unwrap().1.split(';').next().unwrap();
    format!("Cookie: {value}\r\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accounts_sessions_and_roles() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[storage]\nrecordings_dir = \"{}\"\n[metrics]\nlog_interval_seconds = 0\n[web]\nlisten = \"127.0.0.1:0\"\nstatic_dir = \"/nonexistent-rustnvr-web\"\n",
            dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let addr = nvr.web_addr().unwrap();

    // No account yet: open access, the first administrator can be created.
    let r = http(addr, "GET", "/api/v1/auth/status", None, "").await;
    assert_eq!(
        (r.body["required"].as_bool(), r.body["setup"].as_bool()),
        (Some(false), Some(true))
    );
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/setup",
        Some(r#"{"username":"admin","password":"court"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 400, "password too short");
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/setup",
        Some(r#"{"username":"admin","password":"tres-secret-1"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let admin = session_cookie(&r);

    // Now every API route needs a session (no browser Basic prompt).
    let r = http(addr, "GET", "/api/v1/cameras", None, "").await;
    assert_eq!(r.status, 401);
    // Except the container health check, which says nothing more.
    let r = http(addr, "GET", "/api/v1/health", None, "").await;
    assert_eq!((r.status, r.body["status"].as_str()), (200, Some("ok")));
    assert!(!r.headers.to_ascii_lowercase().contains("www-authenticate"));
    assert_eq!(
        http(addr, "GET", "/api/v1/cameras", None, &admin)
            .await
            .status,
        200
    );
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/setup",
        Some(r#"{"username":"x","password":"tres-secret-2"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 409, "setup only once");

    // A viewer can watch but not change anything.
    let r = http(
        addr,
        "POST",
        "/api/v1/users",
        Some(r#"{"username":"lecteur","password":"lecteur-123","role":"viewer"}"#),
        &admin,
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.body);
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/login",
        Some(r#"{"username":"lecteur","password":"lecteur-123"}"#),
        "",
    )
    .await;
    assert_eq!((r.status, r.body["role"].as_str()), (200, Some("viewer")));
    let viewer = session_cookie(&r);
    // The settings have no page for a viewer: not readable either.
    for path in ["settings", "mqtt", "storage/destinations", "audit"] {
        assert_eq!(
            http(addr, "GET", &format!("/api/v1/{path}"), None, &viewer)
                .await
                .status,
            403,
            "{path}"
        );
    }
    assert_eq!(
        http(addr, "GET", "/api/v1/storage", None, &viewer)
            .await
            .status,
        200
    );
    let r = http(addr, "PUT", "/api/v1/mqtt", Some("{}"), &viewer).await;
    assert_eq!(r.status, 403);
    assert_eq!(
        http(addr, "GET", "/api/v1/users", None, &viewer)
            .await
            .status,
        403,
        "the list of accounts is for administrators"
    );
    let r = http(addr, "DELETE", "/api/v1/users/admin", None, &admin).await;
    assert_eq!(r.status, 409, "last administrator kept");

    // Cross-origin changes are refused even with a valid session.
    let evil = format!("{admin}Origin: https://evil.example\r\n");
    assert_eq!(
        http(addr, "DELETE", "/api/v1/users/lecteur", None, &evil)
            .await
            .status,
        403
    );

    // Five failures lock the account for a while.
    for _ in 0..5 {
        let r = http(
            addr,
            "POST",
            "/api/v1/auth/login",
            Some(r#"{"username":"lecteur","password":"faux-faux"}"#),
            "",
        )
        .await;
        assert_eq!(r.status, 401);
    }
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/login",
        Some(r#"{"username":"lecteur","password":"lecteur-123"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 429, "{}", r.body);

    // Logout ends the session.
    assert_eq!(
        http(addr, "POST", "/api/v1/auth/logout", None, &viewer)
            .await
            .status,
        204
    );
    assert_eq!(
        http(addr, "GET", "/api/v1/cameras", None, &viewer)
            .await
            .status,
        401
    );
    // Changing a password ends the account's sessions.
    let r = http(
        addr,
        "PUT",
        "/api/v1/users/lecteur",
        Some(r#"{"password":"nouveau-pass-1"}"#),
        &admin,
    )
    .await;
    assert_eq!(r.status, 204);
    nvr.shutdown().await;
}

/// Basic credentials, accepted on every route, are locked out like the login
/// page; a WebSocket opened by a page of another origin is refused even with
/// a valid session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn basic_guessing_is_locked_and_websockets_check_their_origin() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[storage]\nrecordings_dir = \"{}\"\n[metrics]\nlog_interval_seconds = 0\n[web]\nlisten = \"127.0.0.1:0\"\nstatic_dir = \"/nonexistent-rustnvr-web\"\n",
            dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let addr = nvr.web_addr().unwrap();
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/setup",
        Some(r#"{"username":"admin","password":"tres-secret-1"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let admin = session_cookie(&r);
    let basic = |password: &str| {
        let b64 = base64::engine::general_purpose::STANDARD.encode(format!("admin:{password}"));
        format!("Authorization: Basic {b64}\r\n")
    };

    // The right password works with Basic.
    let r = http(
        addr,
        "GET",
        "/api/v1/cameras",
        None,
        &basic("tres-secret-1"),
    )
    .await;
    assert_eq!(r.status, 200);
    // Five wrong ones lock it, the right one included, for a while.
    for _ in 0..5 {
        let r = http(addr, "GET", "/api/v1/cameras", None, &basic("devine")).await;
        assert_eq!(r.status, 401);
    }
    let r = http(
        addr,
        "GET",
        "/api/v1/cameras",
        None,
        &basic("tres-secret-1"),
    )
    .await;
    assert_eq!(r.status, 429, "{}", r.body);
    // The page's login is locked too (same account, same address).
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/login",
        Some(r#"{"username":"admin","password":"tres-secret-1"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 429);
    // An open session still works.
    assert_eq!(
        http(addr, "GET", "/api/v1/cameras", None, &admin)
            .await
            .status,
        200
    );

    // WebSocket from another origin: refused before the upgrade.
    let upgrade = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";
    let evil = format!("{admin}{upgrade}Origin: https://other.example:8443\r\n");
    let r = http(addr, "GET", "/api/v1/ws", None, &evil).await;
    assert_eq!(r.status, 403, "{}", r.body);
    // Same origin: switched to WebSocket (the status line is enough).
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /api/v1/ws HTTP/1.1\r\nHost: test\r\n{admin}{upgrade}Origin: http://test\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut first = [0u8; 12];
    tokio::time::timeout(std::time::Duration::from_secs(5), s.read_exact(&mut first))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&first, b"HTTP/1.1 101");
}

/// A viewer limited to one camera does not hear of the others; logins and
/// changes are written to the activity log, read by administrators.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewers_limited_to_cameras_and_activity_log() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
            [storage]
            recordings_dir = "{rec}"
            [metrics]
            log_interval_seconds = 0
            [web]
            listen = "127.0.0.1:0"
            static_dir = "/nonexistent-rustnvr-web"
            [[cameras]]
            id = "jardin"
            main = {{ url = "rtsp://127.0.0.1:1/a" }}
            [[cameras]]
            id = "garage"
            main = {{ url = "rtsp://127.0.0.1:1/b" }}
            "#,
            rec = dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let addr = nvr.web_addr().unwrap();
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/setup",
        Some(r#"{"username":"admin","password":"tres-secret-1"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let admin = session_cookie(&r);

    let r = http(
        addr,
        "POST",
        "/api/v1/users",
        Some(
            r#"{"username":"voisin","password":"voisin-1234","role":"viewer","cameras":["nulle-part"]}"#,
        ),
        &admin,
    )
    .await;
    assert_eq!(r.status, 400, "unknown camera: {}", r.body);
    let r = http(
        addr,
        "POST",
        "/api/v1/users",
        Some(
            r#"{"username":"voisin","password":"voisin-1234","role":"viewer","cameras":["jardin"]}"#,
        ),
        &admin,
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.body);
    let r = http(addr, "GET", "/api/v1/users", None, &admin).await;
    let account = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "voisin")
        .unwrap();
    assert_eq!(account["cameras"], serde_json::json!(["jardin"]));

    let r = http(
        addr,
        "POST",
        "/api/v1/auth/login",
        Some(r#"{"username":"voisin","password":"voisin-1234"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["cameras"], serde_json::json!(["jardin"]));
    let voisin = session_cookie(&r);
    let ids = |r: &Reply| {
        r.body
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let r = http(addr, "GET", "/api/v1/cameras", None, &voisin).await;
    assert_eq!(ids(&r), ["jardin"]);
    let r = http(addr, "GET", "/api/v1/cameras", None, &admin).await;
    assert_eq!(ids(&r).len(), 2);
    // The other camera does not exist for this account, whatever the route.
    for path in [
        "cameras/garage",
        "cameras/garage/recordings?from=0&to=1000",
        "cameras/garage/snapshot",
        "cameras/garage/live",
        "cameras/%67arage",
        "recordings/local/garage/2026-11-03/20261103T101500.000Z.mp4",
        "ai/captures/garage/2026-11-03",
    ] {
        let r = http(addr, "GET", &format!("/api/v1/{path}"), None, &voisin).await;
        assert_eq!(r.status, 404, "{path}: {}", r.body);
    }
    assert_eq!(
        http(addr, "GET", "/api/v1/cameras/jardin", None, &voisin)
            .await
            .status,
        200
    );
    // Basic credentials carry the same limit as a session.
    let basic = "Authorization: Basic dm9pc2luOnZvaXNpbi0xMjM0\r\n"; // voisin:voisin-1234
    assert_eq!(
        http(addr, "GET", "/api/v1/cameras/garage", None, basic)
            .await
            .status,
        404
    );
    assert_eq!(
        http(addr, "GET", "/api/v1/cameras/jardin", None, basic)
            .await
            .status,
        200
    );
    let r = http(addr, "GET", "/api/v1/storage", None, &voisin).await;
    assert!(
        r.body["cameras"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["camera"] == "jardin"),
        "{}",
        r.body["cameras"]
    );

    // Rights changed by the administrator apply at the next request,
    // without logging in again.
    let r = http(
        addr,
        "PUT",
        "/api/v1/users/voisin",
        Some(r#"{"cameras":null}"#),
        &admin,
    )
    .await;
    assert_eq!(r.status, 204, "{}", r.body);
    let r = http(addr, "GET", "/api/v1/cameras", None, &voisin).await;
    assert_eq!(ids(&r).len(), 2, "all cameras again");
    let r = http(
        addr,
        "PUT",
        "/api/v1/users/voisin",
        Some(r#"{"cameras":[]}"#),
        &admin,
    )
    .await;
    assert_eq!(r.status, 204, "{}", r.body);
    let r = http(addr, "GET", "/api/v1/cameras", None, &voisin).await;
    assert!(ids(&r).is_empty(), "no camera");

    // The activity log: logins (a failed one too) and changes, not reads.
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/login",
        Some(r#"{"username":"voisin","password":"mauvais-mot"}"#),
        "",
    )
    .await;
    assert_eq!(r.status, 401);
    let r = http(addr, "PUT", "/api/v1/mqtt", Some("{}"), &voisin).await;
    assert_eq!(r.status, 403);
    assert_eq!(
        http(addr, "GET", "/api/v1/audit", None, &voisin)
            .await
            .status,
        403
    );
    let r = http(addr, "GET", "/api/v1/audit", None, &admin).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let lines: Vec<(String, String, u64)> = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["username"].as_str().unwrap_or("-").to_owned(),
                e["action"].as_str().unwrap().to_owned(),
                e["status"].as_u64().unwrap(),
            )
        })
        .collect();
    let has = |user: &str, action: &str, status: u64| {
        lines
            .iter()
            .any(|(u, a, s)| u == user && a == action && *s == status)
    };
    assert!(has("admin", "login", 200), "{lines:?}");
    assert!(has("admin", "POST /users", 201), "{lines:?}");
    assert!(has("admin", "POST /users", 400), "{lines:?}");
    assert!(has("admin", "PUT /users/voisin", 204), "{lines:?}");
    assert!(has("voisin", "login", 200), "{lines:?}");
    assert!(has("voisin", "login_failed", 401), "{lines:?}");
    assert!(has("voisin", "PUT /mqtt", 403), "{lines:?}");
    assert!(
        lines.iter().all(|(_, a, _)| !a.starts_with("GET ")),
        "reads are not logged: {lines:?}"
    );
    assert_eq!(lines[0].1, "PUT /mqtt", "newest first: {lines:?}");
    // Next page: older than the last line received.
    let r = http(addr, "GET", "/api/v1/audit?limit=2", None, &admin).await;
    let last = r.body.as_array().unwrap()[1]["id"].as_i64().unwrap();
    let r = http(
        addr,
        "GET",
        &format!("/api/v1/audit?limit=2&before={last}"),
        None,
        &admin,
    )
    .await;
    assert!(
        r.body
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["id"].as_i64().unwrap() < last),
        "{}",
        r.body
    );
    nvr.shutdown().await;
}

/// Opens a WebSocket on `path`; `None` when the server answers otherwise
/// than 101 (camera not streaming yet, refused…).
async fn ws_open(addr: SocketAddr, path: &str, cookie: &str) -> Option<TcpStream> {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: test\r\n{cookie}Origin: http://test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        if s.read(&mut byte).await.unwrap() == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    head.starts_with(b"HTTP/1.1 101").then_some(s)
}

/// Next frame sent by the server (never masked): opcode and payload; `None`
/// once the connection is closed, or after `wait`.
async fn ws_frame(s: &mut TcpStream, wait: Duration) -> Option<(u8, Vec<u8>)> {
    let read = async {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).await.ok()?;
        let len = match h[1] & 0x7f {
            126 => {
                let mut l = [0u8; 2];
                s.read_exact(&mut l).await.ok()?;
                u64::from(u16::from_be_bytes(l))
            }
            127 => {
                let mut l = [0u8; 8];
                s.read_exact(&mut l).await.ok()?;
                u64::from_be_bytes(l)
            }
            n => u64::from(n),
        };
        let mut payload = vec![0u8; usize::try_from(len).unwrap()];
        s.read_exact(&mut payload).await.ok()?;
        Some((h[0] & 0x0f, payload))
    };
    tokio::time::timeout(wait, read).await.ok().flatten()
}

/// The JSON messages of a state WebSocket received during `period`.
async fn ws_messages(s: &mut TcpStream, period: Duration) -> Vec<serde_json::Value> {
    let end = Instant::now() + period;
    let mut out = Vec::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match ws_frame(s, left).await {
            Some((1, text)) => out.push(serde_json::from_slice(&text).unwrap()),
            Some(_) => {}
            None => break,
        }
    }
    out
}

/// The long connections follow the rights of the account: the state
/// WebSocket of a viewer limited to one camera never names the other, its
/// live view closes within seconds once the camera is taken from it, and
/// it downloads the extracts of its cameras only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_connections_follow_the_camera_rights() {
    let dir = tempfile::tempdir().unwrap();
    let mut servers = Vec::new();
    for _ in 0..2 {
        let mut cfg = MockServerConfig::new(Fixture::h264());
        // 4x real time: segments every second or so, few frames to read.
        cfg.frame_interval = Duration::from_millis(25);
        servers.push(MockRtspServer::start(cfg).await.unwrap());
    }
    let rec = dir.path().join("rec");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
            [storage]
            recordings_dir = "{rec}"
            [recording]
            segment_seconds = 5
            [metrics]
            log_interval_seconds = 0
            [web]
            listen = "127.0.0.1:0"
            static_dir = "/nonexistent-rustnvr-web"
            [[cameras]]
            id = "jardin"
            main = {{ url = "{jardin}" }}
            [[cameras]]
            id = "garage"
            main = {{ url = "{garage}" }}
            "#,
            rec = rec.display(),
            jardin = servers[0].url(),
            garage = servers[1].url(),
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(256)).await.unwrap();
    let addr = nvr.web_addr().unwrap();
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/setup",
        Some(r#"{"username":"admin","password":"tres-secret-1"}"#),
        "",
    )
    .await;
    let admin = session_cookie(&r);
    let r = http(
        addr,
        "POST",
        "/api/v1/users",
        Some(
            r#"{"username":"voisin","password":"voisin-1234","role":"viewer","cameras":["jardin"]}"#,
        ),
        &admin,
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.body);
    let r = http(
        addr,
        "POST",
        "/api/v1/auth/login",
        Some(r#"{"username":"voisin","password":"voisin-1234"}"#),
        "",
    )
    .await;
    let voisin = session_cookie(&r);

    // The live view of the viewer, once the camera streams.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut live = loop {
        if let Some(s) = ws_open(addr, "/api/v1/cameras/jardin/live?stream=main", &voisin).await {
            break s;
        }
        assert!(Instant::now() < deadline, "jardin never streamed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let mut video = false;
    while let Some((op, _)) = ws_frame(&mut live, Duration::from_secs(10)).await {
        if op == 2 {
            video = true;
            break;
        }
    }
    assert!(video, "no video on the live view");

    // The state WebSocket: snapshot and a few seconds of events (both
    // cameras record a segment every second or so).
    let mut state = ws_open(addr, "/api/v1/ws", &voisin)
        .await
        .expect("state socket");
    let messages = ws_messages(&mut state, Duration::from_secs(4)).await;
    let cameras: Vec<&str> = messages
        .iter()
        .filter(|m| m["type"] == "camera")
        .map(|m| m["camera"]["id"].as_str().unwrap())
        .collect();
    assert!(cameras.contains(&"jardin"), "{cameras:?}");
    assert!(
        messages.iter().any(|m| m["type"] == "segment"),
        "segments are pushed: {messages:?}"
    );
    for m in &messages {
        assert!(
            !m.to_string().contains("garage"),
            "the other camera leaked: {m}"
        );
    }
    // The administrator hears of both.
    let mut all = ws_open(addr, "/api/v1/ws", &admin)
        .await
        .expect("admin socket");
    let messages = ws_messages(&mut all, Duration::from_secs(2)).await;
    assert!(messages.iter().any(|m| m.to_string().contains("garage")));

    // Garage instead of jardin: the open live view closes, the state
    // socket gets the new list of cameras.
    let r = http(
        addr,
        "PUT",
        "/api/v1/users/voisin",
        Some(r#"{"cameras":["garage"]}"#),
        &admin,
    )
    .await;
    assert_eq!(r.status, 204, "{}", r.body);
    let changed = Instant::now();
    let closed = loop {
        match ws_frame(&mut live, Duration::from_secs(12)).await {
            Some((8, payload)) => break String::from_utf8_lossy(&payload[2..]).into_owned(),
            Some(_) => {}
            None => panic!("the live view did not close"),
        }
    };
    assert_eq!(closed, "access to this camera withdrawn");
    assert!(
        changed.elapsed() < Duration::from_secs(10),
        "{:?}",
        changed.elapsed()
    );
    let messages = ws_messages(&mut state, Duration::from_secs(7)).await;
    let lists: Vec<&serde_json::Value> =
        messages.iter().filter(|m| m["type"] == "cameras").collect();
    let last = lists.last().expect("new list of cameras");
    let ids: Vec<&str> = last["cameras"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["garage"]);
    assert!(
        ws_open(addr, "/api/v1/cameras/jardin/live?stream=main", &voisin)
            .await
            .is_none(),
        "no new live view of jardin"
    );

    // Extracts: of its cameras only (as if FFmpeg had made them).
    let exports = rec.join(".rustnvr").join("exports");
    std::fs::create_dir_all(&exports).unwrap();
    for camera in ["jardin", "garage"] {
        std::fs::write(
            exports.join(format!("1-{camera}--{camera}_20261103-101500_5s.mp4")),
            b"mp4",
        )
        .unwrap();
    }
    let get = |file: &str, cookie: &str| {
        let path = format!("/api/v1/exports/{file}");
        let cookie = cookie.to_owned();
        async move { http(addr, "GET", &path, None, &cookie).await.status }
    };
    let jardin = "1-jardin--jardin_20261103-101500_5s.mp4";
    let garage = "1-garage--garage_20261103-101500_5s.mp4";
    assert_eq!(get(garage, &voisin).await, 200);
    assert_eq!(get(jardin, &voisin).await, 404);
    assert_eq!(get(jardin, &admin).await, 200);
    nvr.shutdown().await;
}
