use super::types::*;
use crate::app_error::AppCommandError;
use crate::hall::members::{snapshot_reference, SnapshotReference};
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};

pub(super) fn storage_error(_: sea_orm::DbErr) -> AppCommandError {
    AppCommandError::io_error("Team task storage is unavailable")
}
fn corrupt() -> AppCommandError {
    AppCommandError::io_error("Stored team task is invalid")
}
pub(super) fn conflict() -> AppCommandError {
    AppCommandError::already_exists("Task changed or is no longer a draft; refresh before editing")
}
fn invalid() -> AppCommandError {
    AppCommandError::invalid_input("Invalid team task fields")
}
fn uuid(value: &str) -> Result<String, AppCommandError> {
    uuid::Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| invalid())
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn text(title: &str, description: &str) -> Result<(String, String), AppCommandError> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count() > 160
        || title.chars().any(char::is_control)
        || description.chars().count() > 8000
        || description
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r')
    {
        return Err(invalid());
    }
    Ok((title.into(), description.into()))
}
pub(super) fn validate(row: &TaskRow) -> Result<(), AppCommandError> {
    if uuid(&row.task_id).is_err()
        || uuid(&row.submission_id).is_err()
        || uuid(&row.member_id).is_err()
        || uuid(&row.snapshot_id).is_err()
        || !digest(&row.request_hash)
        || !digest(&row.snapshot_version)
        || row.snapshot_member_revision < 1
        || row.revision < 1
        || row.project_id.is_some_and(|id| id < 1)
        || !matches!(
            row.status.as_str(),
            "draft"
                | "cancelled"
                | "queued"
                | "interrupted"
                | "running"
                | "awaiting_permission"
                | "succeeded"
                | "failed"
        )
        || !matches!(row.purpose.as_str(), "work" | "acceptance")
        || text(&row.title, &row.description).is_err()
        || row.updated_at < row.created_at
    {
        return Err(corrupt());
    }
    Ok(())
}
fn summary(row: TaskRow) -> Result<TaskSummary, AppCommandError> {
    validate(&row)?;
    Ok(TaskSummary {
        snapshot: SnapshotReference {
            snapshot_id: row.snapshot_id,
            member_id: row.member_id.clone(),
            member_revision: row.snapshot_member_revision,
            project_id: row.project_id,
            snapshot_version: row.snapshot_version,
        },
        task_id: row.task_id,
        member_id: row.member_id,
        project_id: row.project_id,
        purpose: if row.purpose == "work" {
            TaskPurpose::Work
        } else {
            TaskPurpose::Acceptance
        },
        title: row.title,
        status: row.status,
        revision: row.revision,
        created_at: row.created_at,
        updated_at: row.updated_at,
        launch_allowed: false,
        runtime_verified: false,
    })
}
pub(super) async fn by_id<C: ConnectionTrait>(
    db: &C,
    id: &str,
) -> Result<TaskRow, AppCommandError> {
    TaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_task WHERE task_id = ?",
        vec![id.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| AppCommandError::not_found("Team task not found"))
}
async fn by_submission<C: ConnectionTrait>(
    db: &C,
    id: &str,
) -> Result<Option<TaskRow>, AppCommandError> {
    TaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_task WHERE submission_id = ?",
        vec![id.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(storage_error)
}
async fn history<C: ConnectionTrait>(
    db: &C,
    id: &str,
    revision: i64,
    action: &str,
) -> Result<(), AppCommandError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_task_history (task_id,revision,action,created_at) VALUES (?,?,?,?)",
        vec![
            id.to_owned().into(),
            revision.into(),
            action.to_owned().into(),
            Utc::now().into(),
        ],
    ))
    .await
    .map_err(storage_error)?;
    Ok(())
}

pub async fn create_task<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    mut draft: CreateTaskDraft,
) -> Result<TaskSummary, AppCommandError> {
    draft.submission_id = uuid(&draft.submission_id)?;
    draft.member_id = uuid(&draft.member_id)?;
    draft.snapshot_id = uuid(&draft.snapshot_id)?;
    (draft.title, draft.description) = text(&draft.title, &draft.description)?;
    if draft.project_id.is_some_and(|id| id < 1) {
        return Err(invalid());
    }
    let mut hasher = Sha256::new();
    hasher.update(b"onehall-task-create-v1");
    hasher.update(serde_json::to_vec(&draft).map_err(|_| invalid())?);
    let hash = format!("{:x}", hasher.finalize());
    if let Some(row) = by_submission(db, &draft.submission_id).await? {
        if row.request_hash != hash {
            return Err(conflict());
        }
        return summary(row);
    }
    let saved = snapshot_reference(db, &draft.member_id, &draft.snapshot_id).await?;
    if saved.project_id != draft.project_id {
        return Err(AppCommandError::invalid_input(
            "Task project must match the saved member snapshot",
        ));
    }
    if let Some(id) = draft.project_id {
        use crate::db::entities::folder::{self, FolderKind};
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        if folder::Entity::find_by_id(id)
            .filter(folder::Column::DeletedAt.is_null())
            .filter(folder::Column::ParentId.is_null())
            .filter(folder::Column::Kind.eq(FolderKind::Regular))
            .one(db)
            .await
            .map_err(storage_error)?
            .is_none()
        {
            return Err(AppCommandError::invalid_input(
                "Task project is unavailable on this backend",
            ));
        }
    }
    let task_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    let txn = db.begin().await.map_err(storage_error)?;
    // First transaction operation is a write: avoids SQLite read→write lock
    // upgrades when two callers concurrently retry a submission.
    let inserted = txn
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO hall_task (task_id,submission_id,request_hash,\
            member_id,project_id,snapshot_id,snapshot_version,\
            snapshot_member_revision,purpose,title,description,status,\
            revision,created_at,updated_at) SELECT ?,?,?,?,?,?,?,?,?,?,?,\
            'draft',1,?,? FROM hall_member m JOIN hall_execution_snapshot s \
            ON s.member_id=m.member_id WHERE m.member_id=? AND \
            s.snapshot_id=? AND s.snapshot_version=? AND \
            s.member_revision=? AND (? IS NULL OR EXISTS(SELECT 1 FROM \
            folder f WHERE f.id=? AND f.deleted_at IS NULL AND f.parent_id \
            IS NULL AND f.kind='regular')) ON CONFLICT(submission_id) DO \
            NOTHING",
            vec![
                task_id.clone().into(),
                draft.submission_id.clone().into(),
                hash.clone().into(),
                draft.member_id.clone().into(),
                draft.project_id.into(),
                draft.snapshot_id.clone().into(),
                saved.snapshot_version.clone().into(),
                saved.member_revision.into(),
                draft.purpose.as_str().into(),
                draft.title.into(),
                draft.description.into(),
                now.into(),
                now.into(),
                draft.member_id.into(),
                draft.snapshot_id.into(),
                saved.snapshot_version.into(),
                saved.member_revision.into(),
                draft.project_id.into(),
                draft.project_id.into(),
            ],
        ))
        .await
        .map_err(storage_error)?;
    let row = by_submission(&txn, &draft.submission_id)
        .await?
        .ok_or_else(conflict)?;
    if row.request_hash != hash {
        return Err(conflict());
    }
    if inserted.rows_affected() == 1 {
        history(&txn, &task_id, 1, "draft_created").await?;
    }
    let result = summary(row)?;
    txn.commit().await.map_err(storage_error)?;
    Ok(result)
}

pub async fn list_tasks(
    db: &DatabaseConnection,
    filter: TaskFilter,
) -> Result<Vec<TaskSummary>, AppCommandError> {
    if filter.project_id.is_some_and(|id| id < 1)
        || (filter.project_id.is_some() && filter.unassigned_project)
    {
        return Err(invalid());
    }
    let mut sql = "SELECT * FROM hall_task WHERE 1=1".to_owned();
    let mut args = Vec::new();
    if let Some(id) = filter.member_id {
        sql.push_str(" AND member_id=?");
        args.push(uuid(&id)?.into());
    }
    if let Some(id) = filter.project_id {
        sql.push_str(" AND project_id=?");
        args.push(id.into());
    }
    if filter.unassigned_project {
        sql.push_str(" AND project_id IS NULL");
    }
    sql.push_str(" ORDER BY created_at DESC,task_id DESC LIMIT 100");
    let txn = db.begin().await.map_err(storage_error)?;
    let rows =
        TaskRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Sqlite, sql, args))
            .all(&txn)
            .await
            .map_err(storage_error)?;
    let mut summaries = Vec::new();
    for row in rows {
        let latest = AttemptRecord::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT * FROM hall_task_attempt WHERE task_id=? ORDER BY \
            attempt_number DESC LIMIT 1",
            vec![row.task_id.clone().into()],
        ))
        .one(&txn)
        .await
        .map_err(storage_error)?;
        if let Some(a) = latest {
            let expected = match a.status.as_str() {
                "starting" => "queued",
                "cancel_requested" => "running",
                other => other,
            };
            if row.status != expected || a.started_at.is_some() != a.conversation_id.is_some() {
                return Err(corrupt());
            }
            if matches!(
                a.status.as_str(),
                "running" | "awaiting_permission" | "cancel_requested" | "succeeded"
            ) && a.started_at.is_none()
            {
                return Err(corrupt());
            }
            if a.started_at.is_some() {
                let runtime = super::runtime::RuntimeObservation::find_by_statement(
                    Statement::from_sql_and_values(
                        DbBackend::Sqlite,
                        "SELECT * FROM hall_task_runtime WHERE attempt_id=?",
                        vec![a.attempt_id.into()],
                    ),
                )
                .one(&txn)
                .await
                .map_err(storage_error)?
                .ok_or_else(corrupt)?;
                if a.finished_at.is_some()
                    && (a.finished_at != runtime.exit_confirmed_at || runtime.stage != "terminal")
                {
                    return Err(corrupt());
                }
            }
        } else if !matches!(row.status.as_str(), "draft" | "cancelled") {
            return Err(corrupt());
        }
        summaries.push(summary(row)?);
    }
    txn.commit().await.map_err(storage_error)?;
    Ok(summaries)
}

pub async fn get_task(
    db: &DatabaseConnection,
    task_id: String,
) -> Result<TaskDetail, AppCommandError> {
    let txn = db.begin().await.map_err(storage_error)?;
    let id = uuid(&task_id)?;
    let row = by_id(&txn, &id).await?;
    validate(&row)?;
    let saved = snapshot_reference(&txn, &row.member_id, &row.snapshot_id).await?;
    if saved.snapshot_version != row.snapshot_version
        || saved.member_revision != row.snapshot_member_revision
        || saved.project_id != row.project_id
    {
        return Err(corrupt());
    }
    let attempts = AttemptRecord::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_task_attempt WHERE task_id=? ORDER BY \
            attempt_number",
        vec![id.clone().into()],
    ))
    .all(&txn)
    .await
    .map_err(storage_error)?;
    for attempt in &attempts {
        if uuid(&attempt.attempt_id).is_err()
            || attempt.task_id != row.task_id
            || attempt.member_id != row.member_id
            || attempt.snapshot_id != row.snapshot_id
            || attempt.snapshot_version != row.snapshot_version
            || attempt.attempt_number < 1
            || attempt.revision < 1
            || attempt.conversation_id.is_some_and(|id| id < 1)
            || attempt
                .result_id
                .as_ref()
                .is_some_and(|id| uuid(id).is_err())
            || !matches!(
                attempt.status.as_str(),
                "queued"
                    | "starting"
                    | "running"
                    | "awaiting_permission"
                    | "cancel_requested"
                    | "cancelled"
                    | "succeeded"
                    | "failed"
                    | "interrupted"
            )
        {
            return Err(corrupt());
        }
    }
    let passive = attempts.is_empty();
    if passive && !matches!(row.status.as_str(), "draft" | "cancelled") {
        return Err(corrupt());
    }
    let has_runtime = txn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND \
        name='hall_task_runtime'",
        ))
        .await
        .map_err(storage_error)?
        .is_some();
    if !has_runtime
        && txn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT 1 FROM seaql_migrations WHERE \
        version='onehall_m20261025_000004_team_runtime'",
            ))
            .await
            .map_err(storage_error)?
            .is_some()
    {
        return Err(corrupt());
    }
    let runtimes = if has_runtime {
        super::runtime::RuntimeObservation::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT r.* FROM hall_task_runtime r JOIN hall_task_attempt a ON \
        a.attempt_id=r.attempt_id WHERE a.task_id=? ORDER BY \
        a.attempt_number",
            vec![id.clone().into()],
        ))
        .all(&txn)
        .await
        .map_err(storage_error)?
    } else {
        Vec::new()
    };
    let results = if has_runtime {
        super::runtime::StoredResult::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT r.* FROM hall_task_result r JOIN hall_task_attempt a ON \
        a.attempt_id=r.attempt_id WHERE a.task_id=? ORDER BY \
        a.attempt_number",
            vec![id.clone().into()],
        ))
        .all(&txn)
        .await
        .map_err(storage_error)?
    } else {
        Vec::new()
    };
    for a in &attempts {
        let runtime = runtimes.iter().find(|r| r.attempt_id == a.attempt_id);
        let result = results.iter().find(|r| r.attempt_id == a.attempt_id);
        if a.result_id.as_deref() != result.map(|r| r.result_id.as_str())
            || a.started_at.is_some() != a.conversation_id.is_some()
            || a.finished_at.is_some()
                && (a.started_at.is_none()
                    || runtime.is_none_or(|r| {
                        r.exit_confirmed_at != a.finished_at || r.stage != "terminal"
                    }))
            || a.finished_at
                .zip(a.started_at)
                .is_some_and(|(end, start)| end < start)
            || matches!(
                a.status.as_str(),
                "running" | "awaiting_permission" | "cancel_requested" | "succeeded"
            ) && (runtime.is_none() || a.started_at.is_none())
            || a.started_at.is_some() && runtime.is_none()
        {
            return Err(corrupt());
        }
    }
    if let Some(a) = attempts.last() {
        let expected = match a.status.as_str() {
            "starting" => "queued",
            "cancel_requested" => "running",
            other => other,
        };
        if row.status != expected {
            return Err(corrupt());
        }
    }
    let has_events = txn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='hall_task_event'",
        ))
        .await
        .map_err(storage_error)?
        .is_some();
    let (history_sql, history_args) = if has_events {
        (
            "SELECT revision,action,created_at FROM (SELECT revision,action,\
            created_at FROM hall_task_history WHERE task_id=? UNION ALL \
            SELECT task_revision AS revision,action,created_at FROM \
            hall_task_event WHERE task_id=?) ORDER BY revision DESC LIMIT \
            100",
            vec![id.clone().into(), id.into()],
        )
    } else {
        // Read-only compatibility for the explicitly frozen ledger-60 fixture.
        // A missing table in a database claiming queue-61 is corruption, not a
        // reason to hide events or downgrade to another backend.
        let legacy = txn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT 1 FROM seaql_migrations WHERE \
                version='onehall_m20261025_000002_team_ledger' AND NOT \
                EXISTS(SELECT 1 FROM seaql_migrations WHERE \
                version='onehall_m20261025_000003_team_queue')",
            ))
            .await
            .map_err(storage_error)?
            .is_some();
        if !legacy || !passive {
            return Err(corrupt());
        }
        (
            "SELECT revision,action,created_at FROM hall_task_history WHERE \
            task_id=? ORDER BY revision DESC LIMIT 100",
            vec![id.into()],
        )
    };
    let history = HistoryRecord::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        history_sql,
        history_args,
    ))
    .all(&txn)
    .await
    .map_err(storage_error)?;
    if history.is_empty()
        || history[0].revision != row.revision
        || history.iter().any(|item| {
            item.revision < 1
                || !matches!(
                    item.action.as_str(),
                    "draft_created"
                        | "draft_updated"
                        | "draft_cancelled"
                        | "task_queued"
                        | "attempt_claimed"
                        | "attempt_interrupted"
                        | "dispatch_intent"
                        | "attempt_running"
                        | "attempt_rejected"
                        | "runtime_waiting"
                        | "runtime_running"
                        | "result_received"
                        | "cancel_requested"
                        | "cancel_acknowledged"
                        | "exit_confirmed"
                        | "queued_cancelled"
                        | "human_best_effort_resolution"
                        | "start_failed"
                        | "startup_recovered"
                        | "human_orphan_resolution"
                )
        })
    {
        return Err(corrupt());
    }
    if history.len() != (row.revision.min(100) as usize)
        || history
            .windows(2)
            .any(|pair| pair[0].revision != pair[1].revision + 1)
        || (history[0].action == "draft_cancelled" && row.status != "cancelled")
    {
        return Err(corrupt());
    }
    let has_exit_checks = txn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND \
        name='hall_task_exit_check'",
        ))
        .await
        .map_err(storage_error)?
        .is_some();
    let mut exit_checks = Vec::new();
    if has_exit_checks {
        for check in txn
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT c.report_json,c.scope_hash,c.lease_id FROM \
            hall_task_exit_check c JOIN hall_task_attempt a ON \
            a.attempt_id=c.attempt_id WHERE a.task_id=? ORDER BY \
            a.attempt_number",
                vec![row.task_id.clone().into()],
            ))
            .await
            .map_err(storage_error)?
        {
            let report: super::runtime::ExitObservation = serde_json::from_str(
                &check
                    .try_get::<String>("", "report_json")
                    .map_err(storage_error)?,
            )
            .map_err(|_| corrupt())?;
            if report.scope_hash
                != check
                    .try_get::<String>("", "scope_hash")
                    .map_err(storage_error)?
                || report.lease_id
                    != check
                        .try_get::<String>("", "lease_id")
                        .map_err(storage_error)?
            {
                return Err(corrupt());
            }
            exit_checks.push(report);
        }
    } else if txn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM seaql_migrations WHERE \
        version='onehall_m20261025_000006_team_exit_evidence'",
        ))
        .await
        .map_err(storage_error)?
        .is_some()
    {
        return Err(corrupt());
    }
    let launch_reviews = super::review::receipts(&txn, &row.task_id).await?;
    let issues = if txn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND \
        name='hall_task_issue'",
        ))
        .await
        .map_err(storage_error)?
        .is_some()
    {
        super::TaskIssue::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT attempt_id,tool,detail,created_at FROM hall_task_issue \
        WHERE task_id=? AND resolved_at IS NULL ORDER BY created_at LIMIT 20",
            vec![row.task_id.clone().into()],
        ))
        .all(&txn)
        .await
        .map_err(storage_error)?
    } else {
        Vec::new()
    };
    let has_recovery = txn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='hall_task_start_recovery'",
        ))
        .await
        .map_err(storage_error)?
        .is_some();
    let (start_recovery, resolution) = if has_recovery {
        let starts = super::runtime::StartObservation::find_by_statement(Statement::from_sql_and_values(DbBackend::Sqlite,
            "SELECT s.attempt_id,s.phase,s.failure,s.failure_code FROM hall_task_start_recovery s JOIN hall_task_attempt a ON a.attempt_id=s.attempt_id WHERE a.task_id=? ORDER BY a.attempt_number",
            vec![row.task_id.clone().into()],
        )).all(&txn).await.map_err(storage_error)?;
        let resolution = super::runtime::Resolution::find_by_statement(Statement::from_sql_and_values(DbBackend::Sqlite,
            "SELECT 'operator_released' AS kind,s.resolution_reason AS reason,s.resolved_at FROM hall_task_start_recovery s JOIN hall_task_attempt a ON a.attempt_id=s.attempt_id WHERE a.task_id=? AND s.resolved_at IS NOT NULL AND a.attempt_number=(SELECT MAX(attempt_number) FROM hall_task_attempt WHERE task_id=a.task_id)",
            vec![row.task_id.clone().into()],
        )).one(&txn).await.map_err(storage_error)?;
        (starts, resolution)
    } else {
        (Vec::new(), None)
    };
    let description = row.description.clone();
    txn.commit().await.map_err(storage_error)?;
    Ok(TaskDetail {
        task: summary(row)?,
        description,
        attempts,
        history,
        runtimes,
        results,
        issues,
        exit_checks,
        launch_reviews,
        start_recovery,
        resolution,
    })
}

async fn change(
    db: &DatabaseConnection,
    task_id: String,
    expected_revision: i64,
    draft: Option<EditTaskDraft>,
) -> Result<TaskSummary, AppCommandError> {
    let id = uuid(&task_id)?;
    if !(1..i64::MAX).contains(&expected_revision) {
        return Err(invalid());
    }
    let now = Utc::now();
    let (sql, args, action) = if let Some(draft) = draft {
        let (title, description) = text(&draft.title, &draft.description)?;
        (
            "UPDATE hall_task SET title=?,description=?,revision=revision+1,\
            updated_at=? WHERE task_id=? AND revision=? AND status='draft'",
            vec![
                title.into(),
                description.into(),
                now.into(),
                id.clone().into(),
                expected_revision.into(),
            ],
            "draft_updated",
        )
    } else {
        (
            "UPDATE hall_task SET status='cancelled',revision=revision+1,\
            updated_at=? WHERE task_id=? AND revision=? AND status='draft'",
            vec![now.into(), id.clone().into(), expected_revision.into()],
            "draft_cancelled",
        )
    };
    let txn = db.begin().await.map_err(storage_error)?;
    if txn
        .execute(Statement::from_sql_and_values(DbBackend::Sqlite, sql, args))
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(conflict());
    }
    history(&txn, &id, expected_revision + 1, action).await?;
    let result = summary(by_id(&txn, &id).await?)?;
    txn.commit().await.map_err(storage_error)?;
    Ok(result)
}
pub async fn edit_task(
    db: &DatabaseConnection,
    task_id: String,
    expected_revision: i64,
    draft: EditTaskDraft,
) -> Result<TaskSummary, AppCommandError> {
    change(db, task_id, expected_revision, Some(draft)).await
}
pub async fn cancel_task(
    db: &DatabaseConnection,
    task_id: String,
    expected_revision: i64,
) -> Result<TaskSummary, AppCommandError> {
    change(db, task_id, expected_revision, None).await
}
