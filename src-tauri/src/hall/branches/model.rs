use crate::models::RemoteWorkspaceHeader;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BranchStatus {
    Unknown,
    Online,
    Unreachable,
    AuthFailed,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HallBranchSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub revision: i64,
    pub status: BranchStatus,
    pub checked_at: Option<String>,
}
// Deliberately no Debug or Serialize: this is a write-only credential input.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterBranchInput {
    pub name: String,
    pub base_url: String,
    pub token: String,
    #[serde(default)]
    pub headers: Vec<RemoteWorkspaceHeader>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchConnection {
    pub connection_id: i32,
    pub name: String,
}
