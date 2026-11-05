//! Authenticated human review is durable queue intent, never real admission.
//! Model PM tools enqueue proposals but cannot mint this review receipt.
use super::{
    queue::{QueueHost, QueueReceipt},
    store::{by_id, conflict, storage_error},
};
use crate::{
    app_error::AppCommandError,
    hall::members::{snapshot_launch_profile, ExecutionCapabilities, ExecutionMode},
};
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
pub(crate) mod migration;
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchReview {
    pub task_id: String,
    pub task_revision: i64,
    pub member_id: String,
    pub member_revision: i64,
    pub snapshot_id: String,
    pub snapshot_version: String,
    pub project_id: Option<i32>,
    pub title: String,
    pub description: String,
    pub cwd: String,
    pub engine: String,
    pub mode: String,
    pub options: BTreeMap<String, String>,
    pub brief: String,
    /// Both omitted for a snapshot that never chose a mode, so reviews approved
    /// before this field existed keep their exact hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecutionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ExecutionCapabilities>,
    /// Frozen input/output manifest of an attached dispatch. Omitted when the
    /// task has none, so every earlier review keeps its exact hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_manifest_digest: Option<String>,
    /// Display copy of that manifest (input names, sizes, output slots). The
    /// digest above binds it, so it stays out of the plan hash and plans
    /// stored before this field existed still compare equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_manifest: Option<crate::hall::exchange::ExchangeManifest>,
    pub plan_hash: String,
    pub real_execution_allowed: bool,
    pub admission: String,
}
fn denied() -> AppCommandError {
    AppCommandError::permission_denied(
        "Launch plan needs a current human review; real execution \
    remains closed",
    )
}
/// Equality minus the display-only manifest copy, which the digest binds.
fn same_plan(mut rebuilt: LaunchReview, stored: &LaunchReview) -> bool {
    rebuilt
        .exchange_manifest
        .clone_from(&stored.exchange_manifest);
    rebuilt == *stored
}
impl LaunchReview {
    fn fingerprint(&self) -> Result<String, AppCommandError> {
        let mut p = self.clone();
        p.plan_hash.clear();
        p.exchange_manifest = None;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&p).map_err(|_| denied())?)
        ))
    }
}
async fn build<C: ConnectionTrait>(
    db: &C,
    id: &str,
    revision: Option<i64>,
) -> Result<LaunchReview, AppCommandError> {
    let t = by_id(db, id).await?;
    super::store::validate(&t)?;
    let saved = crate::hall::members::snapshot_reference(db, &t.member_id, &t.snapshot_id).await?;
    if saved.snapshot_version != t.snapshot_version
        || saved.member_revision != t.snapshot_member_revision
        || saved.project_id != t.project_id
    {
        return Err(denied());
    }
    let p = snapshot_launch_profile(db, &t.member_id, &t.snapshot_id).await?;
    #[derive(FromQueryResult)]
    struct Current {
        revision: i64,
        cwd: String,
        engine: String,
        assistant_uid: Option<String>,
    }
    let m = Current::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT revision,cwd,engine,assistant_uid FROM hall_member WHERE \
     member_id=?",
        vec![t.member_id.clone().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)?
    .ok_or_else(denied)?;
    if m.revision != p.member_revision
        || m.cwd != p.cwd
        || !crate::hall::engine::supported(&m.engine)
        || m.assistant_uid != p.assistant_uid
        || tokio::fs::canonicalize(&p.cwd)
            .await
            .map_err(|_| denied())?
            .to_str()
            != Some(p.cwd.as_str())
    {
        return Err(denied());
    }
    let mut plan = LaunchReview {
        task_id: t.task_id.clone(),
        task_revision: revision.unwrap_or(t.revision),
        member_id: t.member_id,
        member_revision: p.member_revision,
        snapshot_id: t.snapshot_id,
        snapshot_version: t.snapshot_version,
        project_id: t.project_id,
        title: t.title,
        description: t.description,
        cwd: p.cwd,
        engine: m.engine,
        mode: p.mode,
        options: p.config,
        brief: p.brief,
        execution_mode: p.execution_mode,
        capabilities: p.capabilities,
        exchange_manifest_digest: manifest_digest(db, &t.task_id).await?,
        exchange_manifest: None,
        plan_hash: String::new(),
        real_execution_allowed: false,
        admission: "closed_pending_native_b10_and_human_scope".into(),
    };
    plan.plan_hash = plan.fingerprint()?;
    if plan.exchange_manifest_digest.is_some() {
        let shown = crate::hall::exchange::review::for_task(db, &plan.task_id)
            .await
            .map_err(|_| denied())?
            .ok_or_else(denied)?;
        if Some(&shown.digest) != plan.exchange_manifest_digest.as_ref() {
            return Err(denied());
        }
        plan.exchange_manifest = Some(shown);
    }
    Ok(plan)
}
/// Digest of the attached manifest, but only once every input byte has been
/// verified and stored. Until then the plan cannot be built, approved or launched.
async fn manifest_digest<C: ConnectionTrait>(
    db: &C,
    task: &str,
) -> Result<Option<String>, AppCommandError> {
    // Older schemas (before the exchange tables) have no attachments at all.
    if db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='hall_exchange_attachment'",
        ))
        .await
        .map_err(storage_error)?
        .is_none()
        || db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='hall_branch_task_binding'",
            ))
            .await
            .map_err(storage_error)?
            .is_none()
    {
        return Ok(None);
    }
    let Some(row) = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT m.digest AS digest,(SELECT COUNT(*) FROM hall_exchange_input i \
             WHERE i.submission_id=a.submission_id AND (i.confirmed=0 OR i.blob_id IS NULL)) AS pending \
             FROM hall_branch_task_binding b \
             JOIN hall_exchange_attachment a ON a.dispatch_id=b.dispatch_id AND a.issuer_id=b.issuer_id \
             JOIN hall_exchange_manifest m ON m.manifest_id=a.manifest_id \
             WHERE b.task_id=? AND a.revoked=0",
            vec![task.to_owned().into()],
        ))
        .await
        .map_err(storage_error)?
    else {
        return Ok(None);
    };
    let pending: i64 = row.try_get("", "pending").map_err(storage_error)?;
    if pending != 0 {
        return Err(denied());
    }
    Ok(Some(row.try_get("", "digest").map_err(storage_error)?))
}
pub async fn preview(db: &DatabaseConnection, id: &str) -> Result<LaunchReview, AppCommandError> {
    build(db, id, None).await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Approval {
    pub task_id: String,
    pub expected_revision: i64,
    pub plan_hash: String,
    pub submission_id: String,
}
#[derive(FromQueryResult)]
struct Stored {
    queue_revision: i64,
    attempt_id: String,
    submission_id: String,
    plan_hash: String,
    plan_json: String,
}
async fn stored<C: ConnectionTrait>(
    db: &C,
    clause: &str,
    value: &str,
) -> Result<Option<Stored>, AppCommandError> {
    // clause is one of two fixed host literals; never caller text.
    Stored::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        format!(
            "SELECT queue_revision,attempt_id,submission_id,plan_hash,\
     plan_json FROM hall_task_review WHERE {clause}=?"
        ),
        vec![value.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)
}
pub async fn approve(
    db: &DatabaseConnection,
    a: Approval,
) -> Result<QueueReceipt, AppCommandError> {
    let submission = uuid::Uuid::parse_str(&a.submission_id)
        .map_err(|_| denied())?
        .to_string();
    let tx = db.begin().await.map_err(storage_error)?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_task SET revision=revision WHERE task_id=?",
        vec![a.task_id.clone().into()],
    ))
    .await
    .map_err(storage_error)?;
    if let Some(old) = stored(&tx, "submission_id", &submission).await? {
        let plan: LaunchReview = serde_json::from_str(&old.plan_json).map_err(|_| denied())?;
        if plan.task_id != a.task_id
            || plan.task_revision != a.expected_revision
            || plan.plan_hash != a.plan_hash
            || old.plan_hash != a.plan_hash
            || plan.fingerprint()? != a.plan_hash
        {
            return Err(conflict());
        }
        let receipt = QueueReceipt {
            attempt_id: old.attempt_id,
            task_id: plan.task_id,
            task_revision: old.queue_revision,
        };
        tx.commit().await.map_err(storage_error)?;
        return Ok(receipt);
    }
    let t = by_id(&tx, &a.task_id).await?;
    if t.revision != a.expected_revision || !matches!(t.status.as_str(), "draft" | "queued") {
        return Err(conflict());
    }
    let plan = build(&tx, &a.task_id, None).await?;
    if plan.plan_hash != a.plan_hash {
        return Err(conflict());
    }
    let receipt = if t.status == "draft" {
        QueueHost::enqueue_in(&tx, &a.task_id, a.expected_revision, &submission).await?
    } else {
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT a.attempt_id FROM hall_task_attempt a WHERE a.task_id=? \
      AND a.status='queued' AND NOT EXISTS(SELECT 1 FROM \
      hall_task_dispatch d WHERE d.attempt_id=a.attempt_id) ORDER BY \
      a.attempt_number DESC LIMIT 1",
                vec![a.task_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?
            .ok_or_else(conflict)?;
        QueueReceipt {
            attempt_id: row.try_get("", "attempt_id").map_err(storage_error)?,
            task_id: a.task_id.clone(),
            task_revision: a.expected_revision,
        }
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_task_review(attempt_id,submission_id,\
     queue_revision,plan_hash,plan_json,approved_at) VALUES(?,?,?,?,\
     ?,?)",
        vec![
            receipt.attempt_id.clone().into(),
            submission.into(),
            receipt.task_revision.into(),
            plan.plan_hash.clone().into(),
            serde_json::to_string(&plan).map_err(|_| denied())?.into(),
            Utc::now().into(),
        ],
    ))
    .await
    .map_err(|_| conflict())?;
    tx.commit().await.map_err(storage_error)?;
    Ok(receipt)
}
pub(crate) async fn validate_dispatch(
    db: &DatabaseConnection,
    attempt: &str,
    task: &str,
) -> Result<(), AppCommandError> {
    let old = stored(db, "attempt_id", attempt)
        .await?
        .ok_or_else(denied)?;
    let p: LaunchReview = serde_json::from_str(&old.plan_json).map_err(|_| denied())?;
    if old.submission_id.is_empty()
        || p.task_id != task
        || p.plan_hash != old.plan_hash
        || p.fingerprint()? != old.plan_hash
        || !same_plan(build(db, task, Some(p.task_revision)).await?, &p)
    {
        return Err(denied());
    }
    Ok(())
}
#[derive(FromQueryResult, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReceipt {
    pub attempt_id: String,
    pub plan_hash: String,
    pub approved_at: String,
}
pub(crate) async fn receipts<C: ConnectionTrait>(
    db: &C,
    task: &str,
) -> Result<Vec<ReviewReceipt>, AppCommandError> {
    if db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND \
     name='hall_task_review'",
        ))
        .await
        .map_err(storage_error)?
        .is_none()
    {
        if db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT 1 FROM seaql_migrations WHERE \
      version='onehall_m20261025_000007_team_launch_review'",
            ))
            .await
            .map_err(storage_error)?
            .is_some()
        {
            return Err(denied());
        }
        return Ok(Vec::new());
    }
    ReviewReceipt::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT v.attempt_id,v.plan_hash,v.approved_at FROM \
     hall_task_review v JOIN hall_task_attempt a ON \
     a.attempt_id=v.attempt_id WHERE a.task_id=? ORDER BY \
     a.attempt_number",
        vec![task.to_owned().into()],
    ))
    .all(db)
    .await
    .map_err(storage_error)
}
#[cfg(test)]
pub(crate) mod tests;

pub(crate) async fn approved_plan(
    db: &DatabaseConnection,
    task: &str,
) -> Result<LaunchReview, AppCommandError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT attempt_id FROM hall_task_attempt WHERE task_id=? AND \
     status IN ('queued','starting') ORDER BY attempt_number DESC \
     LIMIT 1",
            vec![task.to_owned().into()],
        ))
        .await
        .map_err(storage_error)?
        .ok_or_else(denied)?;
    let attempt: String = row.try_get("", "attempt_id").map_err(storage_error)?;
    validate_dispatch(db, &attempt, task).await?;
    let old = stored(db, "attempt_id", &attempt)
        .await?
        .ok_or_else(denied)?;
    serde_json::from_str(&old.plan_json).map_err(|_| denied())
}
