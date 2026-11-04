//! Accounts and sessions through the HTTP API: first administrator, cookie
//! session, viewer role, failed-login lockout, logout, cross-origin refusal.
#![allow(clippy::unwrap_used, missing_docs)]

use rustnvr_app::Nvr;
use rustnvr_core::config::Config;
use rustnvr_core::events::event_bus;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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
    assert_eq!(
        http(addr, "GET", "/api/v1/settings", None, &viewer)
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
        200,
        "GET allowed"
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
