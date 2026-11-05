//! Application-user API. Model tools use TeamService with host-issued Actor;
//! these commands retain the application's existing authenticated-user boundary.
use super::mailbox;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindParams {
    pub member_id: Option<String>,
    pub expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxParams {
    pub member_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AckParams {
    pub member_id: String,
    pub message_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendParams {
    pub draft: mailbox::MessageDraft,
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_get_pm_binding(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<mailbox::RoleBinding, crate::app_error::AppCommandError> {
    mailbox::role(&db.conn).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_bind_pm(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: Option<String>,
    expected_revision: i64,
) -> Result<mailbox::RoleBinding, crate::app_error::AppCommandError> {
    mailbox::bind_pm(&db.conn, member_id, expected_revision).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_read_mailbox(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
) -> Result<Vec<mailbox::Message>, crate::app_error::AppCommandError> {
    mailbox::inbox(&db.conn, &member_id).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_ack_message(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    message_id: String,
) -> Result<(), crate::app_error::AppCommandError> {
    mailbox::acknowledge(&db.conn, &member_id, &message_id).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_send_message(
    db: tauri::State<'_, crate::db::AppDatabase>,
    draft: mailbox::MessageDraft,
) -> Result<mailbox::Message, crate::app_error::AppCommandError> {
    mailbox::send_as_user(&db.conn, draft).await
}
pub mod web {
    use super::*;
    use crate::{app_error::AppCommandError, app_state::AppState};
    use axum::{Extension, Json};
    use std::sync::Arc;
    pub async fn hall_get_pm_binding(
        Extension(s): Extension<Arc<AppState>>,
    ) -> Result<Json<mailbox::RoleBinding>, AppCommandError> {
        mailbox::role(&s.db.conn).await.map(Json)
    }
    pub async fn hall_bind_pm(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<BindParams>,
    ) -> Result<Json<mailbox::RoleBinding>, AppCommandError> {
        mailbox::bind_pm(&s.db.conn, p.member_id, p.expected_revision)
            .await
            .map(Json)
    }
    pub async fn hall_read_mailbox(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<InboxParams>,
    ) -> Result<Json<Vec<mailbox::Message>>, AppCommandError> {
        mailbox::inbox(&s.db.conn, &p.member_id).await.map(Json)
    }
    pub async fn hall_ack_message(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<AckParams>,
    ) -> Result<Json<()>, AppCommandError> {
        mailbox::acknowledge(&s.db.conn, &p.member_id, &p.message_id)
            .await
            .map(Json)
    }
    pub async fn hall_send_message(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<SendParams>,
    ) -> Result<Json<mailbox::Message>, AppCommandError> {
        mailbox::send_as_user(&s.db.conn, p.draft).await.map(Json)
    }
}
