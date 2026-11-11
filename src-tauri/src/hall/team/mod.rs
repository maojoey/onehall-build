//! Shared UI/model data services. No engine startup, idle subscriptions,
//! wake-up hooks or background worker exists here. Branch dispatch only sends
//! a pending request through the dedicated authenticated channel; it cannot
//! accept or start a remote task.
use crate::{
    app_error::AppCommandError,
    hall::{
        members,
        tasks::{
            self,
            queue::QueueHost,
            runtime::authority::{Actor, Registry},
        },
    },
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
pub mod api;
mod content;
pub(crate) mod issue_migration;
pub mod mailbox;
pub mod mcp;
pub(crate) mod migration;
use mailbox::{denied, storage, Delivery};

#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum ToolRequest {
    #[serde(rename = "hall_own_task")]
    OwnTask {},
    #[serde(rename = "hall_read_mailbox")]
    ReadMailbox {},
    #[serde(rename = "hall_ack_message")]
    AckMessage { message_id: String },
    #[serde(rename = "hall_report")]
    Report { submission_id: String, body: String },
    #[serde(rename = "hall_list_inputs")]
    ListInputs {},
    #[serde(rename = "hall_list_outputs")]
    ListOutputs {},
    #[serde(rename = "hall_read_input", rename_all = "camelCase")]
    ReadInput {
        object_id: String,
        #[serde(default)]
        offset: u64,
        max_bytes: Option<u64>,
    },
    #[serde(rename = "hall_submit_output", rename_all = "camelCase")]
    SubmitOutput { slot_id: String },
    #[serde(rename = "hall_list_members")]
    ListMembers {},
    #[serde(rename = "hall_list_branches")]
    ListBranches {},
    #[serde(rename = "hall_dispatch_branch")]
    DispatchBranch {
        draft: crate::hall::branches::dispatch::Draft,
    },
    #[serde(rename = "hall_branch_dispatch_status")]
    BranchDispatchStatus { dispatch_id: String },
    #[serde(rename = "hall_assign")]
    Assign { draft: tasks::CreateTaskDraft },
    #[serde(rename = "hall_task_status")]
    TaskStatus { task_id: String },
    #[serde(rename = "hall_send_message")]
    SendMessage { draft: mailbox::MessageDraft },
}
/// Models reliably reuse a key on retry but do not reliably produce UUIDs (a
/// real Codex member sent `member-a-notes-hello-20261026`). A UUID passes
/// through unchanged; any other key becomes a UUID derived from the host-bound
/// run plus that key, so retries in the same run stay idempotent and keys can
/// never collide across runs.
fn submission_key(a: &Actor, raw: &str) -> Result<String, AppCommandError> {
    use sha2::{Digest, Sha256};
    if let Ok(id) = uuid::Uuid::parse_str(raw) {
        return Ok(id.to_string());
    }
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().count() > 200 {
        return Err(AppCommandError::invalid_input(
            "submission_id must be a non-empty key of at most 200 characters; \
            reuse the same key when retrying",
        ));
    }
    let digest = Sha256::digest(format!("onehall-submission:{}:{raw}", a.run_id()));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8: custom, name-derived
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}
impl ToolRequest {
    /// The key actions whose failure must stay visible after the run.
    fn key_action(&self) -> Option<&'static str> {
        match self {
            Self::Report { .. } => Some("hall_report"),
            Self::Assign { .. } => Some("hall_assign"),
            Self::DispatchBranch { .. } => Some("hall_dispatch_branch"),
            Self::SendMessage { .. } => Some("hall_send_message"),
            Self::SubmitOutput { .. } => Some("hall_submit_output"),
            _ => None,
        }
    }
    fn with_submission_keys(mut self, a: &Actor) -> Result<Self, AppCommandError> {
        match &mut self {
            Self::Report { submission_id, .. } => {
                *submission_id = submission_key(a, submission_id)?
            }
            Self::Assign { draft } => {
                draft.submission_id = submission_key(a, &draft.submission_id)?
            }
            Self::DispatchBranch { draft } => {
                draft.submission_id = submission_key(a, &draft.submission_id)?
            }
            Self::SendMessage { draft } => {
                draft.submission_id = submission_key(a, &draft.submission_id)?
            }
            _ => {}
        }
        Ok(self)
    }
}
/// Adds the responsible member's current display name to a task the model
/// reads, next to its `memberId`.
async fn with_member_name(
    db: &sea_orm::DatabaseConnection,
    mut task: Value,
) -> Result<Value, AppCommandError> {
    let member = task
        .get("memberId")
        .and_then(Value::as_str)
        .ok_or_else(denied)?
        .to_owned();
    let name = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT name FROM hall_member WHERE member_id=?",
            vec![member.into()],
        ))
        .await
        .map_err(storage)?
        .map(|r| r.try_get::<String>("", "name"))
        .transpose()
        .map_err(storage)?;
    task.as_object_mut()
        .ok_or_else(denied)?
        .insert("memberName".into(), name.into());
    Ok(task)
}
pub struct TeamService {
    host: QueueHost,
    registry: Arc<Registry>,
    /// Absent when the exchange store is unavailable: the content tools then
    /// deny instead of guessing.
    exchange: std::sync::OnceLock<crate::hall::exchange::Exchange>,
}
impl TeamService {
    pub fn new(host: QueueHost, registry: Arc<Registry>) -> Self {
        Self {
            host,
            registry,
            exchange: std::sync::OnceLock::new(),
        }
    }
    pub fn with_exchange(self, exchange: Option<crate::hall::exchange::Exchange>) -> Self {
        if let Some(exchange) = exchange {
            let _ = self.exchange.set(exchange);
        }
        self
    }
    async fn own(&self, a: &Actor) -> Result<(String, String, String), AppCommandError> {
        let ids = self.registry.own_task(a)?;
        // A stale actor cannot continue after recovery/retry/terminal state,
        // and the registry run must be the attempt's recorded run.
        if self
            .host
            .database()
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT 1 FROM hall_task_attempt a JOIN hall_task_dispatch d ON \
            d.attempt_id=a.attempt_id JOIN hall_task_runtime r ON \
            r.attempt_id=a.attempt_id WHERE a.member_id=? AND a.task_id=? \
            AND a.attempt_id=? AND r.run_id=? AND d.review_required=0 AND r.stage IN \
            ('running','waiting_input','finishing','cancel_requested') AND \
            a.attempt_number=(SELECT MAX(attempt_number) FROM \
            hall_task_attempt WHERE task_id=a.task_id)",
                vec![
                    ids.0.clone().into(),
                    ids.1.clone().into(),
                    ids.2.clone().into(),
                    a.run_id().into(),
                ],
            ))
            .await
            .map_err(storage)?
            .is_none()
        {
            return Err(denied());
        }
        Ok(ids)
    }
    async fn pm(&self, a: &Actor) -> Result<(String, String, String), AppCommandError> {
        let ids = self.own(a).await?;
        // Stable host-owned DB binding plus host-issued identity, never the
        // display name, role text, CLI caller or model-supplied member/depth.
        if mailbox::role(self.host.database())
            .await?
            .member_id
            .as_deref()
            != Some(&ids.0)
        {
            return Err(denied());
        }
        Ok(ids)
    }
    pub async fn call(&self, a: &Actor, request: ToolRequest) -> Result<Value, AppCommandError> {
        let action = request.key_action();
        let result = self.run_tool(a, request).await;
        if let Some(tool) = action {
            self.note_key_action(a, tool, result.as_ref().err()).await;
        }
        result
    }
    /// Records a failed key action on the caller's own attempt, or settles
    /// earlier failures of the same tool once a retry gets through. Best
    /// effort: the tool's own answer to the model never depends on it.
    async fn note_key_action(&self, a: &Actor, tool: &str, error: Option<&AppCommandError>) {
        let Ok((_, task, attempt)) = self.registry.own_task(a) else {
            return;
        };
        let now = chrono::Utc::now();
        let statement = match error {
            Some(e) => Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO hall_task_issue(issue_id,task_id,attempt_id,tool,\
            detail,created_at) VALUES(?,?,?,?,?,?)",
                vec![
                    uuid::Uuid::new_v4().to_string().into(),
                    task.into(),
                    attempt.into(),
                    tool.into(),
                    e.message.chars().take(300).collect::<String>().into(),
                    now.into(),
                ],
            ),
            None => Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_issue SET resolved_at=? WHERE attempt_id=? \
            AND tool=? AND resolved_at IS NULL",
                vec![now.into(), attempt.into(), tool.into()],
            ),
        };
        if let Err(e) = self.host.database().execute(statement).await {
            tracing::warn!("[hall] recording team action outcome failed: {e}");
        }
    }
    async fn run_tool(&self, a: &Actor, request: ToolRequest) -> Result<Value, AppCommandError> {
        fn encode<T: serde::Serialize>(v: T) -> Result<Value, AppCommandError> {
            serde_json::to_value(v).map_err(|_| denied())
        }
        let db = self.host.database();
        let request = request.with_submission_keys(a)?;
        match request {
            ToolRequest::OwnTask {} => {
                let (_, task, _) = self.own(a).await?;
                with_member_name(db, encode(tasks::get_task(db, task).await?)?).await
            }
            ToolRequest::ReadMailbox {} => {
                let (member, _, _) = self.own(a).await?;
                encode(mailbox::inbox(db, &member).await?)
            }
            ToolRequest::AckMessage { message_id } => {
                let (member, _, _) = self.own(a).await?;
                mailbox::acknowledge(db, &member, &message_id).await?;
                Ok(serde_json::json!({"acknowledged":true}))
            }
            ToolRequest::Report {
                submission_id,
                body,
            } => {
                let (sender, task, attempt) = self.own(a).await?;
                let recipient = mailbox::role(db).await?.member_id.ok_or_else(denied)?;
                encode(
                    mailbox::send(
                        db,
                        Delivery {
                            submission: &submission_id,
                            sender: Some(&sender),
                            recipient: &recipient,
                            task: Some(&task),
                            attempt: Some(&attempt),
                            kind: "report",
                            body: &body,
                        },
                    )
                    .await?,
                )
            }
            ToolRequest::ListInputs {} => encode(self.list_inputs(a).await?),
            ToolRequest::ListOutputs {} => encode(self.list_outputs(a).await?),
            ToolRequest::ReadInput {
                object_id,
                offset,
                max_bytes,
            } => encode(self.read_input(a, &object_id, offset, max_bytes).await?),
            ToolRequest::SubmitOutput { slot_id } => {
                self.submit_output(a, &slot_id).await
            }
            ToolRequest::ListMembers {} => {
                self.pm(a).await?;
                let mut rows = Vec::new();
                for member in members::hall_list_members_core(db).await? {
                    let snapshots =
                        members::hall_list_member_snapshots_core(db, member.member_id.clone())
                            .await?;
                    let mut row = encode(member)?;
                    row.as_object_mut()
                        .ok_or_else(denied)?
                        .insert("savedSnapshots".into(), encode(snapshots)?);
                    rows.push(row);
                }
                Ok(Value::Array(rows))
            }
            ToolRequest::ListBranches {} => {
                self.pm(a).await?;
                encode(crate::hall::branches::service::list(db).await?)
            }
            ToolRequest::DispatchBranch { draft } => {
                let (member, _, _) = self.pm(a).await?;
                let receipt =
                    crate::hall::branches::dispatch::create_for_pm(db, draft, &member).await?;
                encode(crate::hall::branches::delivery::sync(db, &receipt.dispatch_id).await?)
            }
            ToolRequest::BranchDispatchStatus { dispatch_id } => {
                self.pm(a).await?;
                encode(crate::hall::branches::delivery::sync(db, &dispatch_id).await?)
            }
            ToolRequest::Assign { draft } => {
                self.pm(a).await?;
                let submission = draft.submission_id.clone();
                let task = tasks::create_task(db, draft).await?;
                encode(self.host.enqueue(&task.task_id, 1, &submission).await?)
            }
            ToolRequest::TaskStatus { task_id } => {
                self.pm(a).await?;
                with_member_name(db, encode(tasks::get_task(db, task_id).await?)?).await
            }
            ToolRequest::SendMessage { draft } => {
                let (sender, _, _) = self.pm(a).await?;
                encode(
                    mailbox::send(
                        db,
                        Delivery {
                            submission: &draft.submission_id,
                            sender: Some(&sender),
                            recipient: &draft.recipient_member_id,
                            task: None,
                            attempt: None,
                            kind: "message",
                            body: &draft.body,
                        },
                    )
                    .await?,
                )
            }
        }
    }
}
#[cfg(test)]
pub(crate) mod tests;
