#[cfg(feature = "tauri-runtime")]
use crate::db::error::DbError;
#[cfg(feature = "tauri-runtime")]
use crate::db::service::quick_message_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
#[cfg(feature = "tauri-runtime")]
use crate::models::QuickMessageInfo;

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn quick_messages_list(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<QuickMessageInfo>, DbError> {
    quick_message_service::list(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn quick_messages_create(
    db: tauri::State<'_, AppDatabase>,
    title: String,
    content: String,
) -> Result<QuickMessageInfo, DbError> {
    quick_message_service::create(&db.conn, &title, &content).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn quick_messages_update(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    title: Option<String>,
    content: Option<String>,
) -> Result<QuickMessageInfo, DbError> {
    quick_message_service::update(&db.conn, id, title, content).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn quick_messages_delete(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), DbError> {
    quick_message_service::delete(&db.conn, id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn quick_messages_reorder(
    db: tauri::State<'_, AppDatabase>,
    ids: Vec<i32>,
) -> Result<(), DbError> {
    quick_message_service::reorder(&db.conn, ids).await
}
