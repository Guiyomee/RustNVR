//! HTTP-level tests of the router (no network: requests go through `oneshot`).
#![allow(clippy::unwrap_used, missing_docs)]

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use bytes::Bytes;
use http_body_util::BodyExt as _;
use rustnvr_api::{ApiState, CameraSource, LiveSettings, StreamSource, router};
use rustnvr_core::events::event_bus;
use rustnvr_core::{CameraId, Secret, StreamState, VideoCodec, VideoParams};
use rustnvr_recorder::fmp4::{self, SampleMeta};
use rustnvr_stream::StreamMetrics;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::watch;
use tower::ServiceExt as _;

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    web: std::path::PathBuf,
    _state_tx: watch::Sender<StreamState>,
}

fn write_segment(root: &Path) {
    let dir = root.join("cam").join("2026-11-01");
    std::fs::create_dir_all(&dir).unwrap();
    let mut entry = vec![0, 0, 0, 16];
    entry.extend_from_slice(b"avc1");
    entry.extend_from_slice(&[0; 8]);
    let params = VideoParams {
        codec: VideoCodec::H264,
        rfc6381_codec: "avc1.42C01E".into(),
        width: 320,
        height: 240,
        clock_rate: 90_000,
        sample_entry: Bytes::from(entry),
    };
    let mut data = fmp4::init_segment(&params);
    for i in 0..2u32 {
        let metas: Vec<_> = (0..10)
            .map(|k| SampleMeta {
                duration: 9000,
                size: 100,
                is_key: k == 0,
            })
            .collect();
        data.extend(fmp4::fragment_header(i + 1, u64::from(i) * 90_000, &metas));
        data.extend(std::iter::repeat_n(0u8, 1000));
    }
    std::fs::write(dir.join("20261101T120000.000Z.mp4"), data).unwrap();
    // Ignored: in-progress segment and unrelated file.
    std::fs::write(dir.join("20261101T120002.000Z.mp4.part"), b"x").unwrap();
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
}

fn app(auth: Option<(&str, &str)>, with_frontend: bool) -> (axum::Router, Fixture) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("rec");
    let web = dir.path().join("web");
    write_segment(&root);
    if with_frontend {
        std::fs::create_dir_all(web.join("assets")).unwrap();
        std::fs::write(web.join("index.html"), "<html>RustNVR UI</html>").unwrap();
        std::fs::write(web.join("assets").join("app.js"), "console.log(1)").unwrap();
    }
    let (state_tx, state_rx) = watch::channel(StreamState::Streaming {
        session: 1,
        codec: VideoCodec::H265,
        width: 1920,
        height: 1080,
        audio: Some("mp4a.40.2 16000 Hz".into()),
        since: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_557_353),
    });
    let cameras = vec![
        CameraSource {
            id: CameraId::new("cam").unwrap(),
            name: Some("Entrée".into()),
            enabled: true,
            recording_enabled: true,
            has_sub_stream: false,
            endpoint: "rtsp://10.0.0.2:554".into(),
            stream: Some(StreamSource {
                state: state_rx,
                metrics: Arc::new(StreamMetrics::default()),
            }),
            recorders: Vec::new(),
            record_destinations: Vec::new(),
            live: None,
            sub_stream: None,
            live_sub: None,
            edit: None,
            motion_enabled: false,
            record_audio: true,
            privacy_masks: Vec::new(),
            ptz: false,
            record_mode: Default::default(),
            retention: Default::default(),
            motion: None,
        },
        CameraSource {
            id: CameraId::new("off").unwrap(),
            name: None,
            enabled: false,
            recording_enabled: true,
            has_sub_stream: true,
            endpoint: "rtsp://10.0.0.3:554".into(),
            stream: None,
            recorders: Vec::new(),
            record_destinations: Vec::new(),
            live: None,
            sub_stream: None,
            live_sub: None,
            edit: None,
            motion_enabled: false,
            record_audio: true,
            privacy_masks: Vec::new(),
            ptz: false,
            record_mode: Default::default(),
            retention: Default::default(),
            motion: None,
        },
    ];
    let auth = auth.map(|(u, p)| (u.to_owned(), Secret::new(p)));
    let state = ApiState::new(
        cameras,
        root.clone(),
        event_bus(16),
        auth,
        LiveSettings::default(),
    );
    let router = router(state, &web);
    (
        router,
        Fixture {
            _dir: dir,
            root,
            web,
            _state_tx: state_tx,
        },
    )
}

async fn get(
    app: &axum::Router,
    uri: &str,
    headers: &[(&str, String)],
) -> (StatusCode, axum::http::HeaderMap, Bytes) {
    let mut req = Request::builder().uri(uri);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body)
}

fn json(body: &Bytes) -> serde_json::Value {
    serde_json::from_slice(body).unwrap()
}

fn basic(user: &str, pass: &str) -> (&'static str, String) {
    (
        "authorization",
        format!("Basic {}", B64.encode(format!("{user}:{pass}"))),
    )
}

#[tokio::test]
async fn requires_authentication_when_configured() {
    let (app, _f) = app(Some(("admin", "s3cret")), true);
    for uri in [
        "/api/v1/cameras",
        "/",
        "/api/v1/recordings/local/cam/2026-11-01/20261101T120000.000Z.mp4",
    ] {
        let (status, headers, _) = get(&app, uri, &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        assert!(
            headers[header::WWW_AUTHENTICATE]
                .to_str()
                .unwrap()
                .starts_with("Basic")
        );
    }
    let (status, _, _) = get(&app, "/api/v1/cameras", &[basic("admin", "wrong")]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, body) = get(&app, "/api/v1/system", &[basic("admin", "s3cret")]).await;
    assert_eq!(status, StatusCode::OK);
    let system = json(&body);
    assert_eq!(system["auth_enabled"], true);
    assert_eq!(system["user"], "admin");
    assert_eq!(system["cameras_total"], 2);
    assert_eq!(system["cameras_online"], 1);
}

#[tokio::test]
async fn lists_cameras_without_secrets() {
    let (app, _f) = app(None, true);
    let (status, headers, body) = get(&app, "/api/v1/cameras", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let cams = json(&body);
    assert_eq!(cams[0]["id"], "cam");
    assert_eq!(cams[0]["name"], "Entrée");
    assert_eq!(cams[0]["status"], "online");
    assert_eq!(cams[0]["codec"], "h265");
    assert_eq!(cams[0]["width"], 1920);
    assert_eq!(cams[0]["online_since_ms"], 1_793_557_353_000i64);
    assert_eq!(cams[1]["status"], "disabled");
    assert_eq!(cams[1]["name"], "off");
    assert_eq!(cams[1]["endpoint"], "rtsp://10.0.0.3:554");

    let (status, _, body) = get(&app, "/api/v1/cameras/nope", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json(&body)["error"]["code"], "not_found");
}

#[tokio::test]
async fn lists_and_serves_recordings_with_ranges() {
    let (app, _f) = app(None, false);
    // 2026-11-01T12:00:00Z = 1793534400000.
    let uri = "/api/v1/cameras/cam/recordings?from=1793534000000&to=1793535000000";
    let (status, _, body) = get(&app, uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let list = json(&body);
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["start_ms"], 1_793_534_400_000i64);
    assert_eq!(list[0]["duration_ms"], 2000);
    assert_eq!(list[0]["end_ms"], 1_793_534_402_000i64);
    let url = list[0]["url"].as_str().unwrap().to_owned();
    assert_eq!(
        url,
        "/api/v1/recordings/local/cam/2026-11-01/20261101T120000.000Z.mp4"
    );

    // Outside the range: nothing.
    let (_, _, body) = get(
        &app,
        "/api/v1/cameras/cam/recordings?from=1793534402000&to=1793535000000",
        &[],
    )
    .await;
    assert_eq!(json(&body).as_array().unwrap().len(), 0);

    let (status, headers, body) = get(&app, &url, &[("range", "bytes=0-99".into())]).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(headers[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(body.len(), 100);
    assert_eq!(&body[4..8], b"ftyp");

    for (uri, expected) in [
        (
            "/api/v1/cameras/cam/recordings?from=10&to=5",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/cameras/cam/recordings?from=0&to=999999999999",
            StatusCode::BAD_REQUEST,
        ),
        ("/api/v1/cameras/cam/recordings", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(get(&app, uri, &[]).await.0, expected, "{uri}");
    }
}

#[tokio::test]
async fn rejects_paths_outside_recordings() {
    let (app, f) = app(None, false);
    std::fs::write(f.root.join("secret.mp4"), b"secret").unwrap();
    for uri in [
        "/api/v1/recordings/local/cam/..%2F..%2Fsecret/20261101T120000.000Z.mp4",
        "/api/v1/recordings/local/cam/2026-11-01/..%2F..%2Fsecret.mp4",
        "/api/v1/recordings/local/..%2Fcam/2026-11-01/20261101T120000.000Z.mp4",
        "/api/v1/recordings/local/cam/2026-11-01/20261101T120002.000Z.mp4.part",
    ] {
        let (status, _, body) = get(&app, uri, &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert!(!body.starts_with(b"secret"));
    }
    let (status, _, _) = get(
        &app,
        "/api/v1/recordings/local/cam/2026-11-02/20261102T120000.000Z.mp4",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn serves_interface_with_client_side_routes() {
    let (app, f) = app(None, true);
    assert!(f.web.join("index.html").is_file());
    let (status, headers, body) = get(&app, "/cameras/cam", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "<html>RustNVR UI</html>");
    // index.html is revalidated: a rebuilt interface is used at once.
    assert_eq!(headers["cache-control"], "no-cache");
    let (status, headers, body) = get(&app, "/assets/app.js", &[]).await;
    assert_eq!(
        (status, &body[..]),
        (StatusCode::OK, &b"console.log(1)"[..])
    );
    assert_eq!(
        headers["cache-control"],
        "public, max-age=31536000, immutable"
    );
    // A missing built file is a 404, not index.html.
    let (status, headers, _) = get(&app, "/assets/missing.js", &[]).await;
    assert_eq!(
        (status, &headers["cache-control"]),
        (StatusCode::NOT_FOUND, &HeaderValue::from_static("no-cache"))
    );
    // Unknown API routes are JSON 404s, not the interface.
    let (status, _, body) = get(&app, "/api/v1/no-such-route", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json(&body)["error"]["code"], "not_found");
}

#[tokio::test]
async fn serves_interface_under_home_assistant_ingress() {
    let (app, f) = app(None, true);
    std::fs::write(
        f.web.join("index.html"),
        "<html><head><link rel=\"icon\" href=\"./favicon.svg\" />\
         <script type=\"module\" src=\"./assets/app.js\"></script></head></html>",
    )
    .unwrap();
    // Without ingress: absolute addresses, frames refused.
    for path in ["/", "/index.html", "/cameras/cam"] {
        let (status, headers, body) = get(&app, path, &[]).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let page = String::from_utf8_lossy(&body);
        assert!(page.contains("src=\"/assets/app.js\""), "{path}: {page}");
        assert!(!page.contains("rustnvr-base"));
        assert_eq!(headers["x-frame-options"], "DENY");
        assert!(
            headers["content-security-policy"]
                .to_str()
                .unwrap()
                .ends_with("frame-ancestors 'none'")
        );
        assert_eq!(headers["vary"], "x-ingress-path");
    }
    // Under ingress: the prefix in the page, frames of the same origin allowed.
    let ingress = [("x-ingress-path", "/api/hassio_ingress/abc".to_owned())];
    for path in ["/", "/cameras/cam"] {
        let (status, headers, body) = get(&app, path, &ingress).await;
        assert_eq!(status, StatusCode::OK);
        let page = String::from_utf8_lossy(&body);
        assert!(
            page.contains("src=\"/api/hassio_ingress/abc/assets/app.js\""),
            "{page}"
        );
        assert!(page.contains("href=\"/api/hassio_ingress/abc/favicon.svg\""));
        assert!(
            page.contains("<meta name=\"rustnvr-base\" content=\"/api/hassio_ingress/abc\" />")
        );
        assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
        assert!(
            headers["content-security-policy"]
                .to_str()
                .unwrap()
                .ends_with("frame-ancestors 'self'")
        );
        assert_eq!(headers["cache-control"], "no-cache");
    }
    let (_, headers, _) = get(&app, "/api/v1/health", &ingress).await;
    assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
    // A value that is not a plain path is ignored.
    let bad = [("x-ingress-path", "/x\"><script>".to_owned())];
    let (_, headers, body) = get(&app, "/", &bad).await;
    let page = String::from_utf8_lossy(&body);
    assert!(page.contains("src=\"/assets/app.js\"") && !page.contains("script>\""));
    assert_eq!(headers["x-frame-options"], "DENY");
}

#[tokio::test]
async fn explains_missing_frontend() {
    let (app, _f) = app(None, false);
    let (status, _, body) = get(&app, "/", &[]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(String::from_utf8_lossy(&body).contains("npm run build"));
}

#[tokio::test]
async fn reports_capabilities_and_storage() {
    let (app, _f) = app(None, false);
    let (_, _, body) = get(&app, "/api/v1/capabilities", &[]).await;
    let caps = json(&body);
    assert_eq!(caps["recordings"], true);
    assert_eq!(caps["live_video"], true);
    assert_eq!(caps["events"], false);
    let (status, _, body) = get(&app, "/api/v1/storage", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let s = json(&body);
    assert_eq!(s["cameras"][0]["segments"], 1);
    assert_eq!(s["days"][0]["day"], "2026-11-01");
    assert_eq!(s["retention_configured"], false);
    #[cfg(unix)]
    assert!(s["disk"]["total_bytes"].as_u64().unwrap() > 0);
}

async fn send_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn camera_management_requires_a_controller() {
    let (app, _f) = app(None, false);
    let (_, _, body) = get(&app, "/api/v1/capabilities", &[]).await;
    assert_eq!(json(&body)["camera_management"], false);
    let (status, body) = send_json(
        &app,
        "POST",
        "/api/v1/cameras",
        r#"{"id":"new","name":"New","main":{"url":"rtsp://10.0.0.9/s"}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "unavailable");
    // A non-JSON body is rejected (no cross-site form submission).
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/cameras")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("id=x"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn probe_rejects_invalid_urls_and_reports_unreachable_cameras() {
    let (app, _f) = app(None, false);
    let (status, body) = send_json(
        &app,
        "POST",
        "/api/v1/cameras/probe",
        r#"{"url":"http://x/"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (status, body) = send_json(
        &app,
        "POST",
        "/api/v1/cameras/probe",
        &format!(r#"{{"url":"rtsp://127.0.0.1:{port}/x","username":"u","password":"secret"}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"]["code"], "probe_failed");
    assert!(!body.to_string().contains("secret"));
}
