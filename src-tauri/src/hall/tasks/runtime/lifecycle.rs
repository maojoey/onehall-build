use super::{port::RuntimePort, types::*};
use crate::app_error::AppCommandError;
use crate::hall::members::snapshot_launch_profile;
use crate::hall::tasks::{
    queue::{Claim, QueueHost},
    store::{by_id, conflict, storage_error},
};
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult,
    Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct Coordinator {
    pub(super) host: QueueHost,
    pub(super) port: Arc<dyn RuntimePort>,
    enabled: bool,
    review_required: bool,
    runtime_kind: &'static str,
    gate: Option<Arc<super::admission::Gate>>,
    /// Attached after construction; `None` leaves every task on the plain path.
    exchange: std::sync::OnceLock<crate::hall::exchange::Exchange>,
}
fn denied() -> AppCommandError {
    AppCommandError::permission_denied("Team execution admission is closed or no longer valid")
}
async fn row<C: ConnectionTrait>(db: &C, id: &str) -> Result<RuntimeRow, AppCommandError> {
    RuntimeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT r.*,a.member_id,a.snapshot_id FROM hall_task_runtime r \
        JOIN hall_task_attempt a ON a.attempt_id=r.attempt_id WHERE \
        r.attempt_id=?",
        vec![id.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)?
    .ok_or_else(denied)
}
pub(super) async fn guard(
    db: &DatabaseTransaction,
    host: &QueueHost,
    c: &Claim,
) -> Result<RuntimeRow, AppCommandError> {
    guard_mode(db, host, c, false).await
}
async fn guard_mode(
    db: &DatabaseTransaction,
    host: &QueueHost,
    c: &Claim,
    recovery: bool,
) -> Result<RuntimeRow, AppCommandError> {
    if c.host_epoch != host.host_epoch {
        return Err(denied());
    }
    // First statement takes a write lock; recheck fences in the same transaction.
    if db
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_dispatch SET review_required=review_required \
            WHERE attempt_id=? AND claim_id=? AND host_epoch=? AND \
            review_required=?",
            vec![
                c.attempt_id.clone().into(),
                c.claim_id.clone().into(),
                c.host_epoch.clone().into(),
                recovery.into(),
            ],
        ))
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(denied());
    }
    let r = row(db, &c.attempt_id).await?;
    if r.attempt_id != c.attempt_id || r.claim_id != c.claim_id {
        return Err(denied());
    }
    Ok(r)
}
pub(super) async fn event(
    tx: &DatabaseTransaction,
    c: &Claim,
    key: &str,
    action: &str,
    task_status: &str,
) -> Result<(), AppCommandError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_task SET status=?,revision=revision+1,updated_at=? \
            WHERE task_id=?",
        vec![
            task_status.to_owned().into(),
            Utc::now().into(),
            c.task_id.clone().into(),
        ],
    ))
    .await
    .map_err(storage_error)?;
    let task = by_id(tx, &c.task_id).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_task_event(event_id,task_id,attempt_id,\
            task_revision,action,source_key,created_at) VALUES(?,?,?,?,?,?,\
            ?)",
        vec![
            uuid::Uuid::new_v4().to_string().into(),
            c.task_id.clone().into(),
            c.attempt_id.clone().into(),
            task.revision.into(),
            action.to_owned().into(),
            key.to_owned().into(),
            Utc::now().into(),
        ],
    ))
    .await
    .map_err(storage_error)?;
    Ok(())
}
fn bound(r: &RuntimeRow, b: &Binding, c: &Claim) -> bool {
    b.owner.member == r.member_id
        && b.owner.snapshot == r.snapshot_id
        && b.owner.task == c.task_id
        && b.owner.attempt == c.attempt_id
        && b.owner.claim == c.claim_id
        && b.owner.epoch == c.host_epoch
        && r.run_id.as_deref() == Some(&b.run_id)
        && r.connection_id.as_deref() == Some(&b.connection_id)
        && r.conversation_id == Some(b.conversation_id)
        && r.external_session_id.as_deref() == Some(&b.external_session_id)
}
impl Coordinator {
    /// No caller can open this gate using JSON, engine Connected or a claim.
    pub fn production_closed(host: QueueHost, port: Arc<dyn RuntimePort>) -> Self {
        Self {
            host,
            port,
            enabled: false,
            review_required: true,
            runtime_kind: "cli",
            gate: None,
            exchange: std::sync::OnceLock::new(),
        }
    }
    #[cfg(test)]
    pub(super) fn fixture(host: QueueHost, port: Arc<dyn RuntimePort>) -> Self {
        Self {
            host,
            port,
            enabled: true,
            review_required: false,
            runtime_kind: "fixture",
            gate: None,
            exchange: std::sync::OnceLock::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn reviewed_fixture(host: QueueHost, port: Arc<dyn RuntimePort>) -> Self {
        Self {
            host,
            port,
            enabled: true,
            review_required: true,
            runtime_kind: "fixture",
            gate: None,
            exchange: std::sync::OnceLock::new(),
        }
    }
    pub(crate) fn controlled(
        host: QueueHost,
        port: Arc<dyn RuntimePort>,
        gate: Arc<super::admission::Gate>,
    ) -> Self {
        Self {
            host,
            port,
            enabled: false,
            review_required: true,
            runtime_kind: "cli",
            gate: Some(gate),
            exchange: std::sync::OnceLock::new(),
        }
    }
    pub(crate) fn attach_exchange(&self, exchange: crate::hall::exchange::Exchange) {
        let _ = self.exchange.set(exchange);
    }
    /// Revocation failures are logged, never silently dropped: a task keeps
    /// no write authority once its run is released, so a failure needs attention.
    pub(super) async fn revoke_exchange(&self, task: &str, attempt: &str, run: &str) {
        if let Some(exchange) = self.exchange.get() {
            if let Err(error) = exchange.revoke_run(&self.host.db, task, attempt, run).await {
                tracing::warn!(%error, task, attempt, "exchange write revocation failed");
            }
        }
    }
    pub(crate) async fn dispatch_task(
        &self,
        task: &str,
    ) -> Result<Option<(Claim, Binding)>, AppCommandError> {
        self.dispatch(Some(task)).await
    }
    pub fn database(&self) -> &DatabaseConnection {
        &self.host.db
    }
    pub async fn dispatch_next(&self) -> Result<Option<(Claim, Binding)>, AppCommandError> {
        self.dispatch(None).await
    }
    async fn dispatch(
        &self,
        task: Option<&str>,
    ) -> Result<Option<(Claim, Binding)>, AppCommandError> {
        if !self.enabled && !task.is_some_and(|id| self.gate.as_ref().is_some_and(|g| g.allows(id)))
        {
            return Err(denied());
        }
        let claim = match task {
            Some(id) => {
                self.host
                    .claim_task(id, self.review_required, self.runtime_kind)
                    .await?
            }
            None => {
                self.host
                    .claim_runtime(self.review_required, self.runtime_kind)
                    .await?
            }
        };
        let Some(c) = claim else { return Ok(None) };
        use super::start_recovery::{AbortStatus, StartIdentity};
        let mut reserved: Option<String> = None;
        let mut identity = None;
        let mut may_have_started = false;
        let mut code = "preparation_failed";
        let outcome: Result<Binding, AppCommandError> = async {
            self.record_start(&c).await?;
            let plan = self.plan(&c).await?;
            code = "preflight_failed";
            self.port.validate_start(&plan).await?;
            code = "reservation_failed";
            let run = self.port.reserve_for(&plan).await?;
            reserved = Some(run.clone());
            // Materialize inputs/outputs for the real attempt before any model
            // starts; unverified input bytes keep the claim from launching.
            if let Some(exchange) = self.exchange.get() {
                code = "exchange_bind_failed";
                exchange
                    .bind_run(&self.host.db, &plan.task_id, &plan.attempt_id, &run)
                    .await
                    .map_err(|_| denied())?;
            }
            identity = Some(StartIdentity {
                run: run.clone(),
                owner: plan.owner.clone(),
            });
            code = "intent_commit_failed";
            self.start_phase(&c, &run, "starting").await?;
            {
                let tx = self.host.db.begin().await.map_err(storage_error)?;
                let r = guard(&tx, &self.host, &c).await?;
                if r.stage != "prepared" {
                    return Err(denied());
                }
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "UPDATE hall_task_runtime SET stage='dispatching',run_id=? WHERE \
              attempt_id=?",
                    vec![run.clone().into(), c.attempt_id.clone().into()],
                ))
                .await
                .map_err(storage_error)?;
                event(&tx, &c, "host:dispatch", "dispatch_intent", "queued").await?;
                tx.commit().await.map_err(storage_error)?;
            }
            code = "start_failed";
            may_have_started = true;
            let b = self.port.start(&run, &plan).await?;
            code = "binding_invalid";
            self.start_phase(&c, &run, "binding").await?;
            if b.owner != plan.owner
                || b.lease_id.is_empty()
                || b.run_id != run
                || b.connection_id.is_empty()
                || b.conversation_id < 1
                || b.external_session_id.is_empty()
            {
                return Err(denied());
            }
            code = "binding_commit_failed";
            let tx = self.host.db.begin().await.map_err(storage_error)?;
            let r = guard(&tx, &self.host, &c).await?;
            if r.stage != "dispatching" || r.run_id.as_deref() != Some(&run) {
                return Err(denied());
            }
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_runtime SET stage='running',connection_id=?,\
            conversation_id=?,external_session_id=? WHERE attempt_id=?",
                vec![
                    b.connection_id.clone().into(),
                    b.conversation_id.into(),
                    b.external_session_id.clone().into(),
                    c.attempt_id.clone().into(),
                ],
            ))
            .await
            .map_err(storage_error)?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_attempt SET status='running',conversation_id=?,\
            started_at=?,revision=revision+1,updated_at=? WHERE attempt_id=?",
                vec![
                    b.conversation_id.into(),
                    b.started_at.into(),
                    Utc::now().into(),
                    c.attempt_id.clone().into(),
                ],
            ))
            .await
            .map_err(storage_error)?;
            event(&tx, &c, "host:bound", "attempt_running", "running").await?;
            tx.commit().await.map_err(storage_error)?;
            code = "activation_failed";
            self.start_phase(&c, &run, "activating").await?;
            self.port.activate(&b).await?;
            code = "activation_commit_failed";
            self.start_phase(&c, &run, "active").await?;
            Ok(b)
        }
        .await;
        let b = match outcome {
            Ok(b) => b,
            Err(_) => {
                let status = if may_have_started {
                    self.port
                        .abort_start(identity.as_ref().expect("reserved identity"))
                        .await
                        .unwrap_or(AbortStatus::ExitUnverified)
                } else {
                    AbortStatus::NotStarted
                };
                if let Some(run) = &reserved {
                    self.port.revoke_run(run);
                    self.revoke_exchange(&c.task_id, &c.attempt_id, run).await;
                }
                let observation = match &identity {
                    Some(id) => self.port.start_observation(id).await,
                    None => None,
                };
                self.failed_start(&c, status, code, observation).await?;
                return Err(denied());
            }
        };
        Ok(Some((c, b)))
    }
    async fn plan(&self, c: &Claim) -> Result<RunPlan, AppCommandError> {
        if self.review_required {
            crate::hall::tasks::review::validate_dispatch(&self.host.db, &c.attempt_id, &c.task_id)
                .await?;
        }
        let task = by_id(&self.host.db, &c.task_id).await?;
        let p = snapshot_launch_profile(&self.host.db, &task.member_id, &task.snapshot_id).await?;
        #[derive(FromQueryResult)]
        struct Current {
            revision: i64,
            cwd: String,
            engine: String,
            assistant_uid: Option<String>,
        }
        let current = Current::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT revision,cwd,engine,assistant_uid FROM hall_member WHERE \
            member_id=?",
            vec![task.member_id.clone().into()],
        ))
        .one(&self.host.db)
        .await
        .map_err(storage_error)?
        .ok_or_else(denied)?;
        if current.revision != p.member_revision
            || current.cwd != p.cwd
            || current.engine != p.engine
            || !crate::hall::engine::supported(&current.engine)
            || current.assistant_uid != p.assistant_uid
            || tokio::fs::canonicalize(&p.cwd)
                .await
                .map_err(|_| denied())?
                .to_str()
                != Some(p.cwd.as_str())
        {
            return Err(denied());
        }
        Ok(RunPlan {
            owner: ExecutionOwner {
                member: p.member_id.clone(),
                task: c.task_id.clone(),
                attempt: c.attempt_id.clone(),
                snapshot: task.snapshot_id.clone(),
                claim: c.claim_id.clone(),
                epoch: c.host_epoch.clone(),
            },
            profile: p,
            task_id: c.task_id.clone(),
            attempt_id: c.attempt_id.clone(),
            snapshot_id: task.snapshot_id,
            description: task.description,
        })
    }
    pub async fn needs_review(&self, c: &Claim) -> Result<(), AppCommandError> {
        let tx = self.host.db.begin().await.map_err(storage_error)?;
        let r = guard(&tx, &self.host, c).await?;
        if matches!(r.stage.as_str(), "terminal" | "rejected") {
            return Err(denied());
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_runtime SET stage='needs_review' WHERE \
            attempt_id=?",
            vec![c.attempt_id.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_dispatch SET review_required=1 WHERE \
            attempt_id=?",
            vec![c.attempt_id.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_attempt SET status='interrupted',\
            revision=revision+1,updated_at=? WHERE attempt_id=?",
            vec![Utc::now().into(), c.attempt_id.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        event(&tx, c, "host:review", "attempt_interrupted", "interrupted").await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn accept_event(
        &self,
        c: &Claim,
        b: &Binding,
        seq: u64,
        e: RuntimeEvent,
    ) -> Result<bool, AppCommandError> {
        if seq == 0 || seq > i64::MAX as u64 {
            return Err(denied());
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&e).map_err(|_| denied())?)
        );
        let tx = self.host.db.begin().await.map_err(storage_error)?;
        let r = guard(&tx, &self.host, c).await?;
        if !bound(&r, b, c) {
            return Err(denied());
        }
        if seq as i64 <= r.last_sequence {
            if seq as i64 == r.last_sequence && r.last_digest.as_deref() != Some(&digest) {
                return Err(conflict());
            }
            return Ok(false);
        }
        if matches!(r.stage.as_str(), "terminal" | "rejected" | "needs_review") {
            return Err(denied());
        }
        if matches!(&e, RuntimeEvent::Observed) {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_runtime SET last_sequence=?,last_digest=? \
                WHERE attempt_id=?",
                vec![
                    (seq as i64).into(),
                    digest.into(),
                    c.attempt_id.clone().into(),
                ],
            ))
            .await
            .map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(true);
        }
        let cancelling = r.cancel_requested_at.is_some();
        let (stage, status, action) = match &e {
            RuntimeEvent::Observed => unreachable!(),
            RuntimeEvent::WaitingInput { key, waiting } => {
                if key.len() > 256 || key.chars().any(char::is_control) {
                    return Err(denied());
                }
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "INSERT INTO hall_task_wait(attempt_id,card_key,waiting) \
                    VALUES(?,?,?) ON CONFLICT(attempt_id,card_key) DO UPDATE SET \
                    waiting=excluded.waiting",
                    vec![
                        c.attempt_id.clone().into(),
                        key.clone().into(),
                        (*waiting).into(),
                    ],
                ))
                .await
                .map_err(storage_error)?;
                let waiting = tx
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Sqlite,
                        "SELECT 1 FROM hall_task_wait WHERE attempt_id=? AND waiting=1",
                        vec![c.attempt_id.clone().into()],
                    ))
                    .await
                    .map_err(storage_error)?
                    .is_some();
                if cancelling {
                    ("cancel_requested", "cancel_requested", "runtime_waiting")
                } else if r.stage == "finishing" {
                    ("finishing", "running", "runtime_waiting")
                } else if waiting {
                    ("waiting_input", "awaiting_permission", "runtime_waiting")
                } else {
                    ("running", "running", "runtime_running")
                }
            }
            RuntimeEvent::CancelAcknowledged => {
                if !cancelling {
                    return Err(denied());
                }
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "UPDATE hall_task_runtime SET \
                    cancel_ack_at=COALESCE(cancel_ack_at,?) WHERE attempt_id=?",
                    vec![Utc::now().into(), c.attempt_id.clone().into()],
                ))
                .await
                .map_err(storage_error)?;
                (
                    "cancel_requested",
                    "cancel_requested",
                    "cancel_acknowledged",
                )
            }
            RuntimeEvent::Result { outcome, body } => {
                if body.len() > 65_536 || *outcome == Outcome::Rejected {
                    return Err(denied());
                }
                let result_id = uuid::Uuid::new_v4().to_string();
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "INSERT INTO hall_task_result(result_id,attempt_id,outcome,body,\
                    created_at) VALUES(?,?,?,?,?) ON CONFLICT(attempt_id) DO NOTHING",
                    vec![
                        result_id.into(),
                        c.attempt_id.clone().into(),
                        outcome.as_str().into(),
                        body.clone().into(),
                        Utc::now().into(),
                    ],
                ))
                .await
                .map_err(storage_error)?;
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "UPDATE hall_task_attempt SET result_id=(SELECT result_id FROM \
                    hall_task_result WHERE attempt_id=?) WHERE attempt_id=?",
                    vec![c.attempt_id.clone().into(), c.attempt_id.clone().into()],
                ))
                .await
                .map_err(storage_error)?;
                (
                    if cancelling {
                        "cancel_requested"
                    } else {
                        "finishing"
                    },
                    if cancelling {
                        "cancel_requested"
                    } else {
                        "running"
                    },
                    "result_received",
                )
            }
        };
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_runtime SET stage=?,last_sequence=?,\
            last_digest=? WHERE attempt_id=?",
            vec![
                stage.into(),
                (seq as i64).into(),
                digest.into(),
                c.attempt_id.clone().into(),
            ],
        ))
        .await
        .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_attempt SET status=?,revision=revision+1,\
            updated_at=? WHERE attempt_id=?",
            vec![
                status.into(),
                Utc::now().into(),
                c.attempt_id.clone().into(),
            ],
        ))
        .await
        .map_err(storage_error)?;
        event(
            &tx,
            c,
            &format!("runtime:{seq}"),
            action,
            if status == "cancel_requested" {
                "running"
            } else if status == "awaiting_permission" {
                "awaiting_permission"
            } else {
                "running"
            },
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(true)
    }
    pub async fn request_cancel(&self, c: &Claim, b: &Binding) -> Result<(), AppCommandError> {
        self.cancel_at(c, b, None).await
    }
    pub(crate) async fn cancel_at(
        &self,
        c: &Claim,
        b: &Binding,
        revision: Option<i64>,
    ) -> Result<(), AppCommandError> {
        {
            let tx = self.host.db.begin().await.map_err(storage_error)?;
            let r = guard(&tx, &self.host, c).await?;
            if let Some(expected) = revision {
                if by_id(&tx, &c.task_id).await?.revision != expected {
                    return Err(conflict());
                }
            }
            if !bound(&r, b, c)
                || matches!(r.stage.as_str(), "terminal" | "needs_review" | "rejected")
            {
                return Err(denied());
            }
            if r.cancel_requested_at.is_none() {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "UPDATE hall_task_runtime SET stage='cancel_requested',\
             cancel_requested_at=? WHERE attempt_id=?",
                    vec![Utc::now().into(), c.attempt_id.clone().into()],
                ))
                .await
                .map_err(storage_error)?;
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "UPDATE hall_task_attempt SET status='cancel_requested',\
             revision=revision+1,updated_at=? WHERE attempt_id=?",
                    vec![Utc::now().into(), c.attempt_id.clone().into()],
                ))
                .await
                .map_err(storage_error)?;
                event(&tx, c, "host:cancel", "cancel_requested", "running").await?;
            }
            tx.commit().await.map_err(storage_error)?;
        }
        self.port.cancel(b).await
    }
    pub async fn confirm_exit(&self, c: &Claim, b: &Binding) -> Result<bool, AppCommandError> {
        self.confirm(c, b, false).await
    }
    pub(crate) async fn recover_exit(
        &self,
        c: &Claim,
        b: &Binding,
    ) -> Result<bool, AppCommandError> {
        self.confirm(c, b, true).await
    }
    async fn confirm(
        &self,
        c: &Claim,
        b: &Binding,
        recovery: bool,
    ) -> Result<bool, AppCommandError> {
        {
            let tx = self.host.db.begin().await.map_err(storage_error)?;
            let r = guard_mode(&tx, &self.host, c, recovery).await?;
            if !bound(&r, b, c)
                || !(if recovery {
                    r.stage == "needs_review"
                } else {
                    matches!(r.stage.as_str(), "finishing" | "cancel_requested")
                })
            {
                return Err(denied());
            }
            tx.commit().await.map_err(storage_error)?;
        }
        let proof = self.port.confirm_exit(b).await?;
        if let Some(report) = self.port.exit_observation(b).await {
            if report.attempt_id != c.attempt_id
                || report.scope_hash != b.owner.digest()
                || report.lease_id != b.lease_id
            {
                return Err(denied());
            }
            let tx = self.host.db.begin().await.map_err(storage_error)?;
            let r = guard_mode(&tx, &self.host, c, recovery).await?;
            if !bound(&r, b, c) {
                return Err(denied());
            }
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO hall_task_exit_check(attempt_id,scope_hash,lease_id,\
                report_json) VALUES(?,?,?,?) ON CONFLICT(attempt_id) DO UPDATE \
                SET report_json=excluded.report_json WHERE \
                scope_hash=excluded.scope_hash AND lease_id=excluded.lease_id",
                vec![
                    c.attempt_id.clone().into(),
                    report.scope_hash.clone().into(),
                    report.lease_id.clone().into(),
                    serde_json::to_string(&report).map_err(|_| denied())?.into(),
                ],
            ))
            .await
            .map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            if !report.complete
                && report.root_reaped
                && matches!(
                    report.coverage.as_str(),
                    "native_observation_only" | "macos_process_group_best_effort"
                )
            {
                if !recovery {
                    self.needs_review(c).await?;
                }
                return Ok(false);
            }
        }
        let Some(proof) = proof else { return Ok(false) };
        if proof.owner != b.owner
            || proof.lease_id != b.lease_id
            || proof.confirmed_at < b.started_at
            || proof.run_id != b.run_id
            || proof.connection_id != b.connection_id
        {
            return Err(denied());
        }
        let tx = self.host.db.begin().await.map_err(storage_error)?;
        let r = guard_mode(&tx, &self.host, c, recovery).await?;
        if !bound(&r, b, c)
            || !(if recovery {
                r.stage == "needs_review"
            } else {
                matches!(r.stage.as_str(), "finishing" | "cancel_requested")
            })
        {
            return Err(denied());
        }
        let result = StoredResult::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT * FROM hall_task_result WHERE attempt_id=?",
            vec![c.attempt_id.clone().into()],
        ))
        .one(&tx)
        .await
        .map_err(storage_error)?;
        let outcome = if r.cancel_requested_at.is_some() {
            "cancelled"
        } else {
            result
                .as_ref()
                .map(|x| x.outcome.as_str())
                .or(if recovery { Some("failed") } else { None })
                .ok_or_else(denied)?
        };
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_runtime SET stage='terminal',\
            exit_confirmed_at=? WHERE attempt_id=?",
            vec![proof.confirmed_at.into(), c.attempt_id.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_attempt SET status=?,finished_at=?,\
            revision=revision+1,updated_at=? WHERE attempt_id=?",
            vec![
                outcome.to_owned().into(),
                proof.confirmed_at.into(),
                Utc::now().into(),
                c.attempt_id.clone().into(),
            ],
        ))
        .await
        .map_err(storage_error)?;
        if recovery {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_dispatch SET review_required=0 WHERE \
            attempt_id=? AND claim_id=?",
                vec![c.attempt_id.clone().into(), c.claim_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?;
        }
        event(&tx, c, "host:exit", "exit_confirmed", outcome).await?;
        tx.commit().await.map_err(storage_error)?;
        self.port.revoke(b);
        self.revoke_exchange(&b.owner.task, &b.owner.attempt, &b.run_id)
            .await;
        Ok(true)
    }
    /// Explicit local operator acceptance of the remaining Mac descendant risk.
    /// This releases scheduling capacity, not a verified-exit certificate.
    pub(crate) async fn resolve_best_effort(
        &self,
        c: &Claim,
        b: &Binding,
        expected_revision: i64,
    ) -> Result<(), AppCommandError> {
        let report = self.port.exit_observation(b).await.ok_or_else(denied)?;
        if report.attempt_id != c.attempt_id
            || report.scope_hash != b.owner.digest()
            || report.lease_id != b.lease_id
            || report.coverage != "macos_process_group_best_effort"
            || report.complete
            || !report.root_reaped
            || report.root_birth_hash.is_none()
            || report.known_processes == 0
            || report.known_processes != report.known_reaped
            || report.pending_spawns != 0
            || report.reason != "descendant_coverage_unconfirmed"
        {
            return Err(denied());
        }
        let tx = self.host.db.begin().await.map_err(storage_error)?;
        let runtime = guard_mode(&tx, &self.host, c, true).await?;
        let task = by_id(&tx, &c.task_id).await?;
        if !bound(&runtime, b, c)
            || runtime.stage != "needs_review"
            || task.status != "interrupted"
            || task.revision != expected_revision
        {
            return Err(denied());
        }
        let changed = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_runtime SET stage='terminal' WHERE attempt_id=? \
                 AND stage='needs_review' AND exit_confirmed_at IS NULL",
                vec![c.attempt_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        let now = Utc::now();
        let changed = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_task_attempt SET revision=revision+1,updated_at=? \
                 WHERE attempt_id=? AND status='interrupted' AND finished_at IS NULL",
                vec![now.into(), c.attempt_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_dispatch SET review_required=0 WHERE attempt_id=? AND claim_id=?",
            vec![c.attempt_id.clone().into(), c.claim_id.clone().into()],
        ))
        .await
        .map_err(storage_error)?;
        event(
            &tx,
            c,
            "human:resolve_best_effort",
            "human_best_effort_resolution",
            "interrupted",
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        self.port.revoke(b);
        self.revoke_exchange(&b.owner.task, &b.owner.attempt, &b.run_id)
            .await;
        Ok(())
    }
    pub async fn poll_once(&self, c: &Claim, b: &Binding) -> Result<(), AppCommandError> {
        let tx = self.host.db.begin().await.map_err(storage_error)?;
        let r = guard(&tx, &self.host, c).await?;
        if !bound(&r, b, c) {
            return Err(denied());
        }
        tx.commit().await.map_err(storage_error)?;
        let events = match self.port.events(b, r.last_sequence as u64).await {
            Ok(events) => events,
            Err(_) => {
                self.needs_review(c).await?;
                return Err(denied());
            }
        };
        for (seq, event) in events {
            self.accept_event(c, b, seq, event).await?;
        }
        Ok(())
    }
}
