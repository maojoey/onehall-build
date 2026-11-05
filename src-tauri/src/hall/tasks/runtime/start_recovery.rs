//! Durable startup ledger. No PID discovery, prompt replay or old epoch mutation.
use super::{types::ExecutionOwner, Coordinator};
use crate::{
    app_error::AppCommandError,
    hall::tasks::{
        queue::Claim,
        store::{by_id, conflict, storage_error},
        TaskDetail,
    },
};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Serialize;

/// Host-created recovery identity; deliberately not deserializable.
pub struct StartIdentity {
    pub(super) run: String,
    pub(super) owner: ExecutionOwner,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AbortStatus {
    NotStarted,
    ExitVerified,
    ExitUnverified,
}
impl AbortStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::ExitVerified => "exit_verified",
            Self::ExitUnverified => "exit_unverified",
        }
    }
}
#[derive(Serialize, FromQueryResult)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub kind: String,
    pub reason: String,
    pub resolved_at: DateTime<Utc>,
}
#[derive(Serialize, FromQueryResult)]
#[serde(rename_all = "camelCase")]
pub struct StartObservation {
    pub attempt_id: String,
    pub phase: String,
    pub failure: Option<String>,
    pub failure_code: Option<String>,
}
fn denied() -> AppCommandError {
    AppCommandError::permission_denied("Local operator recovery is not authorized for this attempt")
}
async fn verify_record<C: ConnectionTrait>(
    db: &C,
    claim: &Claim,
    run: Option<&str>,
) -> Result<(), AppCommandError> {
    if let Some(row)=db.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,
        "SELECT task_id,claim_id,host_epoch,run_id FROM hall_task_start_recovery WHERE attempt_id=?",
        vec![claim.attempt_id.clone().into()],
    )).await.map_err(storage_error)? {
        if row.try_get::<String>("","task_id").map_err(storage_error)?!=claim.task_id
            || row.try_get::<String>("","claim_id").map_err(storage_error)?!=claim.claim_id
            || row.try_get::<String>("","host_epoch").map_err(storage_error)?!=claim.host_epoch
            || row.try_get::<Option<String>>("","run_id").map_err(storage_error)?.as_deref()!=run
        { return Err(denied()); }
    }
    Ok(())
}
impl Coordinator {
    pub(super) async fn record_start(&self, c: &Claim) -> Result<(), AppCommandError> {
        self.database().execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "INSERT INTO hall_task_start_recovery(attempt_id,task_id,claim_id,host_epoch,phase,updated_at) VALUES(?,?,?,?,'preparing',?)",
            vec![c.attempt_id.clone().into(),c.task_id.clone().into(),c.claim_id.clone().into(),c.host_epoch.clone().into(),Utc::now().into()],
        )).await.map_err(storage_error)?;
        Ok(())
    }
    pub(super) async fn start_phase(
        &self,
        c: &Claim,
        run: &str,
        phase: &str,
    ) -> Result<(), AppCommandError> {
        let changed = self.database().execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "UPDATE hall_task_start_recovery SET run_id=?,phase=?,updated_at=? WHERE attempt_id=? AND claim_id=? AND host_epoch=? AND writes_revoked=0",
            vec![run.to_owned().into(),phase.to_owned().into(),Utc::now().into(),c.attempt_id.clone().into(),c.claim_id.clone().into(),c.host_epoch.clone().into()],
        )).await.map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        Ok(())
    }
    pub(super) async fn failed_start(
        &self,
        c: &Claim,
        status: AbortStatus,
        code: &str,
        observation: Option<super::ExitObservation>,
    ) -> Result<(), AppCommandError> {
        let tx = self.database().begin().await.map_err(storage_error)?;
        super::lifecycle::guard(&tx, &self.host, c).await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "UPDATE hall_task_start_recovery SET phase='aborted',failure=?,failure_code=?,writes_revoked=1,updated_at=? WHERE attempt_id=?",
            vec![status.as_str().into(),code.to_owned().into(),Utc::now().into(),c.attempt_id.clone().into()],
        )).await.map_err(storage_error)?;
        if let Some(observation) = observation {
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
                "INSERT INTO hall_task_exit_check(attempt_id,scope_hash,lease_id,report_json) VALUES(?,?,?,?) ON CONFLICT(attempt_id) DO NOTHING",
                vec![c.attempt_id.clone().into(),observation.scope_hash.clone().into(),observation.lease_id.clone().into(),serde_json::to_string(&observation).map_err(|_| denied())?.into()],
            )).await.map_err(storage_error)?;
        }
        let unknown = status == AbortStatus::ExitUnverified;
        let confirmed = (status == AbortStatus::ExitVerified).then(Utc::now);
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_runtime SET stage=?,exit_confirmed_at=? WHERE attempt_id=?",
            vec![
                match status {
                    AbortStatus::ExitUnverified => "needs_review",
                    AbortStatus::NotStarted => "rejected",
                    AbortStatus::ExitVerified => "terminal",
                }
                .into(),
                confirmed.into(),
                c.attempt_id.clone().into(),
            ],
        ))
        .await
        .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_dispatch SET review_required=? WHERE attempt_id=?",
            vec![unknown.into(), c.attempt_id.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        let state = if unknown { "interrupted" } else { "failed" };
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "UPDATE hall_task_attempt SET status=?,finished_at=CASE WHEN started_at IS NOT NULL THEN ? ELSE NULL END,revision=revision+1,updated_at=? WHERE attempt_id=?",
            vec![state.into(),confirmed.into(),Utc::now().into(),c.attempt_id.clone().into()],
        )).await.map_err(storage_error)?;
        super::lifecycle::event(&tx, c, "host:start_failed", "start_failed", state).await?;
        tx.commit().await.map_err(storage_error)
    }
    /// Only rechecks an existing host-owned domain. A missing domain proves nothing.
    pub(super) async fn recover_persisted(
        &self,
        task: &str,
    ) -> Result<TaskDetail, AppCommandError> {
        let d = crate::hall::tasks::get_task(self.database(), task.into()).await?;
        let a = d.attempts.last().ok_or_else(denied)?;
        let runtime = d.runtimes.last().ok_or_else(denied)?;
        if d.task.status != "interrupted" || runtime.stage != "needs_review" {
            return Err(denied());
        }
        let row = self.database().query_one(Statement::from_sql_and_values(DbBackend::Sqlite,
            "SELECT COALESCE(r.run_id,s.run_id) AS run_id,d.claim_id,d.host_epoch FROM hall_task_runtime r JOIN hall_task_dispatch d ON d.attempt_id=r.attempt_id AND d.claim_id=r.claim_id LEFT JOIN hall_task_start_recovery s ON s.attempt_id=r.attempt_id WHERE r.attempt_id=?",
            vec![a.attempt_id.clone().into()],
        )).await.map_err(storage_error)?.ok_or_else(denied)?;
        let claim: String = row.try_get("", "claim_id").map_err(storage_error)?;
        let epoch: String = row.try_get("", "host_epoch").map_err(storage_error)?;
        let run: Option<String> = row.try_get("", "run_id").map_err(storage_error)?;
        let recovery_claim = Claim {
            task_id: task.into(),
            attempt_id: a.attempt_id.clone(),
            claim_id: claim.clone(),
            host_epoch: epoch.clone(),
        };
        verify_record(self.database(), &recovery_claim, run.as_deref()).await?;
        let identity = run.as_ref().map(|run| StartIdentity {
            run: run.clone(),
            owner: ExecutionOwner {
                member: d.task.member_id.clone(),
                task: task.into(),
                attempt: a.attempt_id.clone(),
                snapshot: a.snapshot_id.clone(),
                claim: claim.clone(),
                epoch: epoch.clone(),
            },
        });
        let status = match &identity {
            Some(id) => self
                .port
                .recover_start(id)
                .await
                .unwrap_or(AbortStatus::ExitUnverified),
            None => AbortStatus::ExitUnverified,
        };
        let observation = match &identity {
            Some(id) => self.port.start_observation(id).await,
            None => None,
        };
        if observation.as_ref().is_some_and(|o| {
            o.attempt_id != a.attempt_id
                || identity
                    .as_ref()
                    .is_none_or(|i| o.scope_hash != i.owner.digest())
        }) {
            return Err(denied());
        }
        let tx = self.database().begin().await.map_err(storage_error)?;
        let changed=tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "UPDATE hall_task_dispatch SET review_required=review_required WHERE attempt_id=? AND claim_id=? AND host_epoch=? AND review_required=1",
            vec![a.attempt_id.clone().into(),claim.clone().into(),epoch.clone().into()],
        )).await.map_err(storage_error)?;
        if changed.rows_affected() != 1 || by_id(&tx, task).await?.revision != d.task.revision {
            return Err(conflict());
        }
        verify_record(&tx, &recovery_claim, run.as_deref()).await?;
        let now = Utc::now();
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "INSERT INTO hall_task_start_recovery(attempt_id,task_id,claim_id,host_epoch,run_id,phase,updated_at) VALUES(?,?,?,?,?,'aborted',?) ON CONFLICT(attempt_id) DO NOTHING",
            vec![a.attempt_id.clone().into(),task.to_owned().into(),claim.clone().into(),epoch.clone().into(),run.clone().into(),now.into()],
        )).await.map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "UPDATE hall_task_start_recovery SET phase='aborted',failure=?,failure_code='recovery_checked',writes_revoked=1,updated_at=? WHERE attempt_id=?",
            vec![status.as_str().into(),now.into(),a.attempt_id.clone().into()],
        )).await.map_err(storage_error)?;
        if let Some(observation) = &observation {
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
                "INSERT INTO hall_task_exit_check(attempt_id,scope_hash,lease_id,report_json) VALUES(?,?,?,?) ON CONFLICT(attempt_id) DO UPDATE SET report_json=excluded.report_json WHERE scope_hash=excluded.scope_hash AND lease_id=excluded.lease_id",
                vec![a.attempt_id.clone().into(),observation.scope_hash.clone().into(),observation.lease_id.clone().into(),serde_json::to_string(observation).map_err(|_| denied())?.into()],
            )).await.map_err(storage_error)?;
        }
        if status != AbortStatus::ExitUnverified {
            let confirmed = (status == AbortStatus::ExitVerified).then_some(now);
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
                "UPDATE hall_task_runtime SET stage=?,exit_confirmed_at=? WHERE attempt_id=? AND stage='needs_review'",
                vec![if confirmed.is_some() {"terminal"} else {"rejected"}.into(),confirmed.into(),a.attempt_id.clone().into()],
            )).await.map_err(storage_error)?;
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
                "UPDATE hall_task_attempt SET status='failed',finished_at=?,revision=revision+1,updated_at=? WHERE attempt_id=?",
                vec![confirmed.filter(|_| a.started_at.is_some()).into(),now.into(),a.attempt_id.clone().into()],
            )).await.map_err(storage_error)?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_dispatch SET review_required=0 WHERE attempt_id=?",
                vec![a.attempt_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?;
            super::lifecycle::event(
                &tx,
                &Claim {
                    task_id: task.into(),
                    attempt_id: a.attempt_id.clone(),
                    claim_id: claim,
                    host_epoch: epoch,
                },
                "host:recovered_exit",
                "startup_recovered",
                "failed",
            )
            .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        if let Some(run) = run {
            self.port.revoke_run(&run);
            self.revoke_exchange(task, &a.attempt_id, &run).await;
        }
        crate::hall::tasks::get_task(self.database(), task.into()).await
    }
    pub(super) async fn resolve_orphan(
        &self,
        task: &str,
        attempt: &str,
        revision: i64,
        ack: bool,
        reason: &str,
    ) -> Result<TaskDetail, AppCommandError> {
        if !ack {
            return Err(denied());
        }
        let reason = reason.trim();
        if reason.is_empty()
            || reason.chars().count() > 1000
            || reason.chars().any(char::is_control)
        {
            return Err(AppCommandError::invalid_input(
                "Recovery reason must contain 1–1000 printable characters",
            ));
        }
        let tx = self.database().begin().await.map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task SET revision=revision WHERE task_id=?",
            vec![task.to_owned().into()],
        ))
        .await
        .map_err(storage_error)?;
        let t = by_id(&tx, task).await?;
        let row = tx.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,
            "SELECT a.attempt_id,d.claim_id,d.host_epoch,COALESCE(r.run_id,s.run_id) AS run_id,r.stage,r.exit_confirmed_at,s.resolved_revision,s.resolution_reason FROM hall_task_attempt a JOIN hall_task_dispatch d ON d.attempt_id=a.attempt_id JOIN hall_task_runtime r ON r.attempt_id=a.attempt_id AND r.claim_id=d.claim_id LEFT JOIN hall_task_start_recovery s ON s.attempt_id=a.attempt_id WHERE a.task_id=? ORDER BY a.attempt_number DESC LIMIT 1",
            vec![task.to_owned().into()],
        )).await.map_err(storage_error)?.ok_or_else(denied)?;
        let current: String = row.try_get("", "attempt_id").map_err(storage_error)?;
        if current != attempt {
            return Err(conflict());
        }
        let old_revision: Option<i64> = row
            .try_get("", "resolved_revision")
            .map_err(storage_error)?;
        if let Some(old) = old_revision {
            let old_reason: Option<String> = row
                .try_get("", "resolution_reason")
                .map_err(storage_error)?;
            if old != revision || old_reason.as_deref() != Some(reason) {
                return Err(conflict());
            }
            tx.commit().await.map_err(storage_error)?;
            return crate::hall::tasks::get_task(self.database(), task.into()).await;
        }
        if t.revision != revision {
            return Err(conflict());
        }
        if t.status != "interrupted"
            || row.try_get::<String>("", "stage").map_err(storage_error)? != "needs_review"
            || row
                .try_get::<Option<String>>("", "exit_confirmed_at")
                .map_err(storage_error)?
                .is_some()
        {
            return Err(denied());
        }
        let claim: String = row.try_get("", "claim_id").map_err(storage_error)?;
        let epoch: String = row.try_get("", "host_epoch").map_err(storage_error)?;
        let run: Option<String> = row.try_get("", "run_id").map_err(storage_error)?;
        let now = Utc::now();
        verify_record(
            &tx,
            &Claim {
                task_id: task.into(),
                attempt_id: attempt.into(),
                claim_id: claim.clone(),
                host_epoch: epoch.clone(),
            },
            run.as_deref(),
        )
        .await?;
        // Backfill only from the persistent host record; never invent a run or Binding.
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "INSERT INTO hall_task_start_recovery(attempt_id,task_id,claim_id,host_epoch,run_id,phase,updated_at) VALUES(?,?,?,?,?,'aborted',?) ON CONFLICT(attempt_id) DO NOTHING",
            vec![attempt.to_owned().into(),task.to_owned().into(),claim.clone().into(),epoch.clone().into(),run.clone().into(),now.into()],
        )).await.map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "UPDATE hall_task_start_recovery SET writes_revoked=1,resolved_revision=?,resolution_reason=?,resolution_actor='authenticated_local_operator',resolved_at=?,updated_at=? WHERE attempt_id=?",
            vec![revision.into(),reason.to_owned().into(),now.into(),now.into(),attempt.to_owned().into()],
        )).await.map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_runtime SET stage='terminal' WHERE attempt_id=?",
            vec![attempt.to_owned().into()],
        ))
        .await
        .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_dispatch SET review_required=0 WHERE attempt_id=?",
            vec![attempt.to_owned().into()],
        ))
        .await
        .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_attempt SET revision=revision+1,updated_at=? WHERE attempt_id=?",
            vec![now.into(), attempt.to_owned().into()],
        ))
        .await
        .map_err(storage_error)?;
        super::lifecycle::event(
            &tx,
            &Claim {
                task_id: task.into(),
                attempt_id: attempt.into(),
                claim_id: claim,
                host_epoch: epoch,
            },
            "human:resolve_orphan",
            "human_orphan_resolution",
            "interrupted",
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        if let Some(run) = run {
            self.port.revoke_run(&run);
            self.revoke_exchange(task, attempt, &run).await;
        }
        crate::hall::tasks::get_task(self.database(), task.into()).await
    }
}
