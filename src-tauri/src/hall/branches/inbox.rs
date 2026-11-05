//! Branch-local intake ledger. This module is not mounted as a network API.
//! The caller must bind `issuer_id` to an authenticated headquarters identity;
//! receiving and locally accepting an envelope never starts a task.
use crate::app_error::AppCommandError;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Envelope {
    pub dispatch_id: String,
    pub member_ref: String,
    pub title: String,
    pub description: String,
}
#[derive(Clone, Copy)]
pub(super) enum Decision {
    Accept,
    Reject,
}
impl Decision {
    fn status(self) -> &'static str {
        match self {
            Self::Accept => "accepted",
            Self::Reject => "rejected",
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Receipt {
    pub dispatch_id: String,
    pub issuer_id: String,
    pub member_ref: String,
    pub title: String,
    pub status: String,
    pub revision: i64,
    pub launch_allowed: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Detail {
    #[serde(flatten)]
    pub receipt: Receipt,
    pub description: String,
}
#[derive(FromQueryResult)]
struct Row {
    issuer_id: String,
    dispatch_id: String,
    request_hash: String,
    member_ref: String,
    title: String,
    description: String,
    status: String,
    revision: i64,
}
impl Row {
    fn detail(self) -> Detail {
        Detail {
            receipt: Receipt {
                dispatch_id: self.dispatch_id,
                issuer_id: self.issuer_id,
                member_ref: self.member_ref,
                title: self.title,
                status: self.status,
                revision: self.revision,
                launch_allowed: false,
            },
            description: self.description,
        }
    }
}
fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Branch inbox storage is unavailable")
}
fn invalid() -> AppCommandError {
    AppCommandError::invalid_input("Invalid branch inbox fields")
}
fn conflict() -> AppCommandError {
    AppCommandError::already_exists("Branch inbox item changed; refresh before retrying")
}
fn id(value: &str) -> Result<String, AppCommandError> {
    uuid::Uuid::parse_str(value)
        .map(|v| v.to_string())
        .map_err(|_| invalid())
}
fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, query, values)
}
fn normalize(mut envelope: Envelope) -> Result<Envelope, AppCommandError> {
    envelope.dispatch_id = id(&envelope.dispatch_id)?;
    envelope.member_ref = envelope.member_ref.trim().to_owned();
    envelope.title = envelope.title.trim().to_owned();
    if envelope.member_ref.is_empty()
        || envelope.member_ref.chars().count() > 128
        || !envelope
            .member_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        || envelope.title.is_empty()
        || envelope.title.chars().count() > 160
        || envelope.title.chars().any(char::is_control)
        || envelope.description.chars().count() > 8000
        || envelope
            .description
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(invalid());
    }
    Ok(envelope)
}
async fn row<C: ConnectionTrait>(
    db: &C,
    issuer: &str,
    dispatch: &str,
) -> Result<Row, AppCommandError> {
    Row::find_by_statement(sql(
        "SELECT issuer_id,dispatch_id,request_hash,member_ref,title,description,status,revision \
         FROM hall_branch_inbox WHERE issuer_id=? AND dispatch_id=?",
        vec![issuer.into(), dispatch.into()],
    ))
    .one(db)
    .await
    .map_err(storage)?
    .ok_or_else(|| AppCommandError::not_found("Branch inbox item not found"))
}
async fn store<C: ConnectionTrait>(
    db: &C,
    issuer_id: &str,
    envelope: Envelope,
) -> Result<Receipt, AppCommandError> {
    store_in(db, issuer_id, envelope, None).await
}
pub(super) async fn exists<C: ConnectionTrait>(
    db: &C,
    issuer: &str,
    dispatch: &str,
) -> Result<bool, AppCommandError> {
    Ok(db
        .query_one(sql(
            "SELECT 1 FROM hall_branch_inbox WHERE issuer_id=? AND dispatch_id=?",
            vec![issuer.into(), dispatch.into()],
        ))
        .await
        .map_err(storage)?
        .is_some())
}
/// `manifest_digest` joins the request hash only for v2 items, so v1 hashes
/// and retries stay byte-for-byte compatible.
pub(super) async fn store_in<C: ConnectionTrait>(
    db: &C,
    issuer_id: &str,
    envelope: Envelope,
    manifest_digest: Option<&str>,
) -> Result<Receipt, AppCommandError> {
    let issuer = id(issuer_id)?;
    let envelope = normalize(envelope)?;
    let mut hasher = Sha256::new();
    hasher.update(b"onehall-branch-inbox-v1\0");
    hasher.update(issuer.as_bytes());
    hasher.update(serde_json::to_vec(&envelope).map_err(storage)?);
    if let Some(digest) = manifest_digest {
        hasher.update(b"\0exchange-v2\0");
        hasher.update(digest.as_bytes());
    }
    let hash = format!("{:x}", hasher.finalize());
    let now = chrono::Utc::now().to_rfc3339();
    db.execute(sql(
        "INSERT INTO hall_branch_inbox \
         (issuer_id,dispatch_id,request_hash,member_ref,title,description,status,revision,created_at,updated_at) \
         VALUES (?,?,?,?,?,?,'awaiting_confirmation',1,?,?) \
         ON CONFLICT(issuer_id,dispatch_id) DO NOTHING",
        vec![issuer.clone().into(),envelope.dispatch_id.clone().into(),hash.clone().into(),
             envelope.member_ref.into(),envelope.title.into(),envelope.description.into(),
             now.clone().into(),now.into()],
    )).await.map_err(storage)?;
    let saved = row(db, &issuer, &envelope.dispatch_id).await?;
    if saved.request_hash != hash {
        return Err(conflict());
    }
    Ok(saved.detail().receipt)
}
// Unauthenticated intake is available only to storage tests, never in builds.
#[cfg(test)]
pub(super) async fn receive(
    db: &DatabaseConnection,
    issuer_id: &str,
    envelope: Envelope,
) -> Result<Receipt, AppCommandError> {
    store(db, issuer_id, envelope).await
}

/// The identity is issued by the trust verifier, never deserialized from a
/// request. The first transaction operation takes the SQLite write lock and
/// rechecks authorization, so a completed revocation cannot race intake.
pub(super) async fn receive_verified(
    db: &DatabaseConnection,
    issuer: &super::trust::VerifiedIssuer,
    envelope: Envelope,
) -> Result<Receipt, AppCommandError> {
    let transaction = db.begin().await.map_err(storage)?;
    let authorized = transaction
        .execute(sql(
            "UPDATE hall_branch_trust SET revision=revision WHERE issuer_id=? AND enabled=1",
            vec![issuer.id().into()],
        ))
        .await
        .map_err(storage)?;
    if authorized.rows_affected() != 1 {
        return Err(AppCommandError::permission_denied(
            "Headquarters credentials are invalid",
        ));
    }
    let result = store(&transaction, issuer.id(), envelope).await?;
    transaction.commit().await.map_err(storage)?;
    Ok(result)
}

/// A remote status read is scoped to the verified issuer and fails closed if
/// that issuer has since been revoked. Local history uses `get` instead.
pub(super) async fn get_verified<C: ConnectionTrait>(
    db: &C,
    issuer: &super::trust::VerifiedIssuer,
    dispatch_id: &str,
) -> Result<Detail, AppCommandError> {
    let dispatch = id(dispatch_id)?;
    // A single snapshot distinguishes a trusted missing item (safe to POST)
    // from revoked trust, without exposing another issuer's namespace.
    let result=db.query_one(sql(
        "SELECT i.issuer_id,i.dispatch_id,i.request_hash,i.member_ref,i.title,i.description,i.status,i.revision FROM hall_branch_trust t LEFT JOIN hall_branch_inbox i ON i.issuer_id=t.issuer_id AND i.dispatch_id=? WHERE t.issuer_id=? AND t.enabled=1",
        vec![dispatch.into(),issuer.id().into()]
    )).await.map_err(storage)?.ok_or_else(||AppCommandError::permission_denied("Headquarters credentials are invalid"))?;
    if result
        .try_get::<Option<String>>("", "dispatch_id")
        .map_err(storage)?
        .is_none()
    {
        return Err(AppCommandError::not_found("Branch inbox item not found"));
    }
    Ok(Row::from_query_result(&result, "")
        .map_err(storage)?
        .detail())
}

pub(super) async fn get<C: ConnectionTrait>(
    db: &C,
    issuer_id: &str,
    dispatch_id: &str,
) -> Result<Detail, AppCommandError> {
    Ok(row(db, &id(issuer_id)?, &id(dispatch_id)?).await?.detail())
}
/// Only a locally authenticated operator may invoke this once an IPC boundary
/// exists; this function is deliberately not registered with any router.
pub(super) async fn decide<C: ConnectionTrait>(
    db: &C,
    issuer_id: &str,
    dispatch_id: &str,
    expected_revision: i64,
    decision: Decision,
) -> Result<Receipt, AppCommandError> {
    if expected_revision < 1 {
        return Err(invalid());
    }
    let issuer = id(issuer_id)?;
    let dispatch = id(dispatch_id)?;
    let status = decision.status();
    db.execute(sql(
        "UPDATE hall_branch_inbox SET status=?,revision=revision+1,updated_at=? \
         WHERE issuer_id=? AND dispatch_id=? AND status='awaiting_confirmation' AND revision=?",
        vec![
            status.into(),
            chrono::Utc::now().to_rfc3339().into(),
            issuer.clone().into(),
            dispatch.clone().into(),
            expected_revision.into(),
        ],
    ))
    .await
    .map_err(storage)?;
    let saved = row(db, &issuer, &dispatch).await?;
    if saved.status != status || expected_revision.checked_add(1) != Some(saved.revision) {
        return Err(conflict());
    }
    Ok(saved.detail().receipt)
}
