//! Optional object recognition ("AI") on event pictures.
//!
//! RustNVR ships no model and recognition is disabled by default. A provider
//! implements [`ObjectDetector`]: the HTTP API of DeepStack / CodeProject.AI
//! Server, run separately, or (`onnx` feature) a YOLOX ONNX model run in
//! process by tract ([`onnx`]).

pub mod models;
#[cfg(feature = "onnx")]
pub mod onnx;

use rustnvr_core::config::{AiConfig, AiProviderConfig};
use rustnvr_core::{DetectedObject, Secret};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// A boxed future, for providers used as trait objects.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Why an analysis failed.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    /// The provider could not be reached or answered with an HTTP error.
    #[error("provider unreachable: {0}")]
    Unreachable(String),
    /// The provider answered but reported a failure or an unreadable reply.
    #[error("provider error: {0}")]
    Provider(String),
    /// No answer within the configured delay.
    #[error("no answer within {0:?}")]
    Timeout(Duration),
}

/// Recognizes objects in a JPEG picture.
pub trait ObjectDetector: Send + Sync {
    /// Short provider name, for logs and the interface.
    fn name(&self) -> &str;
    /// Objects found in `jpeg`, with boxes as fractions of the picture.
    fn detect<'a>(&'a self, jpeg: &'a [u8]) -> BoxFuture<'a, Result<Vec<DetectedObject>, AiError>>;
    /// Where the analyses run (`CPU`, `GPU`…), when known: fixed for the
    /// built-in recognition, as reported by the last answer of a server.
    fn device(&self) -> Option<String> {
        None
    }
}

/// A provider with the filters of the configuration.
#[derive(Clone)]
pub struct Analyzer {
    detector: Arc<dyn ObjectDetector>,
    min_score: f32,
    labels: Vec<String>,
    timeout: Duration,
    max_pictures: u32,
    interval: Duration,
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer")
            .field("provider", &self.detector.name())
            .finish_non_exhaustive()
    }
}

impl Analyzer {
    /// Analyzer for `config`; `None` when no provider is configured.
    pub fn from_config(
        config: &AiConfig,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, String> {
        let detector: Arc<dyn ObjectDetector> = match &config.provider {
            None => return Ok(None),
            Some(AiProviderConfig::DeepstackHttp { url, api_key_env }) => {
                let api_key = match api_key_env {
                    Some(var) => Some(Secret::new(env(var).ok_or_else(|| {
                        format!("ai.provider.api_key_env: environment variable {var:?} is not set")
                    })?)),
                    None => None,
                };
                Arc::new(DeepStackHttp::new(
                    url,
                    api_key,
                    config.min_score,
                    config.timeout(),
                )?)
            }
            #[cfg(feature = "onnx")]
            Some(AiProviderConfig::Onnx { model }) => {
                let started = std::time::Instant::now();
                let detector = onnx::OnnxDetector::load(model)?;
                tracing::info!(
                    model = %model.display(),
                    input = detector.input_size(),
                    ms = started.elapsed().as_millis() as u64,
                    "object recognition model loaded"
                );
                Arc::new(onnx::SharedOnnx(Arc::new(detector)))
            }
            #[cfg(not(feature = "onnx"))]
            Some(AiProviderConfig::Onnx { .. }) => {
                return Err(
                    "ai.provider onnx: this RustNVR was built without the `onnx` feature"
                        .to_owned(),
                );
            }
        };
        Ok(Some(Self {
            max_pictures: config.max_pictures.max(1),
            interval: Duration::from_secs(config.interval_seconds.max(1)),
            ..Self::new(
                detector,
                config.min_score,
                config.labels.clone(),
                config.timeout(),
            )
        }))
    }

    /// Analyzer around any provider.
    pub fn new(
        detector: Arc<dyn ObjectDetector>,
        min_score: f32,
        labels: Vec<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            detector,
            min_score,
            labels,
            timeout,
            max_pictures: 1,
            interval: Duration::from_secs(3),
        }
    }

    /// Pictures to analyse per event at most, and the delay between them.
    pub fn schedule(&self) -> (u32, Duration) {
        (self.max_pictures, self.interval)
    }

    /// Provider name.
    pub fn provider(&self) -> &str {
        self.detector.name()
    }

    /// Where the analyses run (`CPU`, `GPU`…), when known.
    pub fn device(&self) -> Option<String> {
        self.detector.device()
    }

    /// Objects in `jpeg` that pass the filters, most confident first.
    pub async fn analyze(&self, jpeg: &[u8]) -> Result<Vec<DetectedObject>, AiError> {
        Ok(self.filter(self.detect_all(jpeg).await?))
    }

    /// Everything the provider finds in `jpeg`, before the filters (to try
    /// the settings), most confident first.
    pub async fn detect_all(&self, jpeg: &[u8]) -> Result<Vec<DetectedObject>, AiError> {
        let mut found = tokio::time::timeout(self.timeout, self.detector.detect(jpeg))
            .await
            .map_err(|_| AiError::Timeout(self.timeout))??;
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(found)
    }

    /// Whether `o` passes the confidence and label filters.
    pub fn keeps(&self, o: &DetectedObject) -> bool {
        o.score >= self.min_score
            && (self.labels.is_empty() || self.labels.iter().any(|l| l == &o.label))
    }

    fn filter(&self, objects: Vec<DetectedObject>) -> Vec<DetectedObject> {
        let mut kept: Vec<DetectedObject> = objects.into_iter().filter(|o| self.keeps(o)).collect();
        kept.sort_by(|a, b| b.score.total_cmp(&a.score));
        kept
    }
}

/// Client of the DeepStack / CodeProject.AI Server object detection API:
/// `POST <url>/v1/vision/detection`, multipart field `image`.
pub struct DeepStackHttp {
    client: reqwest::Client,
    endpoint: String,
    /// Shown in logs instead of the URL.
    host: String,
    api_key: Option<Secret>,
    min_confidence: f32,
    /// `inferenceDevice` of the last answer (CodeProject.AI; DeepStack does
    /// not tell).
    last_device: std::sync::Mutex<Option<String>>,
}

#[derive(Deserialize)]
struct DeepStackReply {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    predictions: Vec<DeepStackPrediction>,
    #[serde(default)]
    error: Option<String>,
    /// CodeProject.AI: `CPU`, `GPU`, `GPU (CUDA)`…
    #[serde(default, rename = "inferenceDevice")]
    inference_device: Option<String>,
}

#[derive(Deserialize)]
struct DeepStackPrediction {
    label: String,
    confidence: f32,
    #[serde(default)]
    x_min: Option<f32>,
    #[serde(default)]
    y_min: Option<f32>,
    #[serde(default)]
    x_max: Option<f32>,
    #[serde(default)]
    y_max: Option<f32>,
}

impl DeepStackHttp {
    /// Client for the service at `url` (base URL, without the API path).
    pub fn new(
        url: &str,
        api_key: Option<Secret>,
        min_confidence: f32,
        timeout: Duration,
    ) -> Result<Self, String> {
        let parsed = reqwest::Url::parse(url).map_err(|_| "ai.provider.url is not a URL")?;
        let host = match (parsed.host_str(), parsed.port_or_known_default()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            _ => return Err("ai.provider.url has no host".to_owned()),
        };
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(5)))
            .build()
            .map_err(|e| format!("HTTP client: {}", e.without_url()))?;
        Ok(Self {
            client,
            endpoint: format!("{}/v1/vision/detection", url.trim_end_matches('/')),
            host,
            api_key,
            min_confidence,
            last_device: std::sync::Mutex::new(None),
        })
    }

    async fn request(&self, jpeg: &[u8]) -> Result<Vec<DetectedObject>, AiError> {
        // Errors never carry the URL (it may hold a key in its query).
        let http = |e: reqwest::Error| {
            AiError::Unreachable(format!("{} ({})", e.without_url(), self.host))
        };
        let image = reqwest::multipart::Part::bytes(jpeg.to_vec())
            .file_name("event.jpg")
            .mime_str("image/jpeg")
            .map_err(http)?;
        let mut form = reqwest::multipart::Form::new()
            .part("image", image)
            .text("min_confidence", self.min_confidence.to_string());
        if let Some(key) = &self.api_key {
            form = form.text("api_key", key.expose().to_owned());
        }
        let reply: DeepStackReply = self
            .client
            .post(&self.endpoint)
            .multipart(form)
            .send()
            .await
            .map_err(http)?
            .error_for_status()
            .map_err(http)?
            .json()
            .await
            .map_err(|e| AiError::Provider(format!("unreadable reply: {}", e.without_url())))?;
        if !reply.success {
            return Err(AiError::Provider(
                reply.error.unwrap_or_else(|| "success = false".to_owned()),
            ));
        }
        if let Some(device) = reply.inference_device.filter(|d| !d.trim().is_empty()) {
            // A short, plain value only (it is shown in the interface).
            let device: String = device.trim().chars().take(40).collect();
            *self.last_device.lock().unwrap_or_else(|e| e.into_inner()) = Some(device);
        }
        let size = jpeg_size(jpeg);
        Ok(reply
            .predictions
            .into_iter()
            .map(|p| DetectedObject {
                bbox: match (size, p.x_min, p.y_min, p.x_max, p.y_max) {
                    (Some((w, h)), Some(x0), Some(y0), Some(x1), Some(y1)) => {
                        let (w, h) = (w as f32, h as f32);
                        Some([x0 / w, y0 / h, x1 / w, y1 / h].map(|v| v.clamp(0.0, 1.0)))
                    }
                    _ => None,
                },
                label: p.label,
                score: p.confidence,
                zone: None,
            })
            .collect())
    }
}

impl ObjectDetector for DeepStackHttp {
    fn name(&self) -> &str {
        "deepstack_http"
    }

    fn detect<'a>(&'a self, jpeg: &'a [u8]) -> BoxFuture<'a, Result<Vec<DetectedObject>, AiError>> {
        Box::pin(self.request(jpeg))
    }

    fn device(&self) -> Option<String> {
        self.last_device
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Test pattern (320×240) sent by [`probe`].
const PROBE_JPEG: &[u8] = include_bytes!("probe.jpg");

/// What [`probe`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeReport {
    /// Time of the analysis, network included.
    pub ms: u64,
    /// Where it ran (`CPU`, `GPU`…), when the server tells (CodeProject.AI).
    pub device: Option<String>,
    /// Objects the server found on the test pattern (usually none).
    pub objects: usize,
}

/// Checks that a DeepStack / CodeProject.AI server answers at `url` and
/// that its object detection works, by analysing a test pattern. Nothing is
/// stored.
pub async fn probe(
    url: &str,
    api_key: Option<Secret>,
    timeout: Duration,
) -> Result<ProbeReport, AiError> {
    let server = DeepStackHttp::new(url, api_key, 0.5, timeout).map_err(AiError::Provider)?;
    let started = std::time::Instant::now();
    let found = tokio::time::timeout(timeout, server.request(PROBE_JPEG))
        .await
        .map_err(|_| AiError::Timeout(timeout))??;
    Ok(ProbeReport {
        ms: started.elapsed().as_millis() as u64,
        device: server.device(),
        objects: found.len(),
    })
}

/// Width and height of a JPEG picture, from its frame header.
pub fn jpeg_size(jpeg: &[u8]) -> Option<(u32, u32)> {
    if jpeg.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut pos = 2;
    loop {
        if *jpeg.get(pos)? != 0xff {
            return None;
        }
        let marker = *jpeg.get(pos + 1)?;
        if marker == 0xff {
            pos += 1; // fill byte
            continue;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            pos += 2; // no length
            continue;
        }
        let len = usize::from(u16::from_be_bytes([
            *jpeg.get(pos + 2)?,
            *jpeg.get(pos + 3)?,
        ]));
        // Start of frame markers, except DHT (C4), JPG (C8) and DAC (CC).
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let h = u16::from_be_bytes([*jpeg.get(pos + 5)?, *jpeg.get(pos + 6)?]);
            let w = u16::from_be_bytes([*jpeg.get(pos + 7)?, *jpeg.get(pos + 8)?]);
            return Some((u32::from(w), u32::from(h)));
        }
        if marker == 0xda || len < 2 {
            return None; // image data before any frame header
        }
        pos += 2 + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A minimal JPEG header: SOI, an APP0 segment, SOF0 for 320×240.
    fn jpeg() -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00];
        j.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0xf0, 0x01, 0x40]);
        j.extend_from_slice(&[0; 9]);
        j.extend_from_slice(&[0xff, 0xd9]);
        j
    }

    #[test]
    fn reads_jpeg_size() {
        assert_eq!(jpeg_size(&jpeg()), Some((320, 240)));
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        assert_eq!(jpeg_size(&[0xff, 0xd8, 0xff, 0xda, 0, 2]), None);
    }

    /// Serves one HTTP request with `reply`; returns the request it received.
    async fn one_shot_server(
        status: &'static str,
        reply: &'static str,
    ) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.expect("accept");
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            // Read the headers, then the announced body.
            loop {
                let n = s.read(&mut buf).await.expect("read");
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if req.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            s.write_all(resp.as_bytes()).await.expect("write");
            req
        });
        (url, task)
    }

    #[tokio::test]
    async fn calls_deepstack_api_and_filters() {
        let (url, server) = one_shot_server(
            "200 OK",
            r#"{"success":true,"predictions":[
                {"label":"car","confidence":0.55,"x_min":0,"y_min":0,"x_max":32,"y_max":24},
                {"label":"person","confidence":0.91,"x_min":160,"y_min":120,"x_max":320,"y_max":240},
                {"label":"dog","confidence":0.3,"x_min":0,"y_min":0,"x_max":1,"y_max":1}],
                "inferenceMs":12,"inferenceDevice":"GPU"}"#,
        )
        .await;
        let config = AiConfig {
            provider: Some(AiProviderConfig::DeepstackHttp {
                url: format!("{url}/"),
                api_key_env: Some("KEY".into()),
            }),
            min_score: 0.5,
            labels: vec!["person".into(), "car".into()],
            timeout_seconds: 5,
            max_pictures: 4,
            interval_seconds: 2,
            keep_pictures: false,
        };
        let analyzer = Analyzer::from_config(&config, |_| Some("s3cret".into()))
            .expect("config")
            .expect("enabled");
        assert_eq!(analyzer.provider(), "deepstack_http");
        assert_eq!(analyzer.device(), None, "unknown before the first answer");
        assert_eq!(analyzer.schedule(), (4, Duration::from_secs(2)));
        let found = analyzer.analyze(&jpeg()).await.expect("analyze");
        assert_eq!(
            found.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(),
            ["person", "car"]
        );
        assert_eq!(found[0].bbox, Some([0.5, 0.5, 1.0, 1.0]));
        assert_eq!(analyzer.device().as_deref(), Some("GPU"));
        assert_eq!(found[1].bbox, Some([0.0, 0.0, 0.1, 0.1]));

        let req = String::from_utf8_lossy(&server.await.expect("server")).to_string();
        assert!(
            req.starts_with("POST /v1/vision/detection HTTP/1.1"),
            "{req}"
        );
        assert!(req.contains("name=\"image\"; filename=\"event.jpg\""));
        assert!(req.contains("Content-Type: image/jpeg"));
        assert!(req.contains("name=\"min_confidence\"\r\n\r\n0.5"));
        assert!(req.contains("name=\"api_key\"\r\n\r\ns3cret"));
    }

    #[tokio::test]
    async fn reports_provider_failures() {
        let (url, _server) =
            one_shot_server("200 OK", r#"{"success":false,"error":"model not loaded"}"#).await;
        let d = DeepStackHttp::new(&url, None, 0.4, Duration::from_secs(5)).expect("client");
        let err = d.detect(&jpeg()).await.expect_err("failure");
        assert_eq!(err.to_string(), "provider error: model not loaded");

        let (url, _server) = one_shot_server("500 Internal Server Error", "{}").await;
        let d = DeepStackHttp::new(&url, None, 0.4, Duration::from_secs(5)).expect("client");
        let err = d.detect(&jpeg()).await.expect_err("http error");
        assert!(matches!(err, AiError::Unreachable(_)), "{err}");
        assert!(!err.to_string().contains("http://"), "{err}");
    }

    #[tokio::test]
    async fn probes_a_server_with_the_test_pattern() {
        assert_eq!(jpeg_size(PROBE_JPEG), Some((320, 240)));
        let (url, server) = one_shot_server(
            "200 OK",
            r#"{"success":true,"predictions":[],"inferenceDevice":"CPU","inferenceMs":40}"#,
        )
        .await;
        let report = probe(&url, None, Duration::from_secs(5))
            .await
            .expect("probe");
        assert_eq!((report.device.as_deref(), report.objects), (Some("CPU"), 0));
        let request = server.await.expect("server");
        let text = String::from_utf8_lossy(&request);
        assert!(text.starts_with("POST /v1/vision/detection "), "{text}");
        assert!(text.contains("name=\"image\""));

        // No server there.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = closed.local_addr().expect("addr");
        drop(closed);
        let err = probe(&format!("http://{addr}"), None, Duration::from_secs(5))
            .await
            .expect_err("nothing listening");
        assert!(matches!(err, AiError::Unreachable(_)), "{err}");
        assert!(
            probe("pas une adresse", None, Duration::from_secs(1))
                .await
                .is_err()
        );
    }

    #[test]
    fn disabled_without_provider() {
        assert!(
            Analyzer::from_config(&AiConfig::default(), |_| None)
                .expect("ok")
                .is_none()
        );
    }
}
