//! HTTP server of RustNVR: REST API `/api/v1`, WebSocket `/api/v1/ws`,
//! recorded segments, and the static web interface.
//!
//! The API reports only what the backend really does; `GET /api/v1/capabilities`
//! tells the interface which features exist. See `API_CONTRACT.md`.

pub mod dto;
mod gpu;
pub mod hwaccel;
pub mod sysinfo;

pub mod auth;
mod backup;
mod bookmarks;
pub mod control;
mod detection;
mod devices;
mod discovery;
mod export;
mod ingress;
mod live;
pub mod logs;
mod playback;
mod preview;
mod protect;
mod ptz;
mod recordings;
mod replay;
mod routes;
mod state;
mod stats;
pub mod storage;
pub mod tls;
pub mod transcode;
mod ws;

pub use control::{
    AiModelView, AiProbeInput, AiProbeResult, AiTestObject, AiTestResult, AiView, BackupCamera,
    BackupFile, BackupPreview, BackupRestored, CameraInput, CaptureDay, CapturePicture,
    ControlError, DestinationInput, MqttInput, MqttView, NvrControl, ReplayPicture, ReplayResult,
    RestreamInput, RestreamView, SettingsInput, SettingsView, StreamInput, WebhookTestResult,
};
pub use routes::router;
pub use state::{ApiState, CameraEditInfo, CameraSource, DestinationInfo, StreamSource};
pub use transcode::LiveSettings;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// A running HTTP server.
pub struct ApiServer {
    local_addr: SocketAddr,
    state: Arc<ApiState>,
    task: JoinHandle<std::io::Result<()>>,
}

impl ApiServer {
    /// Binds `addr` (bind errors are returned immediately) and starts serving.
    pub async fn start(
        addr: SocketAddr,
        state: Arc<ApiState>,
        static_dir: PathBuf,
    ) -> std::io::Result<Self> {
        Self::start_with_tls(addr, state, static_dir, None).await
    }

    /// Like [`Self::start`], over HTTPS when `tls` is given.
    pub async fn start_with_tls(
        addr: SocketAddr,
        state: Arc<ApiState>,
        static_dir: PathBuf,
        tls: Option<Arc<rustls::ServerConfig>>,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        tokio::spawn(state::run_sampler(state.clone()));
        state
            .https
            .store(tls.is_some(), std::sync::atomic::Ordering::Relaxed);
        let app = router(state.clone(), &static_dir);
        let shutdown = state.shutdown.clone();
        let task = tokio::spawn(async move {
            match tls {
                Some(config) => tls::serve(listener, app, config, shutdown).await,
                None => {
                    axum::serve(
                        listener,
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(shutdown.cancelled_owned())
                    .await
                }
            }
        });
        Ok(Self {
            local_addr,
            state,
            task,
        })
    }

    /// Address actually bound (useful with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Closes WebSockets, stops accepting requests and waits for in-flight ones.
    pub async fn shutdown(self) {
        self.state.shutdown.cancel();
        match self.task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "HTTP server error"),
            Err(e) => tracing::warn!(error = %e, "HTTP server task failed"),
        }
    }
}
