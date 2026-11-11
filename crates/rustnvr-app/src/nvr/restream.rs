//! RTSP republication (`[restream]`): the server of `rustnvr-restream`,
//! started and stopped with the general settings. It serves the live frames
//! of the running cameras, so the cameras see one connection per stream
//! whatever the number of RTSP clients.

use super::Shared;
use rustnvr_core::CameraId;
use rustnvr_core::config::RestreamConfig;
use rustnvr_restream::{Lookup, RestreamServer};
use std::sync::{Arc, Weak};
use tracing::{info, warn};

/// The live frames of a running camera (`sub`: its sub stream).
fn lookup(shared: &Arc<Shared>) -> Lookup {
    // Weak: the server must not keep the NVR alive.
    let shared: Weak<Shared> = Arc::downgrade(shared);
    Arc::new(move |camera: String, sub: bool| {
        let shared = shared.clone();
        Box::pin(async move {
            let shared = shared.upgrade()?;
            let id = CameraId::new(camera).ok()?;
            let state = shared.state.lock().await;
            let runtime = state.running.get(&id)?;
            if sub {
                runtime.sub.as_ref().map(|(_, tap)| tap.clone())
            } else {
                Some(runtime.live.clone())
            }
        })
    })
}

async fn start(shared: &Arc<Shared>, config: &RestreamConfig) -> Result<RestreamServer, String> {
    let addr = config.listen_addr()?;
    let server = RestreamServer::start(addr, lookup(shared), config.max_clients as usize)
        .await
        .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
    info!(
        url = %format!("rtsp://{}/<camera>", server.local_addr()),
        max_clients = config.max_clients,
        "RTSP republication listening"
    );
    if !addr.ip().is_loopback() {
        warn!(listen = %addr, "RTSP republication reachable from the network without authentication: anyone who can reach this address can watch the cameras");
    }
    Ok(server)
}

/// (Re)starts the republication with the current settings.
pub(super) async fn restart(shared: &Arc<Shared>) {
    let config = shared.general().restream;
    let mut running = shared.restream.lock().await;
    if let Some(old) = running.take() {
        old.shutdown().await;
        info!("RTSP republication stopped");
    }
    let mut error = None;
    if config.enabled {
        match start(shared, &config).await {
            Ok(server) => *running = Some(server),
            Err(e) => {
                warn!(error = %e, "RTSP republication not started");
                error = Some(e);
            }
        }
    }
    *shared
        .restream_error
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = error;
    publish(shared, running.as_ref());
}

/// Tells the API where the cameras are republished.
fn publish(shared: &Shared, server: Option<&RestreamServer>) {
    if let Some(api) = shared.api.get() {
        api.set_restream(server.map(RestreamServer::local_addr));
    }
}

/// Stops the republication (shutdown).
pub(super) async fn stop(shared: &Shared) {
    if let Some(server) = shared.restream.lock().await.take() {
        server.shutdown().await;
    }
    publish(shared, None);
}

/// Address listened on, while the republication is served.
pub(super) async fn addr(shared: &Shared) -> Option<std::net::SocketAddr> {
    shared
        .restream
        .lock()
        .await
        .as_ref()
        .map(RestreamServer::local_addr)
}
