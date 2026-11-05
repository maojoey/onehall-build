use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A host-local member identity, separate from an upstream assistant preset.
/// Registration is metadata only; execution and activation are not provided.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "hall_member")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub member_id: String,
    pub name: String,
    /// A descriptive label, never an authorization claim.
    pub role: String,
    #[sea_orm(unique)]
    pub cwd: String,
    /// Backend-local root folder identity; does not change cwd or grant access.
    pub project_id: Option<i32>,
    pub engine: String,
    pub assistant_uid: Option<String>,
    /// Version observed when registered, not a saved execution-config snapshot.
    pub assistant_revision: Option<i64>,
    pub enabled: bool,
    pub revision: i64,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
