//! Branch-local headquarters trust. Not exposed by IPC or network yet.
//! Raw credentials are returned once to the local caller and never stored.
use crate::app_error::AppCommandError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use sha2::Sha256;

type Tag = Hmac<Sha256>;
const DOMAIN: &[u8] = b"onehall-branch-trust-v1";
const LIMIT: i64 = 32;

// Deliberately no Debug, Clone or Serialize: the raw credential is one-time.
pub(super) struct Registration {
    pub issuer_id: String,
    pub secret: String,
}
#[derive(Serialize, FromQueryResult)]
#[serde(rename_all = "camelCase")]
pub(super) struct Summary {
    pub issuer_id: String,
    pub name: String,
    pub enabled: bool,
    pub revision: i64,
}
// Only authenticated callers can obtain this identity. It is not deserializable.
pub(super) struct VerifiedIssuer {
    issuer_id: String,
}
impl VerifiedIssuer {
    pub(super) fn id(&self) -> &str {
        &self.issuer_id
    }
}
#[derive(FromQueryResult)]
struct CredentialRow {
    credential_hash: String,
    enabled: bool,
}
fn denied() -> AppCommandError {
    AppCommandError::permission_denied("Headquarters credentials are invalid")
}
fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Branch trust storage is unavailable")
}
fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, query, values)
}
fn tag(secret: &str) -> Tag {
    let mut tag = Tag::new_from_slice(DOMAIN).expect("HMAC accepts any key length");
    tag.update(secret.as_bytes());
    tag
}

/// Invoked only by a locally authorized setup flow; not registered as an API.
pub(super) async fn create_local(
    db: &DatabaseConnection,
    name: &str,
) -> Result<Registration, AppCommandError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err(AppCommandError::invalid_input("Invalid headquarters name"));
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let secret = URL_SAFE_NO_PAD.encode(bytes);
    let credential_hash = URL_SAFE_NO_PAD.encode(tag(&secret).finalize().into_bytes());
    let issuer_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let result = db.execute(sql(
        "INSERT INTO hall_branch_trust (issuer_id,name,credential_hash,enabled,revision,created_at,updated_at) \
         SELECT ?,?,?,1,1,?,? WHERE (SELECT COUNT(*) FROM hall_branch_trust) < ?",
        vec![issuer_id.clone().into(),name.into(),credential_hash.into(),now.clone().into(),now.into(),LIMIT.into()],
    )).await.map_err(storage)?;
    if result.rows_affected() != 1 {
        return Err(AppCommandError::invalid_input(
            "Headquarters trust limit reached",
        ));
    }
    Ok(Registration { issuer_id, secret })
}

/// Authentication returns a host-created identity; the wire request cannot
/// manufacture this value. Revocation must also be checked atomically at intake.
pub(super) async fn authenticate(
    db: &DatabaseConnection,
    issuer_id: &str,
    secret: &str,
) -> Result<VerifiedIssuer, AppCommandError> {
    let issuer = uuid::Uuid::parse_str(issuer_id)
        .map_err(|_| denied())?
        .to_string();
    if secret.len() != 43
        || URL_SAFE_NO_PAD
            .decode(secret)
            .map_or(true, |v| v.len() != 32)
    {
        return Err(denied());
    }
    let row = CredentialRow::find_by_statement(sql(
        "SELECT credential_hash,enabled FROM hall_branch_trust WHERE issuer_id=?",
        vec![issuer.clone().into()],
    ))
    .one(db)
    .await
    .map_err(storage)?;
    let expected = row
        .as_ref()
        .and_then(|v| URL_SAFE_NO_PAD.decode(&v.credential_hash).ok())
        .filter(|v| v.len() == 32)
        .unwrap_or_else(|| vec![0u8; 32]);
    let valid = tag(secret).verify_slice(&expected).is_ok();
    if !valid || !row.as_ref().is_some_and(|v| v.enabled) {
        return Err(denied());
    }
    Ok(VerifiedIssuer { issuer_id: issuer })
}

pub(super) async fn list_local(db: &DatabaseConnection) -> Result<Vec<Summary>, AppCommandError> {
    Summary::find_by_statement(sql(
        "SELECT issuer_id,name,enabled,revision FROM hall_branch_trust ORDER BY created_at,issuer_id",
        vec![],
    )).all(db).await.map_err(storage)
}

pub(super) async fn revoke_local(
    db: &DatabaseConnection,
    issuer_id: &str,
    expected_revision: i64,
) -> Result<(), AppCommandError> {
    let issuer = uuid::Uuid::parse_str(issuer_id)
        .map_err(|_| denied())?
        .to_string();
    if expected_revision < 1 {
        return Err(AppCommandError::invalid_input("Invalid trust revision"));
    }
    let result = db
        .execute(sql(
            "UPDATE hall_branch_trust SET enabled=0,revision=revision+1,updated_at=? \
         WHERE issuer_id=? AND enabled=1 AND revision=?",
            vec![
                chrono::Utc::now().to_rfc3339().into(),
                issuer.into(),
                expected_revision.into(),
            ],
        ))
        .await
        .map_err(storage)?;
    if result.rows_affected() != 1 {
        return Err(AppCommandError::already_exists(
            "Trust changed; refresh before retrying",
        ));
    }
    Ok(())
}
