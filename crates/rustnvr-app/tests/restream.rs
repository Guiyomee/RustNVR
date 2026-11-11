//! RTSP republication with the real application: enabled from the general
//! settings, it serves the cameras RustNVR already reads, through one
//! connection to each camera.
#![allow(clippy::unwrap_used, missing_docs)]

use rustnvr_app::Nvr;
use rustnvr_core::RtspTarget;
use rustnvr_core::config::Config;
use rustnvr_core::events::event_bus;
use rustnvr_stream::{RtspSession, SessionOptions};
use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig, ServerStats};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw);
    let status = text.split(' ').nth(1).unwrap().parse().unwrap();
    let json = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .filter(|b| !b.is_empty())
        .map_or(serde_json::Value::Null, |b| {
            serde_json::from_str(b).unwrap()
        });
    (status, json)
}

/// Reads `count` pictures of `rtsp://addr/path`.
async fn watch(addr: SocketAddr, path: &str, count: usize) -> Result<(), String> {
    let target = RtspTarget::parse(&format!("rtsp://{addr}/{path}"), None, None).unwrap();
    let opts = SessionOptions {
        connect_timeout: Duration::from_secs(8),
        stall_timeout: Duration::from_secs(5),
        ..SessionOptions::default()
    };
    let mut session = RtspSession::connect(&target, &opts, 1)
        .await
        .map_err(|e| e.to_string())?;
    for i in 0..count {
        let f = session.next_frame().await.map_err(|e| e.to_string())?;
        assert_eq!((f.params.width, f.params.height), (320, 240));
        assert!(i > 0 || f.is_key);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn republication_is_set_from_the_interface_and_serves_the_cameras() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.frame_interval = Duration::from_millis(40);
    let camera = MockRtspServer::start(cfg).await.unwrap();
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
            main = {{ url = "{url}" }}
            record = {{ enabled = false }}
            "#,
            rec = dir.path().join("rec").display(),
            url = camera.url()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let web = nvr.web_addr().unwrap();

    // Off by default: nothing listens, the interface is told so.
    assert_eq!(nvr.restream_addr().await, None);
    let (_, caps) = http(web, "GET", "/api/v1/capabilities", None).await;
    assert!(caps["restream_port"].is_null(), "{caps}");
    let (status, settings) = http(web, "GET", "/api/v1/settings", None).await;
    assert_eq!(status, 200, "{settings}");
    assert_eq!(
        settings["restream"],
        serde_json::json!({
            "enabled": false,
            "listen": "127.0.0.1:8554",
            "max_clients": 16,
            "allow_unauthenticated_remote": false
        })
    );
    let input = |restream: serde_json::Value| {
        serde_json::json!({
            "recording": settings["recording"],
            "motion": settings["motion"],
            "live_transcode": settings["live_transcode"],
            "live_max_bitrate_kbps": settings["live_max_bitrate_kbps"],
            "low_space_percent": settings["low_space_percent"],
            "restream": restream,
        })
        .to_string()
    };

    // On the network without the explicit agreement: refused, with the reason.
    let remote = serde_json::json!({"enabled": true, "listen": "0.0.0.0:0", "max_clients": 4, "allow_unauthenticated_remote": false});
    let (status, err) = http(web, "PUT", "/api/v1/settings", Some(&input(remote))).await;
    assert_eq!(status, 400, "{err}");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("restream.allow_unauthenticated_remote"),
        "{err}"
    );
    assert_eq!(nvr.restream_addr().await, None);

    // Enabled: served at once, without restarting RustNVR.
    let on = serde_json::json!({"enabled": true, "listen": "127.0.0.1:0", "max_clients": 4, "allow_unauthenticated_remote": false});
    let (status, saved) = http(web, "PUT", "/api/v1/settings", Some(&input(on))).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["restream"]["enabled"], true);
    assert!(saved["restream_error"].is_null(), "{saved}");
    let rtsp = nvr.restream_addr().await.expect("republication started");
    let (_, caps) = http(web, "GET", "/api/v1/capabilities", None).await;
    assert_eq!(caps["restream_port"], rtsp.port());

    tokio::time::timeout(Duration::from_secs(20), async {
        let (a, b) = tokio::join!(watch(rtsp, "jardin", 20), watch(rtsp, "jardin", 20));
        a.unwrap();
        b.unwrap();
    })
    .await
    .expect("clients were not served in time");
    // Two RTSP clients and RustNVR itself: one connection to the camera.
    assert_eq!(ServerStats::get(&camera.stats().connections), 1);
    // No sub stream, no such camera.
    assert!(watch(rtsp, "jardin/sub", 1).await.is_err());
    assert!(watch(rtsp, "garage", 1).await.is_err());

    // A setting sent without `restream` (older page) keeps it.
    let mut without: serde_json::Value =
        serde_json::from_str(&input(serde_json::Value::Null)).unwrap();
    without.as_object_mut().unwrap().remove("restream");
    let (status, kept) = http(web, "PUT", "/api/v1/settings", Some(&without.to_string())).await;
    assert_eq!(status, 200, "{kept}");
    assert_eq!(kept["restream"]["enabled"], true);
    assert_eq!(nvr.restream_addr().await, Some(rtsp), "not restarted");

    // An address already in use: said in the settings, nothing served.
    let busy = serde_json::json!({"enabled": true, "listen": web.to_string(), "max_clients": 4, "allow_unauthenticated_remote": false});
    let (status, failed) = http(web, "PUT", "/api/v1/settings", Some(&input(busy))).await;
    assert_eq!(status, 200, "{failed}");
    assert!(
        failed["restream_error"]
            .as_str()
            .is_some_and(|e| e.contains("cannot listen")),
        "{failed}"
    );
    assert_eq!(nvr.restream_addr().await, None);

    // Disabled again: the port is closed.
    let off = serde_json::json!({"enabled": false, "listen": "127.0.0.1:8554", "max_clients": 4, "allow_unauthenticated_remote": false});
    let (status, saved) = http(web, "PUT", "/api/v1/settings", Some(&input(off))).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(nvr.restream_addr().await, None);
    assert!(tokio::net::TcpStream::connect(rtsp).await.is_err());
    let (_, caps) = http(web, "GET", "/api/v1/capabilities", None).await;
    assert!(caps["restream_port"].is_null(), "{caps}");

    assert!(nvr.shutdown().await);
    // Kept for the next start.
    let reloaded = Config::load_with_env(&config_path, |_| None).unwrap();
    assert!(!reloaded.restream.enabled);
    assert_eq!(reloaded.restream.max_clients, 4);
}

#[tokio::test]
async fn republication_from_the_configuration_file() {
    let dir = tempfile::tempdir().unwrap();
    let text = |restream: &str| {
        format!(
            "[storage]\nrecordings_dir = \"{}\"\n[metrics]\nlog_interval_seconds = 0\n[web]\nenabled = false\n[restream]\n{restream}\n",
            dir.path().join("rec").display()
        )
    };
    // Refused at startup rather than serving the cameras to the network unasked.
    let err =
        Config::from_toml_with_env(&text("enabled = true\nlisten = \"0.0.0.0:8554\""), |_| None)
            .unwrap_err()
            .to_string();
    assert!(
        err.contains("restream.allow_unauthenticated_remote"),
        "{err}"
    );
    let err = Config::from_toml_with_env(&text("listen = \"nvr\"\nmax_clients = 0"), |_| None)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("restream.listen") && err.contains("restream.max_clients"),
        "{err}"
    );

    let config =
        Config::from_toml_with_env(&text("enabled = true\nlisten = \"127.0.0.1:0\""), |_| None)
            .unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let rtsp = nvr.restream_addr().await.expect("republication started");
    let mut c = tokio::net::TcpStream::connect(rtsp).await.unwrap();
    c.write_all(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let n = c.read(&mut buf).await.unwrap();
    assert!(buf[..n].starts_with(b"RTSP/1.0 200 OK"));
    assert!(nvr.shutdown().await);
    assert!(tokio::net::TcpStream::connect(rtsp).await.is_err());
}
