//! Trusted-host Rust data service, deliberately absent from HTTP/Tauri/MCP.
//! A queue/claim is intent, never a launch or permission grant. No engine is
//! connected here. Unknown old claims keep occupying capacity until a future
//! verified termination/review flow resolves them; nothing is auto-replayed.
use super::store::{by_id, conflict, storage_error, validate};
use crate::app_error::AppCommandError;
use crate::hall::members::snapshot_reference;
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::Serialize;

#[derive(FromQueryResult, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueReceipt {
    pub attempt_id: String,
    pub task_id: String,
    pub task_revision: i64,
}

// Not Deserialize/Serialize/Debug. Callers cannot supply/receive these claims
// over the application protocol. Construction is private to the host service.
pub struct Claim {
    pub(super) attempt_id: String,
    pub(super) task_id: String,
    pub(super) claim_id: String,
    pub(super) host_epoch: String,
}
impl Claim {
    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
}
#[derive(Clone)]
pub struct QueueHost {
    pub(super) db: DatabaseConnection,
    pub(super) host_epoch: String,
    max_active: i64,
}

fn uuid(value: &str) -> Result<String, AppCommandError> {
    uuid::Uuid::parse_str(value)
        .map(|v| v.to_string())
        .map_err(|_| AppCommandError::invalid_input("Invalid queue identity"))
}
async fn event<C: ConnectionTrait>(
    db: &C,
    task_id: &str,
    attempt_id: &str,
    revision: i64,
    action: &str,
) -> Result<(), AppCommandError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_task_event(event_id,task_id,attempt_id,\
            task_revision,action,created_at,source_key) VALUES(?,?,?,?,?,?,\
            ?)",
        vec![
            uuid::Uuid::new_v4().to_string().into(),
            task_id.to_owned().into(),
            attempt_id.to_owned().into(),
            revision.into(),
            action.to_owned().into(),
            Utc::now().into(),
            format!("host:{action}").into(),
        ],
    ))
    .await
    .map_err(storage_error)?;
    Ok(())
}
async fn receipt<C: ConnectionTrait>(
    db: &C,
    submission: &str,
) -> Result<Option<QueueReceipt>, AppCommandError> {
    QueueReceipt::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT a.attempt_id,a.task_id,e.task_revision FROM \
            hall_task_attempt a JOIN hall_task_event e ON \
            e.attempt_id=a.attempt_id AND e.action='task_queued' WHERE \
            a.submission_id=?",
        vec![submission.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)
}

impl QueueHost {
    pub(crate) fn database(&self) -> &DatabaseConnection {
        &self.db
    }
    /// A boot gets a new private epoch; clones share it. No reads, writes,
    /// installs or runtime startup happen in this constructor.
    pub fn new(db: DatabaseConnection, max_active: u16) -> Result<Self, AppCommandError> {
        if !(1..=32).contains(&max_active) {
            return Err(AppCommandError::invalid_input(
                "Queue concurrency must be between 1 and 32",
            ));
        }
        Ok(Self {
            db,
            host_epoch: uuid::Uuid::new_v4().to_string(),
            max_active: i64::from(max_active),
        })
    }
    pub async fn enqueue(
        &self,
        task_id: &str,
        expected_revision: i64,
        submission_id: &str,
    ) -> Result<QueueReceipt, AppCommandError> {
        let txn = self.db.begin().await.map_err(storage_error)?;
        let receipt = Self::enqueue_in(&txn, task_id, expected_revision, submission_id).await?;
        txn.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub(super) async fn enqueue_in(
        txn: &sea_orm::DatabaseTransaction,
        task_id: &str,
        expected_revision: i64,
        submission_id: &str,
    ) -> Result<QueueReceipt, AppCommandError> {
        let id = uuid(task_id)?;
        let submission = uuid(submission_id)?;
        if !(1..i64::MAX).contains(&expected_revision) {
            return Err(AppCommandError::invalid_input("Invalid task revision"));
        }
        // The first transaction statement is a write, including replay paths.
        let changed = txn
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task SET status='queued',revision=revision+1,\
                updated_at=? WHERE task_id=? AND revision=? AND (status='draft' \
                OR (status IN ('failed','cancelled') AND EXISTS(SELECT 1 FROM \
                hall_task_attempt a LEFT JOIN hall_task_runtime r ON \
                r.attempt_id=a.attempt_id WHERE a.task_id=hall_task.task_id AND \
                a.attempt_number=(SELECT MAX(attempt_number) FROM \
                hall_task_attempt WHERE task_id=hall_task.task_id) AND \
                ((r.stage='terminal' AND r.exit_confirmed_at IS NOT NULL) OR \
                r.stage='rejected' OR (a.status='cancelled' AND NOT \
                EXISTS(SELECT 1 FROM hall_task_dispatch d WHERE \
                d.attempt_id=a.attempt_id)))))) AND NOT EXISTS(SELECT 1 FROM \
                hall_task_attempt WHERE submission_id=?)",
                vec![
                    Utc::now().into(),
                    id.clone().into(),
                    expected_revision.into(),
                    submission.clone().into(),
                ],
            ))
            .await
            .map_err(storage_error)?;
        if changed.rows_affected() == 0 {
            let old = receipt(txn, &submission).await?.ok_or_else(conflict)?;
            if old.task_id != id || old.task_revision != expected_revision + 1 {
                return Err(conflict());
            }
            return Ok(old);
        }
        let task = by_id(txn, &id).await?;
        validate(&task)?;
        let saved = snapshot_reference(txn, &task.member_id, &task.snapshot_id).await?;
        if saved.snapshot_version != task.snapshot_version
            || saved.member_revision != task.snapshot_member_revision
            || saved.project_id != task.project_id
        {
            return Err(conflict());
        }
        let number = txn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT COALESCE(MAX(attempt_number),0) AS n FROM \
                hall_task_attempt WHERE task_id=?",
                vec![id.clone().into()],
            ))
            .await
            .map_err(storage_error)?
            .ok_or_else(conflict)?
            .try_get::<i64>("", "n")
            .map_err(storage_error)?
            .checked_add(1)
            .ok_or_else(conflict)?;
        let attempt = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        txn.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO hall_task_attempt(attempt_id,submission_id,task_id,\
                member_id,snapshot_id,snapshot_version,attempt_number,status,\
                revision,created_at,updated_at) VALUES(?,?,?,?,?,?,?,'queued',1,\
                ?,?)",
            vec![
                attempt.clone().into(),
                submission.into(),
                id.clone().into(),
                task.member_id.into(),
                task.snapshot_id.into(),
                task.snapshot_version.into(),
                number.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(storage_error)?;
        event(txn, &id, &attempt, task.revision, "task_queued").await?;
        Ok(QueueReceipt {
            attempt_id: attempt,
            task_id: id,
            task_revision: task.revision,
        })
    }

    pub async fn cancel_queued(&self, task_id: &str, revision: i64) -> Result<(), AppCommandError> {
        let id = uuid(task_id)?;
        let tx = self.db.begin().await.map_err(storage_error)?;
        if tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task SET status='cancelled',revision=revision+1,\
            updated_at=? WHERE task_id=? AND revision=? AND status='queued' \
            AND EXISTS(SELECT 1 FROM hall_task_attempt a WHERE \
            a.task_id=hall_task.task_id AND a.status='queued' AND NOT \
            EXISTS(SELECT 1 FROM hall_task_dispatch d WHERE \
            d.attempt_id=a.attempt_id))",
                vec![Utc::now().into(), id.clone().into(), revision.into()],
            ))
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(conflict());
        }
        let attempt = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT attempt_id FROM hall_task_attempt WHERE task_id=? AND \
            status='queued' ORDER BY attempt_number DESC LIMIT 1",
                vec![id.clone().into()],
            ))
            .await
            .map_err(storage_error)?
            .ok_or_else(conflict)?
            .try_get::<String>("", "attempt_id")
            .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_attempt SET status='cancelled',\
            revision=revision+1,updated_at=? WHERE attempt_id=?",
            vec![Utc::now().into(), attempt.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        event(&tx, &id, &attempt, revision + 1, "queued_cancelled").await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }

    pub async fn claim_next(&self) -> Result<Option<Claim>, AppCommandError> {
        self.claim(None, false, None).await
    }
    pub(super) async fn claim_runtime(
        &self,
        reviewed: bool,
        kind: &str,
    ) -> Result<Option<Claim>, AppCommandError> {
        self.claim(Some(kind), reviewed, None).await
    }
    pub(super) async fn claim_task(
        &self,
        task: &str,
        reviewed: bool,
        kind: &str,
    ) -> Result<Option<Claim>, AppCommandError> {
        self.claim(Some(kind), reviewed, Some(task)).await
    }
    async fn claim(
        &self,
        runtime: Option<&str>,
        reviewed: bool,
        task: Option<&str>,
    ) -> Result<Option<Claim>, AppCommandError> {
        let token = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let txn = self.db.begin().await.map_err(storage_error)?;
        let review_filter = if reviewed {
            " AND EXISTS(SELECT 1 FROM hall_task_review v WHERE \
            v.attempt_id=a.attempt_id) "
        } else {
            ""
        };
        let selected = txn
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                format!(
                    "INSERT INTO hall_task_dispatch(attempt_id,claim_id,host_epoch,\
                created_at) SELECT a.attempt_id,?,?,? FROM hall_task_attempt a \
                JOIN hall_task t ON t.task_id=a.task_id WHERE a.status='queued' \
                AND t.status='queued' AND (? IS NULL OR a.task_id=?) \
                {review_filter} AND NOT EXISTS(SELECT 1 FROM hall_task_dispatch \
                d WHERE d.attempt_id=a.attempt_id) AND NOT EXISTS(SELECT 1 FROM \
                hall_task_attempt b LEFT JOIN hall_task_dispatch bd ON \
                bd.attempt_id=b.attempt_id WHERE b.member_id=a.member_id AND \
                (b.status IN ('starting','running','awaiting_permission',\
                'cancel_requested') OR (b.status='interrupted' AND \
                bd.review_required=1))) AND (SELECT COUNT(*) FROM \
                hall_task_attempt c LEFT JOIN hall_task_dispatch cd ON \
                cd.attempt_id=c.attempt_id WHERE c.status IN ('starting',\
                'running','awaiting_permission','cancel_requested') OR \
                (c.status='interrupted' AND cd.review_required=1)) < ? ORDER BY \
                a.created_at,a.attempt_id LIMIT 1"
                ),
                vec![
                    token.clone().into(),
                    self.host_epoch.clone().into(),
                    now.into(),
                    task.map(str::to_owned).into(),
                    task.map(str::to_owned).into(),
                    self.max_active.into(),
                ],
            ))
            .await
            .map_err(storage_error)?;
        if selected.rows_affected() == 0 {
            txn.commit().await.map_err(storage_error)?;
            return Ok(None);
        }
        #[derive(FromQueryResult)]
        struct Pick {
            attempt_id: String,
            task_id: String,
        }
        let pick = Pick::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT a.attempt_id,a.task_id FROM hall_task_attempt a JOIN \
                hall_task_dispatch d ON d.attempt_id=a.attempt_id WHERE \
                d.claim_id=?",
            vec![token.clone().into()],
        ))
        .one(&txn)
        .await
        .map_err(storage_error)?
        .ok_or_else(conflict)?;
        if let Some(kind) = runtime {
            txn.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO hall_task_runtime(attempt_id,claim_id,intent_id,\
                    intent_at,stage,runtime_kind) VALUES(?,?,?,?,'prepared',?)",
                vec![
                    pick.attempt_id.clone().into(),
                    token.clone().into(),
                    uuid::Uuid::new_v4().to_string().into(),
                    now.into(),
                    kind.to_owned().into(),
                ],
            ))
            .await
            .map_err(storage_error)?;
        }
        if txn
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_attempt SET status='starting',\
                revision=revision+1,updated_at=? WHERE attempt_id=? AND \
                status='queued'",
                vec![now.into(), pick.attempt_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(conflict());
        }
        txn.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task SET revision=revision+1,updated_at=? WHERE \
                task_id=? AND status='queued'",
            vec![now.into(), pick.task_id.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        let task = by_id(&txn, &pick.task_id).await?;
        event(
            &txn,
            &pick.task_id,
            &pick.attempt_id,
            task.revision,
            "attempt_claimed",
        )
        .await?;
        txn.commit().await.map_err(storage_error)?;
        Ok(Some(Claim {
            attempt_id: pick.attempt_id,
            task_id: pick.task_id,
            claim_id: token,
            host_epoch: self.host_epoch.clone(),
        }))
    }

    pub async fn is_current(&self, claim: &Claim) -> Result<bool, AppCommandError> {
        if self.host_epoch != claim.host_epoch {
            return Ok(false);
        }
        Ok(self
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT 1 AS valid FROM hall_task_dispatch d JOIN \
                hall_task_attempt a ON a.attempt_id=d.attempt_id WHERE \
                d.claim_id=? AND d.host_epoch=? AND d.attempt_id=? AND \
                d.review_required=0 AND a.status='starting'",
                vec![
                    claim.claim_id.clone().into(),
                    self.host_epoch.clone().into(),
                    claim.attempt_id.clone().into(),
                ],
            ))
            .await
            .map_err(storage_error)?
            .is_some())
    }

    /// Startup bookkeeping only: no retries, prompts, kills or permission grants.
    /// Unknown work remains blocking capacity, not silently treated as exited.
    pub async fn reconcile_previous_boots(&self) -> Result<usize, AppCommandError> {
        let txn = self.db.begin().await.map_err(storage_error)?;
        txn.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_dispatch SET review_required=1 WHERE \
                host_epoch<>? AND attempt_id IN(SELECT attempt_id FROM \
                hall_task_attempt WHERE status IN ('starting','running',\
                'awaiting_permission','cancel_requested'))",
            vec![self.host_epoch.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        #[derive(FromQueryResult)]
        struct Pending {
            attempt_id: String,
            task_id: String,
        }
        let pending = Pending::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT a.attempt_id,a.task_id FROM hall_task_attempt a JOIN \
                hall_task_dispatch d ON d.attempt_id=a.attempt_id WHERE \
                d.host_epoch<>? AND d.review_required=1 AND a.status IN \
                ('starting','running','awaiting_permission','cancel_requested')",
            vec![self.host_epoch.clone().into()],
        ))
        .all(&txn)
        .await
        .map_err(storage_error)?;
        for row in &pending {
            let now = Utc::now();
            txn.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_runtime SET stage='needs_review' WHERE \
                attempt_id=?",
                vec![row.attempt_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?;
            txn.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_attempt SET status='interrupted',\
                    revision=revision+1,updated_at=? WHERE attempt_id=?",
                vec![now.into(), row.attempt_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?;
            txn.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task SET status='interrupted',revision=revision+1,\
                    updated_at=? WHERE task_id=?",
                vec![now.into(), row.task_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?;
            let task = by_id(&txn, &row.task_id).await?;
            event(
                &txn,
                &row.task_id,
                &row.attempt_id,
                task.revision,
                "attempt_interrupted",
            )
            .await?;
        }
        txn.commit().await.map_err(storage_error)?;
        Ok(pending.len())
    }
}
