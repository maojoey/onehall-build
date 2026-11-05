//! Host-trusted bridge from the task runtime to the exchange ledger. Owners are
//! rebuilt from the database rows of the real task/attempt/run, never from a
//! model- or wire-supplied identity.
use super::{AttemptOwner, DispatchOwner, Exchange, ExchangeError, ExitVerification, Result};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

fn sql(q: &str, v: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, q, v)
}

/// `None` when the task has no attached manifest (every v1/plain task).
pub(crate) async fn attempt_owner<C: ConnectionTrait>(
    db: &C,
    task: &str,
    attempt: &str,
    run: &str,
) -> Result<Option<AttemptOwner>> {
    let Some(row) = db
        .query_one(sql(
            "SELECT a.dispatch_id,a.issuer_id,a.branch_id,t.attempt_number FROM hall_branch_task_binding b \
             JOIN hall_exchange_attachment a ON a.dispatch_id=b.dispatch_id AND a.issuer_id=b.issuer_id \
             JOIN hall_task_attempt t ON t.task_id=b.task_id AND t.attempt_id=? \
             WHERE b.task_id=?",
            vec![attempt.into(), task.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let owner = DispatchOwner::authenticated(
        &row.try_get::<String>("", "issuer_id")?,
        &row.try_get::<String>("", "dispatch_id")?,
        &row.try_get::<String>("", "branch_id")?,
    )?;
    let number: i64 = row.try_get("", "attempt_number")?;
    AttemptOwner::authenticated(&owner, task, attempt, run, number).map(Some)
}

/// The task's current run, for host tools that resolve identity per call.
// Consumed by the model tools in `team::content`; identity stays host-derived.
pub(crate) async fn current_owner<C: ConnectionTrait>(
    db: &C,
    task: &str,
) -> Result<Option<(AttemptOwner, ExitVerification)>> {
    let Some(row) = db
        .query_one(sql(
            "SELECT t.attempt_id,r.run_id,r.exit_confirmed_at FROM hall_task_attempt t \
             JOIN hall_task_runtime r ON r.attempt_id=t.attempt_id \
             WHERE t.task_id=? AND r.run_id IS NOT NULL ORDER BY t.attempt_number DESC LIMIT 1",
            vec![task.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let attempt: String = row.try_get("", "attempt_id")?;
    let run: String = row.try_get("", "run_id")?;
    let exit = if row
        .try_get::<Option<String>>("", "exit_confirmed_at")?
        .is_some()
    {
        ExitVerification::Verified
    } else {
        ExitVerification::Unverified
    };
    Ok(attempt_owner(db, task, &attempt, &run)
        .await?
        .map(|owner| (owner, exit)))
}

impl Exchange {
    /// Bind the real attempt/run and prepare its private input/output areas.
    /// Fails closed: the model must not start before inputs are verified on disk.
    pub(crate) async fn bind_run<C: ConnectionTrait>(
        &self,
        db: &C,
        task: &str,
        attempt: &str,
        run: &str,
    ) -> Result<()> {
        let Some(actor) = attempt_owner(db, task, attempt, run).await? else {
            return Ok(());
        };
        if !self.inputs_ready(&actor.dispatch).await? {
            return Err(ExchangeError::Conflict);
        }
        self.materialize(&actor).await.map(|_| ())
    }
    /// Revoke write authority after result, cancel, failed start or release.
    /// Already sealed history stays readable to authorized hosts.
    pub(crate) async fn revoke_run<C: ConnectionTrait>(
        &self,
        db: &C,
        task: &str,
        attempt: &str,
        run: &str,
    ) -> Result<()> {
        match attempt_owner(db, task, attempt, run).await? {
            Some(actor) => self.revoke_attempt(&actor).await,
            None => Ok(()),
        }
    }
}
