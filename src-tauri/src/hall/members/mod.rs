//! Passive member registry. No agent startup, directory mutation, discovery or
//! authority is implied by registering a row or describing a member's role.

use std::path::{Component, Path};

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, SqlErr, TransactionTrait,
};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::db::entities::assistant;
use crate::db::error::DbError;

pub mod api;
mod config;
mod entity;
pub(crate) mod launch;
pub(crate) mod codex_bridge;
pub(crate) mod execution_policy;
pub(crate) mod migration;
pub(crate) mod project_migration;
mod projects;
pub mod scope;
pub(crate) mod snapshot_migration;

pub use config::hall_save_member_config_core;
pub use config::{hall_check_member_core, MemberCheckReport};
pub use config::{hall_list_member_snapshots_core, hall_save_member_snapshot_core, SnapshotRecord};
pub use config::{hall_preview_member_config_core, MemberConfigPreview};
pub use config::{hall_preview_member_config_draft_core, MemberConfigDraftPreview};
pub use config::{hall_preview_member_execution_core, MemberExecutionPreview};
pub use entity::Model as HallMember;
pub use projects::{hall_list_member_projects_core, MemberProject};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterMemberDraft {
    pub name: String,
    pub role: String,
    pub cwd: String,
    pub assistant_uid: Option<String>,
    pub project_id: Option<i32>,
    /// `codex` (default) or `claude_code`; a bound preset must match it.
    #[serde(default)]
    pub engine: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateMemberDraft {
    pub name: String,
    pub role: String,
    pub assistant_uid: Option<String>,
    /// Absent keeps the current engine unless a bound preset decides it.
    #[serde(default)]
    pub engine: Option<String>,
    /// Absent means keep (older clients); explicit null clears the binding.
    #[serde(default, deserialize_with = "project_change")]
    pub project_id: Option<Option<i32>>,
}

fn project_change<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<i32>>, D::Error> {
    Option::<i32>::deserialize(deserializer).map(Some)
}

#[derive(Debug, thiserror::Error)]
enum MemberError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("A member is already registered for this directory")]
    DuplicateDirectory,
    #[error("Member not found")]
    NotFound,
    #[error("Member changed elsewhere; refresh before editing again")]
    RevisionConflict,
    #[error(transparent)]
    Database(#[from] DbErr),
}

impl From<MemberError> for AppCommandError {
    fn from(error: MemberError) -> Self {
        match error {
            MemberError::InvalidInput(message) => Self::invalid_input(message),
            MemberError::DuplicateDirectory => Self::already_exists(error.to_string()),
            MemberError::NotFound => Self::not_found(error.to_string()),
            MemberError::RevisionConflict => Self::already_exists(error.to_string()),
            MemberError::Database(error) => Self::db(DbError::Database(error)),
        }
    }
}

fn invalid(message: impl Into<String>) -> MemberError {
    MemberError::InvalidInput(message.into())
}

fn inline_text(raw: &str, field: &str, limit: usize) -> Result<String, MemberError> {
    let text = raw.trim();
    if text.is_empty() || raw.chars().any(char::is_control) {
        return Err(invalid(format!(
            "Member {field} must be nonempty text without control characters"
        )));
    }
    if text.chars().count() > limit {
        return Err(invalid(format!(
            "Member {field} must be at most {limit} characters"
        )));
    }
    Ok(text.to_owned())
}

async fn member_directory(raw: &str) -> Result<String, MemberError> {
    let path = Path::new(raw);
    if !path.is_absolute()
        || path.parent().is_none()
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(invalid(
            "Member directory must be an absolute non-root path without '..'",
        ));
    }
    // Inspect every component: checking only the final entry would silently
    // follow an unconfirmed symlink in a parent directory. Registration never
    // reads or creates anything inside the member directory.
    for ancestor in path.ancestors() {
        let metadata = tokio::fs::symlink_metadata(ancestor)
            .await
            .map_err(|_| invalid("Member directory must exist and be accessible"))?;
        if metadata.file_type().is_symlink() {
            return Err(invalid(
                "Member directory contains a symlink; select its real path \
                instead",
            ));
        }
        if ancestor == path && !metadata.is_dir() {
            return Err(invalid("Member directory must be a directory"));
        }
    }
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|_| invalid("Cannot resolve member directory"))?;
    // `/.' also names the root but has a lexical parent. Check the resolved
    // path as well so aliases cannot bypass the non-root rule.
    if canonical.parent().is_none() {
        return Err(invalid("Member directory must not be a filesystem root"));
    }
    canonical
        .into_os_string()
        .into_string()
        .map_err(|_| invalid("Member directory must be valid Unicode"))
}

async fn resolve_template(
    db: &DatabaseConnection,
    uid: Option<String>,
) -> Result<Option<assistant::Model>, MemberError> {
    match uid {
        Some(uid) => Ok(Some(
            assistant::Entity::find()
                .filter(assistant::Column::Uid.eq(uid))
                .one(db)
                .await?
                .filter(|row| {
                    row.enabled
                        && row.deleted_at.is_none()
                        && crate::hall::engine::supported(&row.agent_type)
                })
                .ok_or_else(|| {
                    invalid("Assistant must be an existing enabled Codex or Claude Code preset")
                })?,
        )),
        None => Ok(None),
    }
}

/// The engine a member runs on: a bound preset decides it; otherwise the
/// requested one. `None` means "no change requested" (edits only).
fn member_engine(
    template: Option<&assistant::Model>,
    requested: Option<String>,
) -> Result<Option<String>, MemberError> {
    if let Some(engine) = &requested {
        if !crate::hall::engine::supported(engine) {
            return Err(invalid("Engine must be codex or claude_code"));
        }
    }
    match (template, requested) {
        (Some(row), Some(engine)) if row.agent_type != engine => {
            Err(invalid("The assistant preset runs on a different engine"))
        }
        (Some(row), _) => Ok(Some(row.agent_type.clone())),
        (None, requested) => Ok(requested),
    }
}

async fn register(
    db: &DatabaseConnection,
    draft: RegisterMemberDraft,
) -> Result<HallMember, MemberError> {
    let name = inline_text(&draft.name, "name", 80)?;
    let role = inline_text(&draft.role, "role", 200)?;
    let cwd = member_directory(&draft.cwd).await?;
    let template = resolve_template(db, draft.assistant_uid).await?;
    let engine = member_engine(template.as_ref(), draft.engine)?
        .unwrap_or_else(|| crate::hall::engine::DEFAULT.to_owned());
    projects::validate_project(db, draft.project_id).await?;
    let now = Utc::now();
    entity::ActiveModel {
        member_id: Set(uuid::Uuid::new_v4().to_string()),
        name: Set(name),
        role: Set(role),
        cwd: Set(cwd),
        project_id: Set(draft.project_id),
        engine: Set(engine),
        assistant_uid: Set(template.as_ref().map(|row| row.uid.clone())),
        assistant_revision: Set(template.as_ref().map(|row| row.revision)),
        enabled: Set(false),
        revision: Set(1),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .map_err(|error| match error.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(_)) => MemberError::DuplicateDirectory,
        _ => MemberError::Database(error),
    })
}

pub async fn hall_register_member_core(
    db: &DatabaseConnection,
    draft: RegisterMemberDraft,
) -> Result<HallMember, AppCommandError> {
    register(db, draft).await.map_err(Into::into)
}

pub async fn hall_list_members_core(
    db: &DatabaseConnection,
) -> Result<Vec<HallMember>, AppCommandError> {
    entity::Entity::find()
        .order_by_asc(entity::Column::CreatedAt)
        .order_by_asc(entity::Column::MemberId)
        .all(db)
        .await
        .map_err(|error| MemberError::Database(error).into())
}

async fn update(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    draft: UpdateMemberDraft,
) -> Result<HallMember, MemberError> {
    if uuid::Uuid::parse_str(&member_id).is_err() || expected_revision < 1 {
        return Err(invalid(
            "A valid member ID and positive revision are required",
        ));
    }
    let name = inline_text(&draft.name, "name", 80)?;
    let role = inline_text(&draft.role, "role", 200)?;
    let template = resolve_template(db, draft.assistant_uid).await?;
    let engine = member_engine(template.as_ref(), draft.engine)?;
    let uid = template.as_ref().map(|row| row.uid.clone());
    let revision = template.as_ref().map(|row| row.revision);
    if let Some(project_id) = draft.project_id {
        projects::validate_project(db, project_id).await?;
    }

    // The first statement in this transaction is the write: a preceding read
    // could become an un-upgradable WAL snapshot. The revision check is part
    // of the UPDATE, not a separate preflight check. Read back under the same
    // write lock so the reply describes exactly this edit, not a later one.
    let txn = db.begin().await?;
    let mut edit = entity::Entity::update_many()
        .col_expr(entity::Column::Name, Expr::value(name))
        .col_expr(entity::Column::Role, Expr::value(role))
        .col_expr(entity::Column::AssistantUid, Expr::value(uid))
        .col_expr(entity::Column::AssistantRevision, Expr::value(revision))
        .col_expr(
            entity::Column::Revision,
            Expr::col(entity::Column::Revision).add(1),
        )
        .col_expr(entity::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(entity::Column::MemberId.eq(&member_id))
        .filter(entity::Column::Revision.eq(expected_revision));
    if let Some(project_id) = draft.project_id {
        edit = edit.col_expr(entity::Column::ProjectId, Expr::value(project_id));
    }
    if let Some(engine) = engine {
        edit = edit.col_expr(entity::Column::Engine, Expr::value(engine));
    }
    let changed = edit.exec(&txn).await?;
    let row = entity::Entity::find_by_id(member_id).one(&txn).await?;
    if changed.rows_affected == 0 {
        return Err(if row.is_some() {
            MemberError::RevisionConflict
        } else {
            MemberError::NotFound
        });
    }
    let row = row.ok_or(MemberError::NotFound)?;
    txn.commit().await?;
    Ok(row)
}

pub async fn hall_update_member_core(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    draft: UpdateMemberDraft,
) -> Result<HallMember, AppCommandError> {
    update(db, member_id, expected_revision, draft)
        .await
        .map_err(Into::into)
}

/// The member's dispatch switch. Operational only: it neither bumps the
/// revision nor touches reviewed launch plans, so approvals stay valid.
pub async fn hall_set_member_enabled_core(
    db: &DatabaseConnection,
    member_id: String,
    enabled: bool,
) -> Result<HallMember, AppCommandError> {
    if uuid::Uuid::parse_str(&member_id).is_err() {
        return Err(invalid("A valid member ID is required").into());
    }
    let txn = db.begin().await.map_err(MemberError::from)?;
    let changed = entity::Entity::update_many()
        .col_expr(entity::Column::Enabled, Expr::value(enabled))
        .col_expr(entity::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(entity::Column::MemberId.eq(&member_id))
        .exec(&txn)
        .await
        .map_err(MemberError::from)?;
    if changed.rows_affected == 0 {
        return Err(MemberError::NotFound.into());
    }
    let row = entity::Entity::find_by_id(member_id)
        .one(&txn)
        .await
        .map_err(MemberError::from)?
        .ok_or(MemberError::NotFound)?;
    txn.commit().await.map_err(MemberError::from)?;
    Ok(row)
}

pub(crate) use config::snapshot_reference;
pub use config::SnapshotReference;

pub(crate) use config::snapshot_launch_profile;
pub use config::{LaunchProfile, MemberAccess};
pub use execution_policy::{ExecutionCapabilities, ExecutionMode};
