//! Motorised cameras (ONVIF PTZ): presets, continuous moves and stops. The
//! NVR talks to the camera ([`crate::NvrControl::ptz_command`]); moving and
//! changing presets are writes, so administrators only (viewers read).

use crate::control::{PtzCommand, PtzMove, PtzPreset};
use crate::routes::{ApiError, control};
use crate::state::ApiState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

/// `GET /cameras/{id}/device` query: `refresh=1` reads the camera again.
#[derive(Deserialize)]
pub(crate) struct DeviceQuery {
    #[serde(default)]
    refresh: Option<String>,
}

/// `GET /cameras/{id}/device`: maker, model, firmware and serial number of
/// the camera (ONVIF).
pub(crate) async fn device(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<DeviceQuery>,
) -> Result<Json<crate::control::CameraDevice>, ApiError> {
    let fresh = q.refresh.as_deref() == Some("1");
    Ok(Json(control(&state)?.camera_device(id, fresh).await?))
}

/// Answer of `POST /cameras/{id}/reboot`.
#[derive(serde::Serialize)]
pub(crate) struct RebootDto {
    /// What the camera answered (« Rebooting in 30 seconds »…), possibly empty.
    message: String,
}

/// `POST /cameras/{id}/reboot`: restarts the camera through ONVIF.
pub(crate) async fn reboot(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::state::ApiState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<RebootDto>, crate::routes::ApiError> {
    let message = crate::routes::control(&state)?.reboot_camera(id).await?;
    Ok(axum::Json(RebootDto { message }))
}
use std::sync::Arc;

/// `GET /cameras/{id}/ptz`.
#[derive(Serialize)]
pub(crate) struct PtzDto {
    presets: Vec<PtzPreset>,
}

/// `POST /cameras/{id}/ptz/presets` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewPreset {
    name: String,
}

/// `GET /cameras/{id}/ptz`: the presets (also proves the camera answers in
/// ONVIF PTZ).
pub(crate) async fn presets(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<PtzDto>, ApiError> {
    let presets = control(&state)?.ptz_presets(id).await?;
    Ok(Json(PtzDto { presets }))
}

async fn run(state: &ApiState, id: String, command: PtzCommand) -> Result<StatusCode, ApiError> {
    control(state)?.ptz_command(id, command).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /cameras/{id}/ptz/move`: moves until `stop` (10 s at most).
pub(crate) async fn move_camera(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(input): Json<PtzMove>,
) -> Result<StatusCode, ApiError> {
    run(&state, id, PtzCommand::Move(input)).await
}

/// `POST /cameras/{id}/ptz/stop`.
pub(crate) async fn stop(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    run(&state, id, PtzCommand::Stop).await
}

/// `POST /cameras/{id}/ptz/presets`: the current position, stored under a name.
pub(crate) async fn create_preset(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(input): Json<NewPreset>,
) -> Result<StatusCode, ApiError> {
    run(&state, id, PtzCommand::SetPreset(input.name)).await
}

/// `POST /cameras/{id}/ptz/presets/{token}/goto`.
pub(crate) async fn goto_preset(
    State(state): State<Arc<ApiState>>,
    Path((id, token)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    run(&state, id, PtzCommand::Goto(token)).await
}

/// `DELETE /cameras/{id}/ptz/presets/{token}`.
pub(crate) async fn delete_preset(
    State(state): State<Arc<ApiState>>,
    Path((id, token)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    run(&state, id, PtzCommand::RemovePreset(token)).await
}
