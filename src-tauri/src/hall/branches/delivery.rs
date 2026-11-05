//! User-requested, durable delivery. Always query before retrying a POST.
use super::{delivery_v2, dispatch, dispatch_result::TaskReport, inbox};
use crate::app_error::AppCommandError;
use crate::hall::exchange::{ArtifactRef, Exchange};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::Deserialize;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::OnceLock,
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};
static REQUESTS: Semaphore = Semaphore::const_new(8);
pub(super) fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Dispatch delivery storage is unavailable")
}
pub(super) fn failure() -> AppCommandError {
    AppCommandError::network("Dispatch sync failed; retry to query its current state")
}
pub(super) fn sql(q: &str, v: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, q, v)
}
#[derive(FromQueryResult)]
pub(super) struct Target {
    pub branch_id: String,
    pub issuer_id: String,
    pub secret: String,
    pub endpoint: String,
}
#[derive(FromQueryResult)]
pub(super) struct Intent {
    pub dispatch_id: String,
    pub branch_id: String,
    pub member_ref: String,
    pub title: String,
    pub description: String,
    status: String,
    pub remote_revision: i64,
    remote_task_revision: i64,
    remote_task_id: Option<String>,
    task_status: Option<String>,
    remote_attempt_number: Option<i64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Report {
    pub(super) dispatch_id: String,
    pub(super) issuer_id: String,
    pub(super) member_ref: String,
    pub(super) title: String,
    pub(super) status: String,
    pub(super) revision: i64,
    pub(super) launch_allowed: bool,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) task: Option<TaskReport>,
    // v2 branches only; v1 reports never carry these.
    #[serde(default)]
    pub(super) received_inputs: Vec<String>,
    #[serde(default)]
    pub(super) artifacts: Vec<ArtifactRef>,
}
pub(super) async fn intent<C: ConnectionTrait>(
    db: &C,
    id: &str,
) -> Result<Intent, AppCommandError> {
    Intent::find_by_statement(sql("SELECT dispatch_id,branch_id,member_ref,title,description,status,remote_revision,remote_task_revision,remote_task_id,task_status,remote_attempt_number FROM hall_branch_dispatch WHERE dispatch_id=?",vec![id.into()])).one(db).await.map_err(storage)?.ok_or_else(||AppCommandError::not_found("Dispatch not found"))
}
pub(super) async fn target(
    db: &DatabaseConnection,
    branch: &str,
) -> Result<Target, AppCommandError> {
    let target=Target::find_by_statement(sql("SELECT b.id AS branch_id,c.issuer_id,c.secret,c.endpoint FROM hall_branch b JOIN remote_workspace_connection r ON r.id=b.connection_id JOIN hall_branch_dispatch_credential c ON c.branch_id=b.id AND c.endpoint=r.base_url WHERE b.id=? AND b.enabled=1",vec![branch.into()])).one(db).await.map_err(storage)?.ok_or_else(||AppCommandError::permission_denied("Branch is disabled or dispatch credential needs local setup"))?;
    super::service::validate_endpoint(&target.endpoint)?;
    super::service::validate_uuid(&target.issuer_id)?;
    Ok(target)
}
pub(super) fn client() -> Result<&'static reqwest::Client, AppCommandError> {
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(4))
                .timeout(Duration::from_secs(10))
                .gzip(false)
                .brotli(false)
                .build()
        })
        .as_ref()
        .map_err(|_| failure())
}
async fn request(
    db: &DatabaseConnection,
    target_before: &Target,
    path: &str,
    envelope: Option<&inbox::Envelope>,
) -> Result<Option<Report>, AppCommandError> {
    // Re-resolve before EVERY send. A disable/import/endpoint edit prevents any
    // subsequent step of a sync, including a POST after a missing GET.
    let current = target(db, &target_before.branch_id).await?;
    if current.issuer_id != target_before.issuer_id
        || current.secret != target_before.secret
        || current.endpoint != target_before.endpoint
    {
        return Err(failure());
    }
    let mut authorization = reqwest::header::HeaderValue::from_str(&format!(
        "Bearer {}.{}",
        current.issuer_id, current.secret
    ))
    .map_err(|_| failure())?;
    authorization.set_sensitive(true);
    let url = format!("{}/hall-dispatch/v1/inbox{}", current.endpoint, path);
    let request = if let Some(envelope) = envelope {
        client()?.post(url).json(envelope)
    } else {
        client()?.get(url)
    };
    let mut response = request
        .header(reqwest::header::AUTHORIZATION, authorization)
        .send()
        .await
        .map_err(|_| failure())?;
    if envelope.is_none() && response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(failure());
    }
    const LIMIT: usize = 128 * 1024;
    if response.content_length().is_some_and(|n| n > LIMIT as u64) {
        return Err(failure());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
        if bytes.len().saturating_add(chunk.len()) > LIMIT {
            return Err(failure());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| failure())
}
fn terminal(status: &str) -> bool {
    matches!(status, "rejected" | "succeeded" | "failed" | "interrupted")
}
fn status(report: &Report) -> Result<&'static str, AppCommandError> {
    match report.status.as_str() {
        "awaiting_confirmation" if report.task.is_none() => Ok("awaiting_acceptance"),
        "rejected" if report.task.is_none() => Ok("rejected"),
        "accepted" => {
            if let Some(task) = &report.task {
                if task.revision < 1
                    || task.result_text.as_ref().is_some_and(|v| v.len() > 16384)
                    || uuid::Uuid::parse_str(&task.task_id).is_err()
                {
                    return Err(failure());
                }
                match task.status.as_str() {
                    "draft" | "queued" => Ok("accepted"),
                    "running" => Ok("running"),
                    "awaiting_permission" => Ok("awaiting_permission"),
                    "succeeded" if task.result_outcome.as_deref() == Some("succeeded") => {
                        Ok("succeeded")
                    }
                    "succeeded" => Ok("outcome_unknown"),
                    "failed" => Ok("failed"),
                    "interrupted" | "cancelled" => Ok("interrupted"),
                    _ => Err(failure()),
                }
            } else {
                Ok("accepted")
            }
        }
        _ => Err(failure()),
    }
}
pub(super) async fn apply(
    db: &DatabaseConnection,
    target: &Target,
    id: &str,
    report: Report,
) -> Result<(), AppCommandError> {
    let tx = db.begin().await.map_err(storage)?;
    // Take SQLite write lock before reading revisions, also across HQ processes.
    tx.execute(sql(
        "UPDATE hall_branch_dispatch SET revision=revision WHERE dispatch_id=?",
        vec![id.into()],
    ))
    .await
    .map_err(storage)?;
    let old = intent(&tx, id).await?;
    if report.dispatch_id != old.dispatch_id
        || report.issuer_id != target.issuer_id
        || report.member_ref != old.member_ref
        || report.title != old.title
        || report
            .description
            .as_ref()
            .is_some_and(|v| v != &old.description)
        || report.revision < 1
        || report.launch_allowed
    {
        return Err(failure());
    }
    let next = status(&report)?;
    // Inbox and task revisions advance independently. Never use one cursor for
    // both, nor allow a stale task report to downgrade a known terminal.
    if report.revision < old.remote_revision {
        return Err(failure());
    }
    if let Some(task) = &report.task {
        if old
            .remote_task_id
            .as_ref()
            .is_some_and(|id| id != &task.task_id)
            || task.revision < old.remote_task_revision
        {
            return Err(failure());
        }
        // A late report from an older attempt must never replace a newer one.
        if let (Some(old_n), Some(new_n)) = (old.remote_attempt_number, task.attempt_number) {
            if new_n < old_n {
                return Err(failure());
            }
        }
    } else if old.remote_task_id.is_some() {
        return Err(failure());
    }
    // An interrupted task can be finalized by explicit host recovery after
    // verified exit. Preserve cancelled tasks and all completed outcomes, but
    // accept the same bound task's newer recovered success/failure report.
    let recovered = old.status == "interrupted"
        && old.task_status.as_deref() == Some("interrupted")
        && matches!(next, "succeeded" | "failed")
        && report
            .task
            .as_ref()
            .is_some_and(|task| task.revision > old.remote_task_revision);
    if terminal(&old.status) && old.status != next && !recovered {
        return Err(failure());
    }
    if report.revision == old.remote_revision && old.remote_revision > 0 {
        let old_inbox = match old.status.as_str() {
            "awaiting_acceptance" => "awaiting_confirmation",
            "rejected" => "rejected",
            _ => "accepted",
        };
        if old_inbox != report.status {
            return Err(failure());
        }
    }
    let identity = report.task.as_ref().map(|t| {
        (
            t.attempt_id.clone(),
            t.run_id.clone(),
            t.attempt_number,
            t.exit_verified,
        )
    });
    let (task_id, task_revision, task_status, result_text, result_outcome) = match report.task {
        Some(t) => (
            Some(t.task_id),
            t.revision,
            Some(t.status),
            t.result_text,
            t.result_outcome,
        ),
        None => (None, 0, None, None, None),
    };
    let (attempt_id, run_id, attempt_number, exit_verified) = identity.unwrap_or_default();
    // Equal revisions cannot modify already observed task state or result.
    if task_revision > 0 && task_revision == old.remote_task_revision {
        #[derive(FromQueryResult)]
        struct Previous {
            result_outcome: Option<String>,
            task_status: Option<String>,
            result_text: Option<String>,
        }
        let prev = Previous::find_by_statement(sql(
            "SELECT remote_result_outcome AS result_outcome,task_status,result_text FROM hall_branch_dispatch WHERE dispatch_id=?",
            vec![id.into()],
        ))
        .one(&tx)
        .await
        .map_err(storage)?
        .ok_or_else(failure)?;
        if prev.task_status != task_status
            || prev.result_text != result_text
            || prev.result_outcome != result_outcome
        {
            return Err(failure());
        }
    }
    tx.execute(sql("UPDATE hall_branch_dispatch SET status=?,revision=revision+1,remote_revision=?,remote_task_revision=?,remote_task_id=?,task_status=?,result_text=?,remote_result_outcome=?,sync_error=NULL,remote_attempt_id=COALESCE(?,remote_attempt_id),remote_run_id=COALESCE(?,remote_run_id),remote_attempt_number=COALESCE(?,remote_attempt_number),remote_exit_verified=COALESCE(?,remote_exit_verified),updated_at=? WHERE dispatch_id=?",vec![next.into(),report.revision.into(),task_revision.into(),task_id.into(),task_status.into(),result_text.into(),result_outcome.into(),attempt_id.into(),run_id.into(),attempt_number.into(),exit_verified.into(),chrono::Utc::now().to_rfc3339().into(),id.into()])).await.map_err(storage)?;
    tx.commit().await.map_err(storage)?;
    Ok(())
}
async fn deliver(
    db: &DatabaseConnection,
    exchange: Option<&Exchange>,
    id: &str,
) -> Result<(), AppCommandError> {
    let item = intent(db, id).await?;
    let target = target(db, &item.branch_id).await?;
    // Bind the first attempt before any network I/O. Changing trust or moving
    // a branch after an unknown delivery must not duplicate across namespaces.
    let bound=db.execute(sql("UPDATE hall_branch_dispatch SET delivery_issuer_id=?,delivery_endpoint=? WHERE dispatch_id=? AND (delivery_issuer_id IS NULL OR delivery_issuer_id=?) AND (delivery_endpoint IS NULL OR delivery_endpoint=?)",vec![target.issuer_id.clone().into(),target.endpoint.clone().into(),id.into(),target.issuer_id.clone().into(),target.endpoint.clone().into()])).await.map_err(storage)?;
    if bound.rows_affected() != 1 {
        return Err(failure());
    }
    if let Some(exchange) = exchange {
        if let Some(binding) = exchange.dispatch_binding(id).await.map_err(|_| failure())? {
            return Box::pin(delivery_v2::deliver(db, exchange, &item, &target, binding)).await;
        }
    } else if db
        .query_one(sql(
            "SELECT 1 FROM hall_exchange_attachment WHERE dispatch_id=?",
            vec![id.into()],
        ))
        .await
        .map_err(storage)?
        .is_some()
    {
        // Never downgrade an attachment dispatch to the text-only protocol.
        return Err(failure());
    }
    let path = format!("/{id}");
    let report = match request(db, &target, &path, None).await? {
        Some(report) => report,
        None => {
            // A receiver must never forget an already observed record. Fail
            // closed rather than recreating a possibly completed local task.
            if item.remote_revision > 0 {
                return Err(failure());
            }
            request(
                db,
                &target,
                "",
                Some(&inbox::Envelope {
                    dispatch_id: item.dispatch_id,
                    member_ref: item.member_ref,
                    title: item.title,
                    description: item.description,
                }),
            )
            .await?
            .ok_or_else(failure)?
        }
    };
    apply(db, &target, id, report).await
}
pub async fn sync(db: &DatabaseConnection, id: &str) -> Result<dispatch::Receipt, AppCommandError> {
    sync_with(db, None, id).await
}
pub async fn sync_with(
    db: &DatabaseConnection,
    exchange: Option<&Exchange>,
    id: &str,
) -> Result<dispatch::Receipt, AppCommandError> {
    super::service::validate_uuid(id)?;
    intent(db, id).await?;
    let _permit = REQUESTS
        .try_acquire()
        .map_err(|_| AppCommandError::network("Dispatch sync is busy; retry shortly"))?;
    static LOCKS: OnceLock<Vec<Mutex<()>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| (0..64).map(|_| Mutex::new(())).collect());
    let mut hash = DefaultHasher::new();
    id.hash(&mut hash);
    let _guard = locks[(hash.finish() % 64) as usize].lock().await;
    if Box::pin(deliver(db, exchange, id)).await.is_err() {
        db.execute(sql("UPDATE hall_branch_dispatch SET sync_error='Dispatch sync failed; retry to query current state',revision=revision+1,updated_at=? WHERE dispatch_id=?",vec![chrono::Utc::now().to_rfc3339().into(),id.into()])).await.map_err(storage)?;
    }
    // Files first: the coordinator is told only once every sealed artifact
    // that the branch reported has been collected (or no files are involved).
    if !delivery_v2::collection_pending(db, id).await? {
        crate::hall::team::mailbox::notify_branch_result(db, id).await?;
    }
    dispatch::get(db, id).await
}
