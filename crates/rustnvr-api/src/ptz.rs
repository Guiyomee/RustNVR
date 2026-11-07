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
