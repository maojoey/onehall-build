use crate::hall::members::LaunchProfile;
use chrono::{DateTime, Utc};
use sea_orm::FromQueryResult;
use serde::Serialize;
use std::collections::BTreeMap;

// Internal only: no Debug/Serialize/Deserialize of profiles, claims or bindings.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExecutionOwner {
    pub(super) member: String,
    pub(super) task: String,
    pub(super) attempt: String,
    pub(super) snapshot: String,
    pub(super) claim: String,
    pub(super) epoch: String,
}
impl ExecutionOwner {
    pub(crate) fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    &self.member,
                    &self.task,
                    &self.attempt,
                    &self.snapshot,
                    &self.claim,
                    &self.epoch
                ))
                .expect("string tuple")
            )
        )
    }
}
pub struct RunPlan {
    pub(super) owner: ExecutionOwner,
    pub(super) profile: LaunchProfile,
    pub(super) task_id: String,
    pub(super) attempt_id: String,
    pub(super) snapshot_id: String,
    pub(super) description: String,
}
impl RunPlan {
    pub(crate) fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn cwd(&self) -> &str {
        &self.profile.cwd
    }
    pub fn member_id(&self) -> &str {
        &self.profile.member_id
    }
    pub fn mode(&self) -> &str {
        &self.profile.mode
    }
    pub fn engine(&self) -> &str {
        &self.profile.engine
    }
    pub fn options(&self) -> &BTreeMap<String, String> {
        &self.profile.config
    }
    pub(crate) fn launch_profile(&self) -> &LaunchProfile {
        &self.profile
    }
    pub fn prompt(&self) -> String {
        format!("{}\n\n{}", self.description, self.profile.brief)
    }
}
pub struct Binding {
    pub(super) owner: ExecutionOwner,
    pub(super) lease_id: String,
    pub(super) run_id: String,
    pub(super) connection_id: String,
    pub(super) conversation_id: i32,
    pub(super) external_session_id: String,
    pub(super) started_at: DateTime<Utc>,
}
impl Binding {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }
}
/// Only a runtime adapter that owns and verified the target may construct this.
/// A CLI result/cancel acknowledgment never constructs one.
pub struct ExitEvidence {
    pub(super) owner: ExecutionOwner,
    pub(super) lease_id: String,
    pub(super) run_id: String,
    pub(super) connection_id: String,
    pub(super) confirmed_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeEvent {
    /// Advance the durable cursor for non-lifecycle CLI events without fake progress.
    Observed,
    WaitingInput {
        key: String,
        waiting: bool,
    },
    Result {
        outcome: Outcome,
        body: String,
    },
    CancelAcknowledged,
}
#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
    Cancelled,
    Rejected,
}
impl Outcome {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
        }
    }
}
#[derive(Serialize, FromQueryResult)]
#[serde(rename_all = "camelCase")]
pub struct StoredResult {
    pub result_id: String,
    pub attempt_id: String,
    pub outcome: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Serialize, FromQueryResult)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeObservation {
    pub attempt_id: String,
    pub stage: String,
    pub runtime_kind: String,
    pub intent_at: DateTime<Utc>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub cancel_ack_at: Option<DateTime<Utc>>,
    pub exit_confirmed_at: Option<DateTime<Utc>>,
}
#[derive(FromQueryResult)]
pub(super) struct RuntimeRow {
    pub attempt_id: String,
    pub claim_id: String,
    pub member_id: String,
    pub snapshot_id: String,
    pub stage: String,
    pub run_id: Option<String>,
    pub connection_id: Option<String>,
    pub conversation_id: Option<i32>,
    pub external_session_id: Option<String>,
    pub last_sequence: i64,
    pub last_digest: Option<String>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
}
