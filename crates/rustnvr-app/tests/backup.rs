//! Backup and restore of the configuration through the HTTP API: encrypted
//! download, preview, partial restore, restart flag.
#![allow(clippy::unwrap_used, missing_docs)]

use base64::Engine as _;
use rustnvr_app::Nvr;
use rustnvr_core::config::Config;
use rustnvr_core::events::event_bus;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Minimal HTTP/1.1 client: status and raw body.
async fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, raw[end + 4..].to_vec())
}

async fn json(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
    let (status, body) = http(addr, method, path, body).await;
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_preview_and_partial_restore() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[storage]\nrecordings_dir = \"{}\"\n[metrics]\nlog_interval_seconds = 0\n[web]\nlisten = \"127.0.0.1:0\"\nstatic_dir = \"/nonexistent-rustnvr-web\"\n\n[[cameras]]\nid = \"jardin\"\nname = \"Jardin\"\nenabled = false\nmain = {{ url = \"rtsp://127.0.0.1:9/live\" }}\n",
            dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let addr = nvr.web_addr().unwrap();

    let (status, caps) = json(addr, "GET", "/api/v1/capabilities", "").await;
    assert_eq!((status, caps["backup"].as_bool()), (200, Some(true)));

    // A webhook (with a token), then a backup.
    let hooks =
        r#"[{"name":"n8n","url":"https://n8n.local/hook?token=s3cret","events":["motion"]}]"#;
    assert_eq!(json(addr, "PUT", "/api/v1/webhooks", hooks).await.0, 200);
    let (status, _) = http(addr, "POST", "/api/v1/backup", r#"{"passphrase":"court"}"#).await;
    assert_eq!(status, 400, "passphrase too short");
    let (status, file) = http(
        addr,
        "POST",
        "/api/v1/backup",
        r#"{"passphrase":"phrase-de-test"}"#,
    )
    .await;
    assert_eq!(status, 200);
    assert!(file.starts_with(b"RUSTNVR-BACKUP1\n"));
    assert!(!file.windows(6).any(|w| w == b"s3cret"), "encrypted");
    let data = base64::engine::general_purpose::STANDARD.encode(&file);

    // Since then, the webhook is removed.
    assert_eq!(json(addr, "PUT", "/api/v1/webhooks", "[]").await.0, 200);

    let body = |pass: &str| format!(r#"{{"data":"{data}","passphrase":"{pass}"}}"#);
    let (status, r) = json(
        addr,
        "POST",
        "/api/v1/backup/inspect",
        &body("mauvaise-phrase"),
    )
    .await;
    assert_eq!(status, 400, "{r}");
    let (status, p) = json(
        addr,
        "POST",
        "/api/v1/backup/inspect",
        &body("phrase-de-test"),
    )
    .await;
    assert_eq!(status, 200, "{p}");
    let state = |name: &str| {
        p["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .map(|f| f["state"].as_str().unwrap().to_owned())
    };
    assert_eq!(state("config.toml").as_deref(), Some("same"));
    assert_eq!(state("webhooks.ui.toml").as_deref(), Some("changed"));
    assert_eq!(p["cameras"][0]["name"], "Jardin");
    assert_eq!(p["problems"].as_array().map(Vec::len), Some(0));

    // Restore the webhooks only.
    let restore = format!(
        r#"{{"data":"{data}","passphrase":"phrase-de-test","files":["webhooks.ui.toml"],"motion":false}}"#
    );
    let (status, r) = json(addr, "POST", "/api/v1/backup/restore", &restore).await;
    assert_eq!(status, 200, "{r}");
    assert_eq!(r["files"][0], "webhooks.ui.toml");
    assert_eq!(r["restart_needed"], true);
    let restored = std::fs::read_to_string(dir.path().join("webhooks.ui.toml")).unwrap();
    assert!(restored.contains("n8n"), "{restored}");
    let (_, system) = json(addr, "GET", "/api/v1/system", "").await;
    assert_eq!(system["config_restored"], true);
    // Restoring nothing is refused.
    let nothing = format!(r#"{{"data":"{data}","passphrase":"phrase-de-test"}}"#);
    assert_eq!(
        json(addr, "POST", "/api/v1/backup/restore", &nothing)
            .await
            .0,
        400
    );
    nvr.shutdown().await;
}
