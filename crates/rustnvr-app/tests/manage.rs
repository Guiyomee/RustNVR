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
