//! Project bindings are metadata, not permission grants or execution cwd.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;

use super::{invalid, MemberError};
use crate::app_error::AppCommandError;
use crate::db::entities::folder::{self, FolderKind};

#[derive(Debug, Serialize)]
pub struct MemberProject {
    pub id: i32,
    pub name: String,
    pub alias: Option<String>,
    pub path: String,
}

fn live_projects() -> sea_orm::Select<folder::Entity> {
    folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::ParentId.is_null())
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
}

pub(super) async fn validate_project(
    db: &DatabaseConnection,
    project_id: Option<i32>,
) -> Result<(), MemberError> {
    if let Some(id) = project_id {
        if id < 1
            || live_projects()
                .filter(folder::Column::Id.eq(id))
                .one(db)
                .await?
                .is_none()
        {
            return Err(invalid(
                "Project must be an existing regular root folder on this backend",
            ));
        }
    }
    Ok(())
}

pub async fn hall_list_member_projects_core(
    db: &DatabaseConnection,
) -> Result<Vec<MemberProject>, AppCommandError> {
    live_projects()
        .order_by_asc(folder::Column::SortOrder)
        .order_by_asc(folder::Column::Id)
        .all(db)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| MemberProject {
                    id: row.id,
                    name: row.name,
                    alias: row.alias,
                    path: row.path,
                })
                .collect()
        })
        .map_err(|error| MemberError::Database(error).into())
}
