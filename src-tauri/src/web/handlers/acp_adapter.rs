use axum::Json;
use serde::Deserialize;

use crate::acp::adapter_store::{AdapterStatus, UpgradeOutcome};
use crate::app_error::AppCommandError;
use crate::commands::acp_adapter as commands;
use crate::models::agent::AgentType;
use crate::web::handlers::acp::AgentTypeParams;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterUpgradeParams {
    pub agent_type: AgentType,
    #[serde(default)]
    pub version: Option<String>,
}

pub async fn acp_adapter_status(
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<AdapterStatus>, AppCommandError> {
    commands::acp_adapter_status_core(params.agent_type)
        .await
        .map(Json)
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn acp_adapter_upgrade(
    Json(params): Json<AdapterUpgradeParams>,
) -> Result<Json<UpgradeOutcome>, AppCommandError> {
    commands::acp_adapter_upgrade_core(params.agent_type, params.version)
        .await
        .map(Json)
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn acp_adapter_rollback(
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<AdapterStatus>, AppCommandError> {
    commands::acp_adapter_rollback_core(params.agent_type)
        .await
        .map(Json)
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}
