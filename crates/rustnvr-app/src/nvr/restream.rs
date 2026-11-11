//! RTSP republication (`[restream]`): the server of `rustnvr-restream`,
//! started and stopped with the general settings. It serves the live frames
//! of the running cameras, so the cameras see one connection per stream
//! whatever the number of RTSP clients.

use super::Shared;
use rustnvr_core::CameraId;
use rustnvr_core::config::RestreamConfig;
use rustnvr_restream::{Credentials, Lookup, Options, RestreamServer, Source};
use std::sync::{Arc, Weak};
use tracing::{info, warn};

/// A stream of a camera (`sub`: its sub stream), and what clients must
/// present to read it: the credentials of the camera, else the general ones.
fn lookup(shared: &Arc<Shared>, general: Option<Credentials>) -> Lookup {
    // Weak: the server must not keep the NVR alive.
    let shared: Weak<Shared> = Arc::downgrade(shared);
    Arc::new(move |camera: String, sub: bool| {
        let (shared, general) = (shared.clone(), general.clone());
        Box::pin(async move {
            // Unknown cameras are protected like the others, so that only
            // authenticated clients learn which cameras exist.
            let unknown = Source {
                tap: None,
                credentials: general.clone(),
            };
            let (Some(shared), Ok(id)) = (shared.upgrade(), CameraId::new(camera)) else {
                return unknown;
            };
            let state = shared.state.lock().await;
            let Some(config) = state.configs.iter().find(|c| c.id == id) else {
                return unknown;
            };
            let credentials = match &config.restream {
                Some(own) => match own.credentials(|n| std::env::var(n).ok()) {
                    Ok((username, password)) => Some(Credentials { username, password }),
                    // Never served without the password it was given.
                    Err(e) => {
                        warn!(camera = %id, error = %e, "RTSP republication: unusable credentials of the camera, stream not served");
                        return unknown;
                    }
                },
                None => general,
            };
            let tap = state.running.get(&id).and_then(|runtime| {
                if sub {
                    runtime.sub.as_ref().map(|(_, tap)| tap.clone())
                } else {
                    Some(runtime.live.clone())
                }
            });
            Source { tap, credentials }
        })
    })
}

async fn start(shared: &Arc<Shared>, config: &RestreamConfig) -> Result<RestreamServer, String> {
    let addr = config.listen_addr()?;
    let general = config
        .credentials(|n| std::env::var(n).ok())?
        .map(|(username, password)| Credentials { username, password });
    let protected = general.is_some();
    let options = Options {
        max_clients: config.max_clients as usize,
        max_clients_per_address: config.max_clients_per_address as usize,
    };
    let server = RestreamServer::start(addr, lookup(shared, general), options)
        .await
        .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
    info!(
        url = %format!("rtsp://{}/<camera>", server.local_addr()),
        max_clients = config.max_clients,
        password = protected,
        "RTSP republication listening"
    );
    if !addr.ip().is_loopback() && !protected {
        warn!(listen = %addr, "RTSP republication reachable from the network without a general password: anyone who can reach this address can watch the cameras that have no password of their own");
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
