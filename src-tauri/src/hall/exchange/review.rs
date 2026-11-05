//! Read-only view of a frozen manifest for human review on the branch. The
//! digest already binds the manifest; a stored copy that no longer verifies
//! against it is refused rather than shown. Nothing here grants access.
use super::{manifest, ExchangeError, ExchangeManifest, Result};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

async fn has_table<C: ConnectionTrait>(db: &C, name: &str) -> Result<bool> {
    Ok(db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            vec![name.into()],
        ))
        .await?
        .is_some())
}
async fn read<C: ConnectionTrait>(
    db: &C,
    query: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<ExchangeManifest>> {
    let Some(row) = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            query,
            values,
        ))
        .await?
    else {
        return Ok(None);
    };
    let json: String = row.try_get("", "manifest_json")?;
    let digest: String = row.try_get("", "digest")?;
    let parsed: ExchangeManifest =
        serde_json::from_str(&json).map_err(|_| ExchangeError::Storage)?;
    manifest::verify(&parsed).map_err(|_| ExchangeError::Storage)?;
    if parsed.digest != digest {
        return Err(ExchangeError::Storage);
    }
    Ok(Some(parsed))
}

/// Manifest frozen for this issuer's dispatch; `None` for v1 items and for
/// databases created before the exchange tables existed.
pub async fn for_dispatch<C: ConnectionTrait>(
    db: &C,
    issuer: &str,
    dispatch: &str,
) -> Result<Option<ExchangeManifest>> {
    if !has_table(db, "hall_exchange_attachment").await? {
        return Ok(None);
    }
    read(
        db,
        "SELECT m.manifest_json AS manifest_json,m.digest AS digest \
         FROM hall_exchange_attachment a \
         JOIN hall_exchange_manifest m ON m.manifest_id=a.manifest_id \
         WHERE a.dispatch_id=? AND a.issuer_id=?",
        vec![dispatch.into(), issuer.into()],
    )
    .await
}

/// Same join as the launch review digest: the bound, unrevoked attachment.
pub async fn for_task<C: ConnectionTrait>(db: &C, task: &str) -> Result<Option<ExchangeManifest>> {
    if !has_table(db, "hall_exchange_attachment").await?
        || !has_table(db, "hall_branch_task_binding").await?
    {
        return Ok(None);
    }
    read(
        db,
        "SELECT m.manifest_json AS manifest_json,m.digest AS digest \
         FROM hall_branch_task_binding b \
         JOIN hall_exchange_attachment a ON a.dispatch_id=b.dispatch_id AND a.issuer_id=b.issuer_id \
         JOIN hall_exchange_manifest m ON m.manifest_id=a.manifest_id \
         WHERE b.task_id=? AND a.revoked=0",
        vec![task.into()],
    )
    .await
}
