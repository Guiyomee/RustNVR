//! Motorised cameras (ONVIF PTZ): one session per camera, opened on the
//! first command and kept (finding the PTZ service and the profile takes a
//! few requests), opened again when the camera stops answering or its
//! settings change. The presets read are kept, for the Home Assistant
//! selector (MQTT).

use super::Shared;
use rustnvr_api::control::{ControlError, PtzCommand, PtzPreset};
use rustnvr_core::NvrEvent;
use rustnvr_core::config::OnvifTarget;
use rustnvr_discovery::onvif::OnvifError;
use rustnvr_discovery::ptz::{PtzCamera, Velocity};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

/// Timeout of each ONVIF request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A continuous move stops by itself after this long, should the stop never
/// come (browser closed while a button is held).
const MOVE_LIMIT: Duration = Duration::from_secs(10);
/// Longest preset name.
const MAX_PRESET_NAME: usize = 64;

/// Open sessions, by camera, with the address and user they were opened
/// for; the last presets read, by camera.
#[derive(Default)]
pub(super) struct Sessions {
    open: tokio::sync::Mutex<HashMap<String, (String, Arc<PtzCamera>)>>,
    presets: std::sync::Mutex<HashMap<String, Vec<PtzPreset>>>,
}

impl Sessions {
    /// Names of the presets last read from `camera` (empty if never read).
    pub(super) fn preset_names(&self, camera: &str) -> Vec<String> {
        self.presets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(camera)
            .map(|p| p.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Keeps the presets of `camera`; whether they changed.
    fn keep(&self, camera: &str, presets: &[PtzPreset]) -> bool {
        let mut all = self.presets.lock().unwrap_or_else(|e| e.into_inner());
        if all.get(camera).is_some_and(|p| p == presets) {
            return false;
        }
        all.insert(camera.to_owned(), presets.to_vec());
        true
    }

    /// The token of the preset of `camera` named `name`, as last read.
    fn token(&self, camera: &str, name: &str) -> Option<String> {
        self.presets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(camera)?
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.token.clone())
    }
}

/// Address and user of a target: a session is reused while they stay.
fn key(target: &OnvifTarget) -> String {
    let user = target.credentials.as_ref().map_or("", |(u, _)| u.as_str());
    format!("{user}@{}", target.device_url)
}

fn control_error(e: OnvifError) -> ControlError {
    match e {
        OnvifError::NotOnvif => ControlError::Invalid(
            "la caméra ne répond pas en ONVIF à cette adresse (vérifiez le port ONVIF)".into(),
        ),
        // ONVIF answers, without a PTZ service: a fixed camera or a doorbell
        // (the port is there for something else). Not a failure: the
        // interface shows no PTZ panel.
        OnvifError::NoPtz => ControlError::NotFound(OnvifError::NoPtz.to_string()),
        other => ControlError::Invalid(other.to_string()),
    }
}

/// The ONVIF service of `camera`, as configured now.
async fn target(shared: &Shared, camera: &str) -> Result<OnvifTarget, ControlError> {
    let state = shared.state.lock().await;
    let cam = state
        .configs
        .iter()
        .find(|c| c.id.as_str() == camera)
        .ok_or_else(|| ControlError::NotFound(format!("caméra inconnue : {camera}")))?;
    cam.onvif_target(|v| std::env::var(v).ok())
        .map_err(|e| ControlError::Invalid(format!("ONVIF : {e}")))?
        .ok_or_else(|| {
            ControlError::Invalid(
                "pas de port ONVIF réglé pour cette caméra (modifier la caméra → ONVIF)".into(),
            )
        })
}

/// The session of `camera`, opened if needed (`fresh`: opened again).
async fn session(
    shared: &Shared,
    camera: &str,
    fresh: bool,
) -> Result<Arc<PtzCamera>, ControlError> {
    let target = target(shared, camera).await?;
    let key = key(&target);
    let mut sessions = shared.ptz.open.lock().await;
    if !fresh
        && let Some((k, s)) = sessions.get(camera)
        && *k == key
    {
        return Ok(s.clone());
    }
    let opened = Arc::new(
        PtzCamera::connect(&target.device_url, target.credentials, REQUEST_TIMEOUT)
            .await
            .map_err(control_error)?,
    );
    info!(camera, "ONVIF PTZ session opened");
    sessions.insert(camera.to_owned(), (key, opened.clone()));
    Ok(opened)
}

/// Runs `op` on the session of `camera`; on a failure, once more on a new
/// session (the camera restarted, its session expired…).
async fn with_camera<T, F, Fut>(shared: &Shared, camera: &str, op: F) -> Result<T, ControlError>
where
    F: Fn(Arc<PtzCamera>) -> Fut,
    Fut: Future<Output = Result<T, OnvifError>>,
{
    let first = op(session(shared, camera, false).await?).await;
    match first {
        Ok(v) => Ok(v),
        Err(OnvifError::Unauthorized) => Err(control_error(OnvifError::Unauthorized)),
        Err(e) => {
            debug!(camera, error = %e, "PTZ command failed: new session");
            op(session(shared, camera, true).await?)
                .await
                .map_err(control_error)
        }
    }
}

/// The presets of `camera`, read from it (and kept: a change updates the
/// Home Assistant selector).
pub(super) async fn presets(shared: &Shared, camera: &str) -> Result<Vec<PtzPreset>, ControlError> {
    let presets: Vec<PtzPreset> = with_camera(shared, camera, |c| async move { c.presets().await })
        .await?
        .into_iter()
        .map(|p| PtzPreset {
            token: p.token,
            name: p.name,
        })
        .collect();
    if shared.ptz.keep(camera, &presets) {
        let _ = shared.engine.events.send(NvrEvent::CamerasChanged);
    }
    Ok(presets)
}

/// Sends `camera` to its preset named `name` (Home Assistant selector).
pub(super) async fn goto_name(
    shared: &Shared,
    camera: &str,
    name: &str,
) -> Result<(), ControlError> {
    let token = match shared.ptz.token(camera, name) {
        Some(t) => t,
        // Changed on the camera since: read again.
        None => presets(shared, camera)
            .await?
            .into_iter()
            .find(|p| p.name == name)
            .map(|p| p.token)
            .ok_or_else(|| ControlError::NotFound(format!("pas de preset {name:?}")))?,
    };
    command(shared, camera, PtzCommand::Goto(token)).await
}

/// Reads the presets of every enabled camera with an ONVIF service, for the
/// Home Assistant selectors (in the background at startup: a camera that
/// does not answer only delays its own).
pub(super) async fn read_all(shared: Arc<Shared>) {
    let cameras: Vec<String> = {
        let state = shared.state.lock().await;
        state
            .configs
            .iter()
            .filter(|c| c.enabled && c.onvif.is_some())
            .map(|c| c.id.to_string())
            .collect()
    };
    for camera in cameras {
        match presets(&shared, &camera).await {
            Ok(p) => info!(camera, presets = p.len(), "PTZ presets read"),
            Err(e) => debug!(camera, error = %e, "PTZ presets not read"),
        }
    }
}

/// Moves `camera`, or changes its presets.
pub(super) async fn command(
    shared: &Shared,
    camera: &str,
    command: PtzCommand,
) -> Result<(), ControlError> {
    match command {
        PtzCommand::Move(m) => {
            let velocity = Velocity {
                pan: m.pan,
                tilt: m.tilt,
                zoom: m.zoom,
            };
            with_camera(shared, camera, |c| async move {
                c.continuous_move(velocity, MOVE_LIMIT).await
            })
            .await
        }
        PtzCommand::Stop => with_camera(shared, camera, |c| async move { c.stop().await }).await,
        PtzCommand::Goto(token) => {
            let token = &token;
            with_camera(
                shared,
                camera,
                |c| async move { c.goto_preset(token).await },
            )
            .await
        }
        PtzCommand::SetPreset(name) => {
            let name = name.trim().to_owned();
            if name.is_empty() || name.chars().count() > MAX_PRESET_NAME {
                return Err(ControlError::Invalid(format!(
                    "le nom du preset doit avoir de 1 à {MAX_PRESET_NAME} caractères"
                )));
            }
            let name = &name;
            let token =
                with_camera(shared, camera, |c| async move { c.set_preset(name).await }).await?;
            info!(camera, preset = %token, "PTZ preset stored");
            // The list kept for Home Assistant.
            let _ = presets(shared, camera).await;
            Ok(())
        }
        PtzCommand::RemovePreset(token) => {
            let token = &token;
            with_camera(
                shared,
                camera,
                |c| async move { c.remove_preset(token).await },
            )
            .await?;
            info!(camera, preset = %token, "PTZ preset deleted");
            let _ = presets(shared, camera).await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::Secret;

    #[test]
    fn a_session_is_kept_for_the_same_address_and_user() {
        let target = |url: &str, user: Option<&str>| OnvifTarget {
            device_url: url.to_owned(),
            credentials: user.map(|u| (u.to_owned(), Secret::new("x".to_owned()))),
        };
        let a = key(&target(
            "http://10.0.0.2/onvif/device_service",
            Some("admin"),
        ));
        assert_eq!(
            a,
            key(&target(
                "http://10.0.0.2/onvif/device_service",
                Some("admin")
            ))
        );
        assert_ne!(
            a,
            key(&target(
                "http://10.0.0.2:8000/onvif/device_service",
                Some("admin")
            ))
        );
        assert_ne!(
            a,
            key(&target("http://10.0.0.2/onvif/device_service", None))
        );
        assert!(matches!(
            control_error(OnvifError::NotOnvif),
            ControlError::Invalid(m) if m.contains("port ONVIF")
        ));
        // A camera without motor: « not found » (404), the panel is hidden.
        assert!(matches!(
            control_error(OnvifError::NoPtz),
            ControlError::NotFound(_)
        ));
    }
}
