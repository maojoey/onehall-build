//! Durable headquarters dispatch intentions. Creating a record never contacts
//! a branch or starts execution; delivery and local approval are separate steps.
use crate::app_error::AppCommandError;
use crate::hall::exchange::{service::administrator, DispatchOwner, Exchange};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub submission_id: String,
    pub branch_id: String,
    pub member_ref: String,
    pub title: String,
    pub description: String,
    /// Frozen at creation; omitted for v1 dispatches so their hash is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_manifest_id: Option<String>,
}

#[derive(FromQueryResult, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub dispatch_id: String,
    pub branch_id: String,
    pub member_ref: String,
    pub title: String,
    pub status: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
    pub task_status: Option<String>,
    pub result_text: Option<String>,
    pub sync_error: Option<String>,
}

#[derive(FromQueryResult)]
struct Stored {
    dispatch_id: String,
    branch_id: String,
    member_ref: String,
    title: String,
    status: String,
    revision: i64,
    created_at: String,
    updated_at: String,
    request_hash: String,
    task_status: Option<String>,
    result_text: Option<String>,
    sync_error: Option<String>,
}
impl Stored {
    fn receipt(self) -> Receipt {
        Receipt {
            dispatch_id: self.dispatch_id,
            branch_id: self.branch_id,
            member_ref: self.member_ref,
            title: self.title,
            status: self.status,
            revision: self.revision,
            created_at: self.created_at,
            updated_at: self.updated_at,
            task_status: self.task_status,
            result_text: self.result_text,
            sync_error: self.sync_error,
        }
    }
}
fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Branch dispatch storage is unavailable")
}
fn invalid() -> AppCommandError {
    AppCommandError::invalid_input("Invalid branch dispatch fields")
}
fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, query, values)
}
fn normalized(mut draft: Draft) -> Result<Draft, AppCommandError> {
    draft.submission_id = uuid::Uuid::parse_str(&draft.submission_id)
        .map_err(|_| invalid())?
        .to_string();
    draft.branch_id = uuid::Uuid::parse_str(&draft.branch_id)
        .map_err(|_| invalid())?
        .to_string();
    if let Some(manifest) = &draft.exchange_manifest_id {
        draft.exchange_manifest_id = Some(
            uuid::Uuid::parse_str(manifest)
                .map_err(|_| invalid())?
                .to_string(),
        );
    }
    draft.member_ref = draft.member_ref.trim().to_owned();
    draft.title = draft.title.trim().to_owned();
    if draft.member_ref.is_empty()
        || draft.member_ref.chars().count() > 128
        || !draft
            .member_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        || draft.title.is_empty()
        || draft.title.chars().count() > 160
        || draft.title.chars().any(char::is_control)
        || draft.description.chars().count() > 8000
        || draft
            .description
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(invalid());
    }
    Ok(draft)
}

pub async fn create(db: &DatabaseConnection, draft: Draft) -> Result<Receipt, AppCommandError> {
    create_owned(db, None, draft, None).await
}

/// Operator creation that may freeze a staged exchange manifest into the dispatch.
pub async fn create_with_exchange(
    db: &DatabaseConnection,
    exchange: Option<&Exchange>,
    draft: Draft,
) -> Result<Receipt, AppCommandError> {
    create_owned(db, exchange, draft, None).await
}

pub async fn create_for_pm(
    db: &DatabaseConnection,
    draft: Draft,
    member_id: &str,
) -> Result<Receipt, AppCommandError> {
    super::service::validate_uuid(member_id)?;
    create_owned(db, None, draft, Some(member_id)).await
}

async fn create_owned(
    db: &DatabaseConnection,
    exchange: Option<&Exchange>,
    draft: Draft,
    creator: Option<&str>,
) -> Result<Receipt, AppCommandError> {
    let draft = normalized(draft)?;
    if draft.exchange_manifest_id.is_some() && (creator.is_some() || exchange.is_none()) {
        return Err(AppCommandError::permission_denied(
            "Attachments need an operator session with file exchange available",
        ));
    }
    let payload = serde_json::to_vec(&draft).map_err(storage)?;
    // Keep the original host hash for durable retry compatibility.
    let hash = if let Some(member) = creator {
        let mut hash = Sha256::new();
        hash.update(b"onehall-pm-dispatch-v1\0");
        hash.update(member.as_bytes());
        hash.update(&payload);
        format!("{:x}", hash.finalize())
    } else {
        format!("{:x}", Sha256::digest(&payload))
    };
    let now = chrono::Utc::now().to_rfc3339();
    // Insertion and the enabled check share one SQLite statement. A disable
    // that wins first cannot be bypassed by a stale earlier lookup.
    let tx = db.begin().await.map_err(storage)?;
    tx.execute(sql(
        "INSERT INTO hall_branch_dispatch \
         (dispatch_id,submission_id,request_hash,branch_id,member_ref,title,description,status,revision,created_at,updated_at,creator_member_id) \
         SELECT ?,?,?,?,?,?,?,'pending_delivery',1,?,?,? WHERE EXISTS \
         (SELECT 1 FROM hall_branch WHERE id=? AND enabled=1) \
         ON CONFLICT(submission_id) DO NOTHING",
        vec![uuid::Uuid::new_v4().to_string().into(), draft.submission_id.clone().into(),
             hash.clone().into(), draft.branch_id.clone().into(), draft.member_ref.into(),
             draft.title.into(), draft.description.into(), now.clone().into(), now.into(), creator.map(str::to_owned).into(), draft.branch_id.clone().into()],
    )).await.map_err(storage)?;
    let stored = Stored::find_by_statement(sql(
        "SELECT dispatch_id,branch_id,member_ref,title,status,revision,created_at,updated_at,request_hash,task_status,result_text,sync_error \
         FROM hall_branch_dispatch WHERE submission_id=?",
        vec![draft.submission_id.clone().into()],
    )).one(&tx).await.map_err(storage)?
      .ok_or_else(|| AppCommandError::permission_denied("Branch is unavailable or disabled"))?;
    if stored.request_hash != hash {
        return Err(AppCommandError::already_exists(
            "Submission key already belongs to another dispatch",
        ));
    }
    if let (Some(manifest), Some(exchange)) = (&draft.exchange_manifest_id, exchange) {
        // Freeze the manifest in the SAME transaction as the dispatch row.
        let issuer: String = tx
            .query_one(sql(
                "SELECT issuer_id FROM hall_branch_dispatch_credential WHERE branch_id=?",
                vec![draft.branch_id.clone().into()],
            ))
            .await
            .map_err(storage)?
            .ok_or_else(|| {
                AppCommandError::permission_denied("Branch dispatch credential needs local setup")
            })?
            .try_get("", "issuer_id")
            .map_err(storage)?;
        let owner = DispatchOwner::authenticated(&issuer, &stored.dispatch_id, &draft.branch_id)?;
        exchange
            .attach_in(
                &tx,
                &administrator(),
                &owner,
                &draft.submission_id,
                manifest,
            )
            .await?;
    }
    tx.commit().await.map_err(storage)?;
    Ok(stored.receipt())
}

pub async fn list(db: &DatabaseConnection) -> Result<Vec<Receipt>, AppCommandError> {
    let rows = Stored::find_by_statement(sql(
        "SELECT dispatch_id,branch_id,member_ref,title,status,revision,created_at,updated_at,request_hash,task_status,result_text,sync_error \
         FROM hall_branch_dispatch ORDER BY created_at,dispatch_id LIMIT 1000", vec![],
    )).all(db).await.map_err(storage)?;
    Ok(rows.into_iter().map(Stored::receipt).collect())
}

/// Public receipt only. Request bodies and credentials stay in private storage.
pub async fn get(db: &DatabaseConnection, id: &str) -> Result<Receipt, AppCommandError> {
    super::service::validate_uuid(id)?;
    Stored::find_by_statement(sql("SELECT dispatch_id,branch_id,member_ref,title,status,revision,created_at,updated_at,request_hash,task_status,result_text,sync_error FROM hall_branch_dispatch WHERE dispatch_id=?",vec![id.into()])).one(db).await.map_err(storage)?.map(Stored::receipt).ok_or_else(||AppCommandError::not_found("Dispatch not found"))
}
