//! An intake and its host-local task are committed in the same transaction.
use crate::{app_error::AppCommandError, hall::tasks};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub issuer_id: String,
    pub dispatch_id: String,
    pub expected_revision: i64,
    pub member_id: String,
    pub snapshot_id: String,
    pub project_id: Option<i32>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Binding {
    pub task_id: String,
}
#[derive(FromQueryResult)]
struct Saved {
    task_id: String,
    request_hash: String,
}
fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Branch task binding is unavailable")
}
fn conflict() -> AppCommandError {
    AppCommandError::already_exists("Branch task selection changed; refresh before retrying")
}
fn id(raw: &str) -> Result<String, AppCommandError> {
    uuid::Uuid::parse_str(raw)
        .map(|v| v.to_string())
        .map_err(|_| AppCommandError::invalid_input("Invalid branch task identity"))
}
fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, query, values)
}
pub(super) async fn accept_local(
    db: &DatabaseConnection,
    mut r: Request,
) -> Result<Binding, AppCommandError> {
    r.issuer_id = id(&r.issuer_id)?;
    r.dispatch_id = id(&r.dispatch_id)?;
    r.member_id = id(&r.member_id)?;
    r.snapshot_id = id(&r.snapshot_id)?;
    if r.expected_revision < 1 || r.project_id.is_some_and(|v| v < 1) {
        return Err(AppCommandError::invalid_input(
            "Invalid branch task selection",
        ));
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&r).map_err(storage)?)
    );
    let tx = db.begin().await.map_err(storage)?;
    // First write serializes accept/reject and retries on every connection.
    tx.execute(sql(
        "UPDATE hall_branch_inbox SET revision=revision WHERE issuer_id=? AND dispatch_id=?",
        vec![r.issuer_id.clone().into(), r.dispatch_id.clone().into()],
    ))
    .await
    .map_err(storage)?;
    if let Some(saved)=Saved::find_by_statement(sql(
        "SELECT task_id,request_hash FROM hall_branch_task_binding WHERE issuer_id=? AND dispatch_id=?",
        vec![r.issuer_id.clone().into(),r.dispatch_id.clone().into()])).one(&tx).await.map_err(storage)? {
        if saved.request_hash!=hash {return Err(conflict());}
        tx.commit().await.map_err(storage)?;
        return Ok(Binding{task_id:saved.task_id});
    }
    // Local confirmation selects current, enabled capacity. Keep this after
    // the existing binding lookup so retries still recover their original
    // draft after the member is disabled or its configuration changes.
    let eligible=tx.query_one(sql("SELECT 1 FROM hall_member m JOIN hall_execution_snapshot s ON s.member_id=m.member_id WHERE m.member_id=? AND m.enabled=1 AND s.snapshot_id=? AND s.member_revision=m.revision",vec![r.member_id.clone().into(),r.snapshot_id.clone().into()])).await.map_err(storage)?;
    if eligible.is_none() {
        return Err(AppCommandError::permission_denied(
            "Select an enabled local member with a current saved snapshot",
        ));
    }
    let item = super::inbox::get(&tx, &r.issuer_id, &r.dispatch_id).await?;
    if item.receipt.status != "awaiting_confirmation"
        || item.receipt.revision != r.expected_revision
    {
        return Err(conflict());
    }
    let task = tasks::create_task(
        &tx,
        tasks::CreateTaskDraft {
            submission_id: uuid::Uuid::new_v4().to_string(),
            member_id: r.member_id,
            snapshot_id: r.snapshot_id,
            project_id: r.project_id,
            purpose: tasks::TaskPurpose::Work,
            title: item.receipt.title,
            description: item.description,
        },
    )
    .await?;
    tx.execute(sql("INSERT INTO hall_branch_task_binding(issuer_id,dispatch_id,task_id,request_hash) VALUES(?,?,?,?)",
        vec![r.issuer_id.clone().into(),r.dispatch_id.clone().into(),task.task_id.clone().into(),hash.into()])).await.map_err(storage)?;
    super::inbox::decide(
        &tx,
        &r.issuer_id,
        &r.dispatch_id,
        r.expected_revision,
        super::inbox::Decision::Accept,
    )
    .await?;
    tx.commit().await.map_err(storage)?;
    Ok(Binding {
        task_id: task.task_id,
    })
}

pub(super) async fn task_id<C: ConnectionTrait>(
    db: &C,
    issuer: &str,
    dispatch: &str,
) -> Result<Option<String>, AppCommandError> {
    #[derive(FromQueryResult)]
    struct Id {
        task_id: String,
    }
    Ok(Id::find_by_statement(sql(
        "SELECT task_id FROM hall_branch_task_binding WHERE issuer_id=? AND dispatch_id=?",
        vec![id(issuer)?.into(), id(dispatch)?.into()],
    ))
    .one(db)
    .await
    .map_err(storage)?
    .map(|r| r.task_id))
}
