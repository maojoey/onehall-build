//! Immutable host-local records. Full payloads never cross the command boundary.
//! SQLite storage is not encryption, a launch ticket or cross-host identity.
use super::super::{entity, MemberError};
use super::execution::{capture, digest_valid, invalid_archive, ExecutionSummary, FrozenCandidate};
use crate::app_error::AppCommandError;
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, Statement,
};
use serde::Serialize;

// No Debug/Serialize: SQL parameters and the private payload must not be logged
// or returned with a database error. Only SnapshotRecord is a public DTO.
#[derive(FromQueryResult)]
struct StoredSnapshot {
    snapshot_id: String,
    member_id: String,
    member_revision: i64,
    check_version: String,
    snapshot_version: String,
    payload: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRecord {
    project_id: Option<i32>,
    snapshot_id: String,
    member_id: String,
    member_revision: i64,
    check_version: String,
    created_at: DateTime<Utc>,
    launch_allowed: bool,
    summary: ExecutionSummary,
}

impl StoredSnapshot {
    fn record(self) -> Result<SnapshotRecord, AppCommandError> {
        let frozen = FrozenCandidate::restore(&self.payload)?;
        let (member_id, revision, check_version) = frozen.identity();
        let summary = frozen.summary()?;
        if uuid::Uuid::parse_str(&self.snapshot_id).is_err()
            || member_id != self.member_id
            || revision != self.member_revision
            || check_version != self.check_version
            || summary.snapshot_version != self.snapshot_version
        {
            return Err(invalid_archive());
        }
        Ok(SnapshotRecord {
            project_id: frozen.project_id(),
            snapshot_id: self.snapshot_id,
            member_id: self.member_id,
            member_revision: self.member_revision,
            check_version: self.check_version,
            created_at: self.created_at,
            launch_allowed: false,
            summary,
        })
    }
}

fn storage_error(_: sea_orm::DbErr) -> AppCommandError {
    // Do not use the generic database error, which may contain a query or values.
    AppCommandError::io_error("Execution snapshot storage is unavailable")
}

pub async fn hall_save_member_snapshot_core(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    expected_check_version: String,
    expected_snapshot_version: String,
) -> Result<SnapshotRecord, AppCommandError> {
    if !digest_valid(&expected_snapshot_version) {
        return Err(AppCommandError::invalid_input(
            "A valid candidate fingerprint is required",
        ));
    }
    let (_, frozen) = capture(
        db,
        member_id.clone(),
        expected_revision,
        expected_check_version.clone(),
    )
    .await?;
    let frozen = frozen.ok_or(MemberError::RevisionConflict)?;
    if frozen.summary()?.snapshot_version != expected_snapshot_version {
        return Err(MemberError::RevisionConflict.into());
    }
    let payload = frozen.encoded()?;
    if payload.len() > 1_048_576 {
        return Err(AppCommandError::invalid_input(
            "Execution snapshot exceeds the storage limit",
        ));
    }
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    // The conditional insert also catches a registry edit between capture and
    // storage. The unique index makes concurrent retries keep the original row.
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_execution_snapshot (snapshot_id, member_id, \
            member_revision, check_version, snapshot_version, payload, \
            created_at) SELECT ?, ?, ?, ?, ?, ?, ? FROM hall_member WHERE \
            member_id = ? AND revision = ? ON CONFLICT(member_id, \
            snapshot_version) DO NOTHING",
        vec![
            snapshot_id.into(),
            member_id.clone().into(),
            expected_revision.into(),
            expected_check_version.into(),
            expected_snapshot_version.clone().into(),
            payload.into(),
            Utc::now().into(),
            member_id.clone().into(),
            expected_revision.into(),
        ],
    ))
    .await
    .map_err(storage_error)?;
    let row = StoredSnapshot::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_execution_snapshot WHERE member_id = ? AND snapshot_version = ?",
        vec![member_id.into(), expected_snapshot_version.into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)?
    .ok_or(MemberError::RevisionConflict)?;
    row.record()
}

pub async fn hall_list_member_snapshots_core(
    db: &DatabaseConnection,
    member_id: String,
) -> Result<Vec<SnapshotRecord>, AppCommandError> {
    if uuid::Uuid::parse_str(&member_id).is_err() {
        return Err(AppCommandError::invalid_input(
            "A valid member ID is required",
        ));
    }
    entity::Entity::find_by_id(&member_id)
        .one(db)
        .await
        .map_err(storage_error)?
        .ok_or(MemberError::NotFound)?;
    StoredSnapshot::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_execution_snapshot WHERE member_id = ? ORDER \
            BY created_at DESC, snapshot_id DESC LIMIT 20",
        vec![member_id.into()],
    ))
    .all(db)
    .await
    .map_err(storage_error)?
    .into_iter()
    .map(StoredSnapshot::record)
    .collect()
}

/// Safe immutable identity for task references. Not a launch ticket.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotReference {
    pub snapshot_id: String,
    pub member_id: String,
    pub member_revision: i64,
    pub project_id: Option<i32>,
    pub snapshot_version: String,
}

pub(crate) async fn snapshot_reference<C: ConnectionTrait>(
    db: &C,
    member_id: &str,
    snapshot_id: &str,
) -> Result<SnapshotReference, AppCommandError> {
    let row = StoredSnapshot::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_execution_snapshot WHERE snapshot_id = ? AND member_id = ?",
        vec![snapshot_id.to_owned().into(), member_id.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| {
        AppCommandError::invalid_input("Select a saved snapshot belonging to this member")
    })?;
    let frozen = FrozenCandidate::restore(&row.payload)?;
    let project_id = frozen.project_id();
    // Reuse the full stored metadata/content validation before accepting it.
    let record = row.record()?;
    Ok(SnapshotReference {
        snapshot_id: record.snapshot_id,
        member_id: record.member_id,
        member_revision: record.member_revision,
        project_id,
        snapshot_version: record.summary.snapshot_version,
    })
}

pub(crate) async fn snapshot_launch_profile<C: ConnectionTrait>(
    db: &C,
    member_id: &str,
    snapshot_id: &str,
) -> Result<super::execution::LaunchProfile, AppCommandError> {
    snapshot_reference(db, member_id, snapshot_id).await?;
    let row = StoredSnapshot::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_execution_snapshot WHERE snapshot_id=? AND \
            member_id=?",
        vec![snapshot_id.to_owned().into(), member_id.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)?
    .ok_or_else(invalid_archive)?;
    let nonce = uuid::Uuid::parse_str(&row.snapshot_id).map_err(|_| invalid_archive())?;
    Ok(FrozenCandidate::restore(&row.payload)?.launch_profile(nonce))
}
