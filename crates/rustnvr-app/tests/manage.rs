//! Camera management through the HTTP API with the real controller:
//! add → connects and records → edit (password kept) → test → delete.
#![allow(clippy::unwrap_used, missing_docs)]

use rustnvr_app::Nvr;
use rustnvr_core::NvrEvent;
use rustnvr_core::config::{Config, MANAGED_CAMERAS_FILE};
use rustnvr_core::events::event_bus;
use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Minimal HTTP/1.1 client: returns the status and the JSON body (Null if empty).
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_edit_test_and_delete_a_camera_from_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.frame_interval = Duration::from_millis(5);
    let camera = MockRtspServer::start(cfg).await.unwrap();
    let url = camera.url();
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
            id = "fixe"
            main = {{ url = "{url}" }}
            "#,
            rec = dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let events = event_bus(1024);
    let mut rx = events.subscribe();
    let nvr = Nvr::start(&config, events).await.unwrap();
    let addr = nvr.web_addr().unwrap();
    let managed_file = dir.path().join(MANAGED_CAMERAS_FILE);

    let (_, caps) = http(addr, "GET", "/api/v1/capabilities", None).await;
    assert_eq!(caps["camera_management"], true);

    // Add.
    let new = format!(
        r#"{{"id":"nouvelle","name":"Nouvelle","main":{{"url":"{url}","username":"u","password":"s3cret"}}}}"#
    );
    let (status, cam) = http(addr, "POST", "/api/v1/cameras", Some(&new)).await;
    assert_eq!(status, 201, "{cam}");
    assert_eq!(cam["managed"], true);
    assert_eq!(cam["edit"]["main_url"], url.as_str());
    assert_eq!(cam["edit"]["has_password"], true);
    assert!(
        !cam.to_string().contains("s3cret"),
        "password leaked: {cam}"
    );
    let stored = std::fs::read_to_string(&managed_file).unwrap();
    assert!(stored.contains("s3cret") && stored.contains("nouvelle"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&managed_file)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let (status, _) = http(addr, "POST", "/api/v1/cameras", Some(&new)).await;
    assert_eq!(status, 409, "duplicate id");

    // It connects and records like any other camera.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(NvrEvent::SegmentFinished(s)) = rx.recv().await
                && s.camera.as_str() == "nouvelle"
            {
                return;
            }
        }
    })
    .await
    .expect("the new camera records");
    let (_, cam) = http(addr, "GET", "/api/v1/cameras/nouvelle", None).await;
    assert_eq!(cam["status"], "online");

    // Edit without sending the password: it is kept.
    let edit =
        format!(r#"{{"name":"Renommée","record":false,"main":{{"url":"{url}","username":"u"}}}}"#);
    let (status, cam) = http(addr, "PUT", "/api/v1/cameras/nouvelle", Some(&edit)).await;
    assert_eq!(status, 200, "{cam}");
    assert_eq!(cam["name"], "Renommée");
    assert_eq!(cam["recording_enabled"], false);
    assert!(
        std::fs::read_to_string(&managed_file)
            .unwrap()
            .contains("s3cret")
    );

    // Cameras of config.toml are read-only.
    let (status, err) = http(addr, "PUT", "/api/v1/cameras/fixe", Some(&edit)).await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (409, Some("read_only"))
    );
    let (status, _) = http(addr, "DELETE", "/api/v1/cameras/fixe", None).await;
    assert_eq!(status, 409);

    // Connection test of a configured camera.
    let (status, report) = http(addr, "POST", "/api/v1/cameras/nouvelle/probe", None).await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["video"]["codec"], "h264");

    // Delete.
    let (status, _) = http(addr, "DELETE", "/api/v1/cameras/nouvelle", None).await;
    assert_eq!(status, 204);
    let (status, _) = http(addr, "GET", "/api/v1/cameras/nouvelle", None).await;
    assert_eq!(status, 404);
    assert!(
        !std::fs::read_to_string(&managed_file)
            .unwrap()
            .contains("nouvelle")
    );

    // The stored file reloads cleanly.
    let reloaded = Config::load_with_env(&config_path, |_| None).unwrap();
    assert_eq!(reloaded.cameras.len(), 1);

    // --- Storage destinations -------------------------------------------
    let share = dir.path().join("share");
    std::fs::create_dir(&share).unwrap();
    // A plain directory of the system disk is not a mounted share. Checked
    // only where the build directory is on the same filesystem as `/` (on
    // some systems /tmp or the build directory are separate filesystems).
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let on_root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("not-a-share");
        std::fs::create_dir_all(&on_root).unwrap();
        if std::fs::metadata(&on_root).unwrap().dev() == std::fs::metadata("/").unwrap().dev() {
            let refused = format!(
                r#"{{"id":"nas","name":"NAS","path":"{}","require_mount":true}}"#,
                on_root.display()
            );
            let (status, err) =
                http(addr, "POST", "/api/v1/storage/destinations", Some(&refused)).await;
            assert_eq!(status, 400, "{err}");
            assert!(
                err["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("pas monté"),
                "{err}"
            );
        } else {
            eprintln!("build directory not on the root filesystem: mount check not exercised");
        }
    }
    // Accepted as a second local disk.
    let dest = format!(
        r#"{{"id":"nas","name":"NAS","path":"{}","require_mount":false,"retention_days":90}}"#,
        share.display()
    );
    let (status, list) = http(addr, "POST", "/api/v1/storage/destinations", Some(&dest)).await;
    assert_eq!(status, 201, "{list}");
    assert_eq!(list[1]["id"], "nas");
    assert_eq!(list[1]["available"], true);
    assert_eq!(list[1]["retention_days"], 90);

    // A camera recording only to that destination writes there.
    let cam = format!(
        r#"{{"id":"distante","name":"Distante","record":true,"record_destinations":["nas"],"retention_days":7,"max_size_gb":3,"main":{{"url":"{url}"}}}}"#
    );
    let (status, body) = http(addr, "POST", "/api/v1/cameras", Some(&cam)).await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["record_destinations"], serde_json::json!(["nas"]));
    assert_eq!(
        (
            body["retention_days"].as_u64(),
            body["max_size_gb"].as_u64(),
            body["record_mode"].as_str()
        ),
        (Some(7), Some(3), Some("continuous"))
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(NvrEvent::SegmentFinished(s)) = rx.recv().await
                && s.camera.as_str() == "distante"
            {
                assert!(s.path.starts_with(&share), "{}", s.path.display());
                return;
            }
        }
    })
    .await
    .expect("records to the destination");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let (_, recs) = http(
        addr,
        "GET",
        &format!(
            "/api/v1/cameras/distante/recordings?from={}&to={}",
            now - 600_000,
            now + 1000
        ),
        None,
    )
    .await;
    let url = recs[0]["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("/api/v1/recordings/nas/distante/"), "{url}");

    // In use: cannot be removed.
    let (status, err) = http(addr, "DELETE", "/api/v1/storage/destinations/nas", None).await;
    assert_eq!(status, 409, "{err}");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Distante")
    );

    // Local retention, stored in storage.ui.toml.
    let (status, list) = http(
        addr,
        "PUT",
        "/api/v1/storage/destinations/local",
        Some(r#"{"retention_days":30,"max_size_gb":500}"#),
    )
    .await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        (
            list[0]["retention_days"].as_u64(),
            list[0]["max_size_gb"].as_u64()
        ),
        (Some(30), Some(500))
    );
    let reloaded = Config::load_with_env(&config_path, |_| None).unwrap();
    assert_eq!(reloaded.storage.local_retention().retention_days, Some(30));
    assert_eq!(reloaded.storage.destinations[0].id, "nas");
    assert_eq!(
        reloaded
            .cameras
            .iter()
            .find(|c| c.id.as_str() == "distante")
            .unwrap()
            .record
            .destinations,
        ["nas"]
    );
    let kept = &reloaded
        .cameras
        .iter()
        .find(|c| c.id.as_str() == "distante")
        .unwrap()
        .record;
    assert_eq!((kept.retention_days, kept.max_size_gb), (Some(7), Some(3)));

    // Once unused, it can be removed.
    // Default destination: used when recording is enabled without a choice.
    let (_, s) = http(addr, "GET", "/api/v1/settings", None).await;
    assert_eq!(s["default_destination"], "local");
    let (_, storage) = http(addr, "GET", "/api/v1/storage", None).await;
    assert_eq!(storage["default_destination"], "local");
    let mut input = serde_json::json!({
        "recording": s["recording"],
        "motion": s["motion"],
        "live_transcode": s["live_transcode"],
        "live_max_bitrate_kbps": s["live_max_bitrate_kbps"],
        "low_space_percent": s["low_space_percent"],
        "default_destination": "inconnu",
    });
    let (status, _) = http(addr, "PUT", "/api/v1/settings", Some(&input.to_string())).await;
    assert_eq!(status, 400);
    input["default_destination"] = "nas".into();
    let (status, s) = http(addr, "PUT", "/api/v1/settings", Some(&input.to_string())).await;
    assert_eq!(
        (status, s["default_destination"].as_str()),
        (200, Some("nas"))
    );
    // Shown at once by the storage figures, although they are cached.
    let (_, storage) = http(addr, "GET", "/api/v1/storage", None).await;
    assert_eq!(storage["default_destination"], "nas");
    let nas = storage["destinations"]
        .as_array()
        .and_then(|d| d.iter().find(|d| d["id"] == "nas"))
        .expect("nas destination");
    assert!(nas.get("estimated_days_until_full").is_some());
    let cam = r#"{"id":"defaut","name":"Défaut","enabled":false,"record":true,"record_destinations":[],"main":{"url":"rtsp://127.0.0.1:9/defaut"}}"#;
    let (status, body) = http(addr, "POST", "/api/v1/cameras", Some(cam)).await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["record_destinations"], serde_json::json!(["nas"]));
    assert_eq!(
        http(addr, "DELETE", "/api/v1/cameras/defaut", None).await.0,
        204
    );

    assert_eq!(
        http(addr, "DELETE", "/api/v1/cameras/distante", None)
            .await
            .0,
        204
    );
    let (status, list) = http(addr, "DELETE", "/api/v1/storage/destinations/nas", None).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(nvr.shutdown().await);
}

#[tokio::test]
async fn general_settings_are_applied_and_kept() {
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
            "#,
            rec = dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let addr = nvr.web_addr().unwrap();

    let (status, mut body) = http(addr, "GET", "/api/v1/settings", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["recording"]["segment_seconds"], 60);
    assert_eq!(body["managed"], false);
    assert_eq!(body["live_hwaccel"], "auto");
    body["recording"]["segment_seconds"] = 20.into();
    body["motion"]["fps"] = 3.into();
    body["live_max_bitrate_kbps"] = 1500.into();
    body["low_space_percent"] = 80.into();
    let input = serde_json::json!({
        "recording": body["recording"],
        "motion": body["motion"],
        "live_transcode": body["live_transcode"],
        "live_max_bitrate_kbps": body["live_max_bitrate_kbps"],
        "live_hwaccel": "off",
        "low_space_percent": body["low_space_percent"],
    });
    let (status, saved) = http(addr, "PUT", "/api/v1/settings", Some(&input.to_string())).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(
        (
            saved["recording"]["segment_seconds"].as_u64(),
            saved["managed"].as_bool(),
            saved["live_hwaccel"].as_str()
        ),
        (Some(20), Some(true), Some("off"))
    );
    // Applied: conversions on the processor, said by the system state.
    let mut state = serde_json::Value::Null;
    for _ in 0..50 {
        state = http(addr, "GET", "/api/v1/system", None).await.1;
        if state["hwaccel"]["mode"] == "off" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        (&state["hwaccel"]["mode"], &state["hwaccel"]["state"]),
        (&"off".into(), &"off".into()),
        "{state}"
    );

    // Invalid values are refused with the reason.
    let mut bad = input.clone();
    bad["motion"]["fps"] = 50.into();
    let (status, err) = http(addr, "PUT", "/api/v1/settings", Some(&bad.to_string())).await;
    assert_eq!(status, 400);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("motion.fps"),
        "{err}"
    );

    // Object recognition settings: off by default, a missing model refused,
    // valid settings stored and applied without restart.
    let (status, ai) = http(addr, "GET", "/api/v1/ai", None).await;
    assert_eq!(status, 200, "{ai}");
    assert!(ai["settings"]["provider"].is_null() && ai["active"].is_null());
    let missing = r#"{"provider":{"type":"onnx","model":"/nonexistent/m.onnx"},"min_score":0.5,"labels":["person"],"timeout_seconds":10,"max_pictures":5,"interval_seconds":3}"#;
    let (status, err) = http(addr, "PUT", "/api/v1/ai", Some(missing)).await;
    assert_eq!(status, 400);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ai.provider.model"),
        "{err}"
    );
    let off = r#"{"min_score":0.4,"labels":["person","car"],"timeout_seconds":10,"max_pictures":3,"interval_seconds":2}"#;
    let (status, ai) = http(addr, "PUT", "/api/v1/ai", Some(off)).await;
    assert_eq!(status, 200, "{ai}");
    assert_eq!(
        ai["settings"]["labels"],
        serde_json::json!(["person", "car"])
    );
    assert_eq!(ai["managed"], true);
    let (status, _) = http(addr, "POST", "/api/v1/ai/test?camera=none", None).await;
    assert_eq!(status, 400, "no provider running");

    nvr.shutdown().await;
    let reloaded = Config::load_with_env(&config_path, |_| None).unwrap();
    assert_eq!(reloaded.recording.segment_seconds, 20);
    assert_eq!(reloaded.motion.fps, 3);
    assert_eq!(reloaded.live.max_bitrate_kbps, 1500);
    assert_eq!(reloaded.storage.low_space_percent, 80);
    assert_eq!(reloaded.ai.labels, ["person", "car"]);
    assert_eq!(reloaded.ai.max_pictures, 3);
}

/// State of a camera's streams: (main, sub).
async fn stream_states(addr: SocketAddr, id: &str) -> (String, Option<String>) {
    let (_, cam) = http(addr, "GET", &format!("/api/v1/cameras/{id}"), None).await;
    (
        cam["status"].as_str().unwrap_or_default().to_owned(),
        cam["sub_status"].as_str().map(str::to_owned),
    )
}

/// The form has one user name and one password for both streams, and sends
/// no password when it was not typed again: a sub stream added (or put back)
/// later must use the stored one, as the form says (« unchanged »).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sub_stream_added_later_uses_the_stored_password() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.frame_interval = Duration::from_millis(20);
    cfg.credentials = Some(("u".into(), "s3cret".into()));
    let camera = MockRtspServer::start(cfg).await.unwrap();
    let url = camera.url();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
            [storage]
            recordings_dir = "{rec}"
            [reconnect]
            initial_millis = 50
            max_seconds = 1
            [metrics]
            log_interval_seconds = 0
            [web]
            listen = "127.0.0.1:0"
            static_dir = "/nonexistent-rustnvr-web"
            "#,
            rec = dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(64)).await.unwrap();
    let addr = nvr.web_addr().unwrap();
    let online = |want_sub: bool| async move {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let (main, sub) = stream_states(addr, "cam").await;
                if main == "online" && (!want_sub || sub.as_deref() == Some("online")) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok()
    };
    let put = |body: String| async move {
        let (status, cam) = http(addr, "PUT", "/api/v1/cameras/cam", Some(&body)).await;
        assert_eq!(status, 200, "{cam}");
        cam
    };
    // What the form sends for each stream when the password is not retyped.
    let stream = format!(r#"{{"url":"{url}","username":"u","password":null,"transport":"tcp"}}"#);

    // Main and sub streams, with the password.
    let typed = format!(r#"{{"url":"{url}","username":"u","password":"s3cret"}}"#);
    let new = format!(r#"{{"id":"cam","name":"Cam","record":false,"main":{typed},"sub":{typed}}}"#);
    let (status, cam) = http(addr, "POST", "/api/v1/cameras", Some(&new)).await;
    assert_eq!(status, 201, "{cam}");
    assert!(online(true).await, "both streams connect");

    // The sub stream removed: the main stream comes back.
    let cam = put(format!(
        r#"{{"name":"Cam","record":false,"main":{stream},"sub":null}}"#
    ))
    .await;
    assert_eq!(cam["has_sub_stream"], false);
    assert!(online(false).await, "main stream alone");

    // Put back without typing the password again: it connects.
    let cam = put(format!(
        r#"{{"name":"Cam","record":false,"main":{stream},"sub":{stream}}}"#
    ))
    .await;
    assert_eq!(cam["has_sub_stream"], true);
    assert!(
        online(true).await,
        "the sub stream put back connects: {:?}",
        stream_states(addr, "cam").await
    );

    // Another user name is another account: its password must be typed.
    let other = format!(r#"{{"url":"{url}","username":"autre","password":null}}"#);
    put(format!(
        r#"{{"name":"Cam","record":false,"main":{stream},"sub":null}}"#
    ))
    .await;
    put(format!(
        r#"{{"name":"Cam","record":false,"main":{stream},"sub":{other}}}"#
    ))
    .await;
    let stored = std::fs::read_to_string(dir.path().join(MANAGED_CAMERAS_FILE)).unwrap();
    assert_eq!(stored.matches("s3cret").count(), 1, "{stored}");
    assert!(nvr.shutdown().await);
}

/// A camera paused from the API stops (no connection, no recording) until
/// it is resumed; the pause is never stored, and works for a camera of
/// config.toml too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_camera_is_paused_until_resumed_or_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = MockServerConfig::new(Fixture::h264());
    cfg.frame_interval = Duration::from_millis(5);
    let camera = MockRtspServer::start(cfg).await.unwrap();
    let url = camera.url();
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
            id = "fixe"
            main = {{ url = "{url}" }}
            "#,
            rec = dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let events = event_bus(1024);
    let mut rx = events.subscribe();
    let nvr = Nvr::start(&config, events).await.unwrap();
    let addr = nvr.web_addr().unwrap();
    let records = |mut rx: tokio::sync::broadcast::Receiver<NvrEvent>, wait: u64| async move {
        tokio::time::timeout(Duration::from_secs(wait), async move {
            loop {
                match rx.recv().await {
                    Ok(NvrEvent::SegmentFinished(s)) if s.camera.as_str() == "fixe" => return,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        std::future::pending::<()>().await;
                    }
                    _ => {}
                }
            }
        })
        .await
        .is_ok()
    };
    assert!(
        records(rx.resubscribe(), 15).await,
        "records before the pause"
    );

    let (status, cam) = http(
        addr,
        "PUT",
        "/api/v1/cameras/fixe/pause",
        Some(r#"{"paused":true}"#),
    )
    .await;
    assert_eq!(status, 200, "{cam}");
    assert_eq!(
        (cam["paused"].as_bool(), cam["status"].as_str()),
        (Some(true), Some("paused"))
    );
    // Stopped: no segment any more (a segment lasts 0.25 s at this speed).
    rx = rx.resubscribe();
    assert!(
        !records(rx.resubscribe(), 3).await,
        "nothing recorded while paused"
    );
    assert_eq!(stream_states(addr, "fixe").await.0, "paused");
    // Never stored: config.toml cameras have no file, and none is written.
    assert!(!dir.path().join(MANAGED_CAMERAS_FILE).exists());
    let (status, _) = http(
        addr,
        "PUT",
        "/api/v1/cameras/fixe/pause",
        Some(r#"{"paused":true}"#),
    )
    .await;
    assert_eq!(status, 200, "pausing twice changes nothing");

    let (status, cam) = http(
        addr,
        "PUT",
        "/api/v1/cameras/fixe/pause",
        Some(r#"{"paused":false}"#),
    )
    .await;
    assert_eq!(status, 200, "{cam}");
    assert_eq!(cam["paused"], false);
    assert!(
        records(rx.resubscribe(), 15).await,
        "records again once resumed"
    );
    assert_eq!(stream_states(addr, "fixe").await.0, "online");

    let (status, _) = http(
        addr,
        "PUT",
        "/api/v1/cameras/nulle-part/pause",
        Some(r#"{"paused":true}"#),
    )
    .await;
    assert_eq!(status, 404);
    nvr.shutdown().await;
}

/// A fake ONVIF camera: answers the clock, its record, the capabilities
/// (no PTZ) and a restart; the requests it received.
async fn onvif_camera() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = s.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if req.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&req).into_owned();
                log.lock().unwrap().push(text.clone());
                let inner = if text.contains("GetSystemDateAndTime") {
                    "<tds:GetSystemDateAndTimeResponse><tds:SystemDateAndTime><tt:UTCDateTime><tt:Time><tt:Hour>8</tt:Hour><tt:Minute>0</tt:Minute><tt:Second>0</tt:Second></tt:Time><tt:Date><tt:Year>2030</tt:Year><tt:Month>1</tt:Month><tt:Day>2</tt:Day></tt:Date></tt:UTCDateTime></tds:SystemDateAndTime></tds:GetSystemDateAndTimeResponse>".to_owned()
                } else if text.contains("GetDeviceInformation") {
                    "<tds:GetDeviceInformationResponse><tds:Manufacturer>EZVIZ</tds:Manufacturer><tds:Model>CS-C6N</tds:Model><tds:FirmwareVersion>V5.3.0</tds:FirmwareVersion><tds:SerialNumber>Q12345678</tds:SerialNumber><tds:HardwareId>88</tds:HardwareId></tds:GetDeviceInformationResponse>".to_owned()
                } else if text.contains("SystemReboot") {
                    "<tds:SystemRebootResponse><tds:Message>Rebooting</tds:Message></tds:SystemRebootResponse>".to_owned()
                } else {
                    // GetCapabilities: no PTZ service.
                    "<tds:GetCapabilitiesResponse><tds:Capabilities/></tds:GetCapabilitiesResponse>"
                        .to_owned()
                };
                let body = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?><env:Envelope xmlns:env="http://www.w3.org/2003/05/soap-envelope" xmlns:tds="d" xmlns:tt="s"><env:Body>{inner}</env:Body></env:Envelope>"#
                );
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/soap+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(answer.as_bytes()).await;
            });
        }
    });
    (port, seen)
}

/// A camera with an ONVIF port: its record (maker, model, firmware, serial
/// number) is read and kept, and it can be restarted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn onvif_camera_record_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (onvif_port, seen) = onvif_camera().await;
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
            id = "salon"
            main = {{ url = "rtsp://127.0.0.1:1/stream", username = "admin", password = "pw" }}
            onvif = {{ port = {onvif_port} }}
            [[cameras]]
            id = "fixe"
            main = {{ url = "rtsp://127.0.0.1:1/stream" }}
            "#,
            rec = dir.path().join("rec").display()
        ),
    )
    .unwrap();
    let config = Config::load_with_env(&config_path, |_| None).unwrap();
    let nvr = Nvr::start(&config, event_bus(256)).await.unwrap();
    let addr = nvr.web_addr().unwrap();

    let (status, device) = http(addr, "GET", "/api/v1/cameras/salon/device", None).await;
    assert_eq!(status, 200, "{device}");
    assert_eq!(
        (
            device["manufacturer"].as_str(),
            device["model"].as_str(),
            device["firmware"].as_str(),
            device["serial"].as_str(),
            device["hardware"].as_str()
        ),
        (
            Some("EZVIZ"),
            Some("CS-C6N"),
            Some("V5.3.0"),
            Some("Q12345678"),
            Some("88")
        )
    );
    // Kept: asking again does not call the camera; `refresh=1` does.
    let count = |what: &str| {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|r| r.contains(what))
            .count()
    };
    let read = count("GetDeviceInformation");
    let _ = http(addr, "GET", "/api/v1/cameras/salon/device", None).await;
    assert_eq!(count("GetDeviceInformation"), read);
    let (status, _) = http(addr, "GET", "/api/v1/cameras/salon/device?refresh=1", None).await;
    assert_eq!(status, 200);
    assert_eq!(count("GetDeviceInformation"), read + 1);

    let (status, answer) = http(addr, "POST", "/api/v1/cameras/salon/reboot", Some("{}")).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["message"], "Rebooting");
    assert_eq!(count("<tds:SystemReboot/>"), 1);

    // Without an ONVIF port: a clear refusal.
    let (status, err) = http(addr, "GET", "/api/v1/cameras/fixe/device", None).await;
    assert_eq!(status, 400, "{err}");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("port ONVIF")
    );
    let (status, _) = http(addr, "POST", "/api/v1/cameras/fixe/reboot", Some("{}")).await;
    assert_eq!(status, 400);
    nvr.shutdown().await;
}
