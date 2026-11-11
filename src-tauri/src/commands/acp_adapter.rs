//! Settings-page commands for the managed ACP adapter (version status,
//! probe-gated upgrade, rollback). The work lives in [`crate::acp::adapter_store`].

use crate::acp::adapter_cli::AdapterKind;
use crate::acp::adapter_store::{self, AdapterStatus, UpgradeOutcome};
use crate::acp::error::AcpError;
use crate::models::agent::AgentType;

fn kind_for(agent_type: AgentType) -> Result<AdapterKind, AcpError> {
    AdapterKind::for_agent(agent_type)
        .ok_or_else(|| AcpError::protocol("this agent has no managed ACP adapter"))
}

pub(crate) async fn acp_adapter_status_core(
    agent_type: AgentType,
) -> Result<AdapterStatus, AcpError> {
    let kind = kind_for(agent_type)?;
    let root = adapter_store::default_root();
    let runtime = adapter_store::effective_runtime(&root, kind).await;
    Ok(adapter_store::status(&root, kind, runtime.as_ref()))
}

/// Install `version` (the newest when `None`), probe it, and switch to it. A
/// version that fails the probe is discarded and the active one is kept.
pub(crate) async fn acp_adapter_upgrade_core(
    agent_type: AgentType,
    version: Option<String>,
) -> Result<UpgradeOutcome, AcpError> {
    let kind = kind_for(agent_type)?;
    adapter_store::upgrade(&adapter_store::default_root(), kind, version.as_deref())
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))
}

pub(crate) async fn acp_adapter_rollback_core(
    agent_type: AgentType,
) -> Result<AdapterStatus, AcpError> {
    let kind = kind_for(agent_type)?;
    adapter_store::rollback(&adapter_store::default_root(), kind)
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    acp_adapter_status_core(agent_type).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_adapter_status(agent_type: AgentType) -> Result<AdapterStatus, AcpError> {
    acp_adapter_status_core(agent_type).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_adapter_upgrade(
    agent_type: AgentType,
    version: Option<String>,
) -> Result<UpgradeOutcome, AcpError> {
    acp_adapter_upgrade_core(agent_type, version).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_adapter_rollback(agent_type: AgentType) -> Result<AdapterStatus, AcpError> {
    acp_adapter_rollback_core(agent_type).await
}
