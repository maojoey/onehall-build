//! Explicit fixed-path saves; the database lock serializes member edits with
//! this write. Filesystem locking and replacement live in `write`.
use super::super::{entity, MemberError};
use super::{combine, load, merge, write, MemberConfigPreview};
use crate::app_error::AppCommandError;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Statement, TransactionTrait};
use serde_json::Value;

pub async fn hall_save_member_config_core(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    expected_file_version: String,
    draft: Value,
) -> Result<MemberConfigPreview, AppCommandError> {
    if uuid::Uuid::parse_str(&member_id).is_err()
        || expected_revision < 1
        || expected_file_version.len() != 64
        || !expected_file_version.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppCommandError::invalid_input(
            "A member revision and configuration file version are required",
        ));
    }
    let txn = db.begin().await.map_err(MemberError::Database)?;
    // SQLite permits one writer. A conditional no-op update obtains that lock
    // without changing revision, timestamps or any member data. Keep it until
    // the filesystem operation finishes so another registry edit cannot race.
    let locked = txn
        .execute(Statement::from_sql_and_values(
            txn.get_database_backend(),
            "UPDATE hall_member SET revision = revision WHERE member_id = ? AND revision = ?",
            [member_id.clone().into(), expected_revision.into()],
        ))
        .await
        .map_err(MemberError::Database)?;
    if locked.rows_affected() != 1 {
        let found = entity::Entity::find_by_id(&member_id)
            .one(&txn)
            .await
            .map_err(MemberError::Database)?;
        return Err(if found.is_some() {
            MemberError::RevisionConflict
        } else {
            MemberError::NotFound
        }
        .into());
    }
    let (member, inputs) = load(&txn, &member_id, expected_revision).await?;
    let baseline = combine(&member, inputs);
    if !baseline.valid || baseline.file_version.as_deref() != Some(&expected_file_version) {
        return Err(write::conflict());
    }
    let encoded = merge::encode(&draft).ok_or_else(|| {
        AppCommandError::invalid_input("Configuration draft exceeds its size limit")
    })?;
    let patch = merge::parse(&encoded, &member, true).map_err(|_| {
        AppCommandError::invalid_input("Configuration draft is invalid; preview it again")
    })?;
    let bytes = merge::encode(&merge::document(&patch)).ok_or_else(|| {
        AppCommandError::invalid_input("Configuration draft exceeds its size limit")
    })?;
    let result = tokio::task::spawn_blocking(move || {
        write::replace(&member, &expected_file_version, &bytes)
    })
    .await
    .map_err(|_| write::failed())??;
    txn.commit().await.map_err(|_| write::failed())?;
    Ok(result)
}
