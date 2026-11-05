use crate::hall::members::SnapshotReference;
use chrono::{DateTime, Utc};
use sea_orm::FromQueryResult;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPurpose {
    Work,
    Acceptance,
}
impl TaskPurpose {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Acceptance => "acceptance",
        }
    }
}
fn explicit_project<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    Option::<i32>::deserialize(d)
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTaskDraft {
    pub submission_id: String,
    pub member_id: String,
    pub snapshot_id: String,
    #[serde(deserialize_with = "explicit_project")]
    pub project_id: Option<i32>,
    pub purpose: TaskPurpose,
    pub title: String,
    pub description: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditTaskDraft {
    pub title: String,
    pub description: String,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskFilter {
    pub member_id: Option<String>,
    pub project_id: Option<i32>,
    #[serde(default)]
    pub unassigned_project: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub task_id: String,
    pub member_id: String,
    pub project_id: Option<i32>,
    pub snapshot: SnapshotReference,
    pub purpose: TaskPurpose,
    pub title: String,
    pub status: String,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub launch_allowed: bool,
    pub runtime_verified: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    #[serde(flatten)]
    pub task: TaskSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<super::runtime::Resolution>,
    pub start_recovery: Vec<super::runtime::StartObservation>,
    pub launch_reviews: Vec<super::review::ReviewReceipt>,
    pub exit_checks: Vec<super::runtime::ExitObservation>,
    pub runtimes: Vec<super::runtime::RuntimeObservation>,
    pub results: Vec<super::runtime::StoredResult>,
    /// Key team actions that failed and were not retried successfully.
    pub issues: Vec<TaskIssue>,
    pub description: String,
    pub attempts: Vec<AttemptRecord>,
    pub history: Vec<HistoryRecord>,
}
#[derive(FromQueryResult, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskIssue {
    pub attempt_id: String,
    pub tool: String,
    pub detail: String,
    pub created_at: DateTime<Utc>,
}
#[derive(FromQueryResult, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub task_id: String,
    pub member_id: String,
    pub snapshot_id: String,
    pub snapshot_version: String,
    pub attempt_number: i64,
    pub status: String,
    pub revision: i64,
    pub conversation_id: Option<i32>,
    pub result_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}
#[derive(FromQueryResult, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub revision: i64,
    pub action: String,
    pub created_at: DateTime<Utc>,
}

// No Debug/Serialize: SQL failures must never print request descriptions.
#[derive(FromQueryResult)]
pub(super) struct TaskRow {
    pub task_id: String,
    pub submission_id: String,
    pub request_hash: String,
    pub member_id: String,
    pub project_id: Option<i32>,
    pub snapshot_id: String,
    pub snapshot_version: String,
    pub snapshot_member_revision: i64,
    pub purpose: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
