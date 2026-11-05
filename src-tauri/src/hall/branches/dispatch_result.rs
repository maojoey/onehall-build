//! Narrow result view: only the task bound to this authenticated dispatch.
use crate::app_error::AppCommandError;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskReport {
    pub task_id: String,
    pub revision: i64,
    pub status: String,
    pub result_outcome: Option<String>,
    pub result_text: Option<String>,
    // Present only for exchange-capable branches; v1 reports omit them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_verified: Option<bool>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Report {
    #[serde(flatten)]
    pub item: super::inbox::Detail,
    pub task: Option<TaskReport>,
}
fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Dispatch result is unavailable")
}
fn sql(q: &str, v: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, q, v)
}
pub(super) async fn detail(
    db: &DatabaseConnection,
    issuer: &super::trust::VerifiedIssuer,
    dispatch: &str,
) -> Result<Report, AppCommandError> {
    // One snapshot for trust, inbox, binding, task revision and result evidence.
    let tx = db.begin().await.map_err(storage)?;
    let item = super::inbox::get_verified(&tx, issuer, dispatch).await?;
    #[derive(FromQueryResult)]
    struct Task {
        task_id: String,
        revision: i64,
        status: String,
    }
    let task=Task::find_by_statement(sql("SELECT t.task_id,t.revision,t.status FROM hall_branch_task_binding b JOIN hall_task t ON t.task_id=b.task_id WHERE b.issuer_id=? AND b.dispatch_id=?",vec![issuer.id().into(),dispatch.into()])).one(&tx).await.map_err(storage)?;
    let task = if let Some(task) = task {
        #[derive(FromQueryResult)]
        struct ResultRow {
            outcome: Option<String>,
            body: Option<String>,
            attempt_id: String,
            attempt_number: i64,
            run_id: Option<String>,
            exit_confirmed_at: Option<String>,
        }
        let result=ResultRow::find_by_statement(sql("SELECT r.outcome,substr(r.body,1,16384) AS body,a.attempt_id,a.attempt_number,(SELECT rt.run_id FROM hall_task_runtime rt WHERE rt.attempt_id=a.attempt_id) AS run_id,(SELECT rt.exit_confirmed_at FROM hall_task_runtime rt WHERE rt.attempt_id=a.attempt_id) AS exit_confirmed_at FROM hall_task_attempt a LEFT JOIN hall_task_result r ON r.attempt_id=a.attempt_id WHERE a.task_id=? ORDER BY a.attempt_number DESC LIMIT 1",vec![task.task_id.clone().into()])).one(&tx).await.map_err(storage)?;
        let identity = result.as_ref().map(|r| {
            (
                r.attempt_id.clone(),
                r.attempt_number,
                r.run_id.clone(),
                r.exit_confirmed_at.is_some(),
            )
        });
        let (outcome, text) = if let Some(r) = result {
            let text = r.body.map(|mut body| {
                let mut end = body.len().min(16384);
                while !body.is_char_boundary(end) {
                    end -= 1;
                }
                body.truncate(end);
                body
            });
            (r.outcome, text)
        } else {
            (None, None)
        };
        Some(TaskReport {
            task_id: task.task_id,
            revision: task.revision,
            status: task.status,
            result_outcome: outcome,
            result_text: text,
            attempt_id: identity.as_ref().map(|i| i.0.clone()),
            attempt_number: identity.as_ref().map(|i| i.1),
            run_id: identity.as_ref().and_then(|i| i.2.clone()),
            exit_verified: identity.as_ref().map(|i| i.3),
        })
    } else {
        None
    };
    tx.commit().await.map_err(storage)?;
    Ok(Report { item, task })
}
