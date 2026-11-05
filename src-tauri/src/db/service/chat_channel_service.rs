use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::chat_channel;
use crate::db::error::DbError;

pub async fn create(
    conn: &DatabaseConnection,
    name: String,
    channel_type: String,
    config_json: String,
    enabled: bool,
    daily_report_enabled: bool,
    daily_report_time: Option<String>,
) -> Result<chat_channel::Model, DbError> {
    let now = Utc::now();
    let active = chat_channel::ActiveModel {
        id: NotSet,
        name: Set(name),
        channel_type: Set(channel_type),
        enabled: Set(enabled),
        config_json: Set(config_json),
        event_filter_json: Set(None),
        daily_report_enabled: Set(daily_report_enabled),
        daily_report_time: Set(daily_report_time),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(active.insert(conn).await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    name: Option<String>,
    enabled: Option<bool>,
    config_json: Option<String>,
    event_filter_json: Option<Option<String>>,
    daily_report_enabled: Option<bool>,
    daily_report_time: Option<Option<String>>,
) -> Result<chat_channel::Model, DbError> {
    let model = chat_channel::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("chat channel not found: {id}")))?;

    let mut active = model.into_active_model();
    if let Some(v) = name {
        active.name = Set(v);
    }
    if let Some(v) = enabled {
        active.enabled = Set(v);
    }
    if let Some(v) = config_json {
        active.config_json = Set(v);
    }
    if let Some(v) = event_filter_json {
        active.event_filter_json = Set(v);
    }
    if let Some(v) = daily_report_enabled {
        active.daily_report_enabled = Set(v);
    }
    if let Some(v) = daily_report_time {
        active.daily_report_time = Set(v);
    }
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    chat_channel::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn get_by_id(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<Option<chat_channel::Model>, DbError> {
    Ok(chat_channel::Entity::find_by_id(id).one(conn).await?)
}

pub async fn list_all(conn: &DatabaseConnection) -> Result<Vec<chat_channel::Model>, DbError> {
    Ok(chat_channel::Entity::find()
        .order_by_asc(chat_channel::Column::Id)
        .all(conn)
        .await?)
}

pub async fn list_enabled(conn: &DatabaseConnection) -> Result<Vec<chat_channel::Model>, DbError> {
    Ok(chat_channel::Entity::find()
        .filter(chat_channel::Column::Enabled.eq(true))
        .order_by_asc(chat_channel::Column::Id)
        .all(conn)
        .await?)
}
