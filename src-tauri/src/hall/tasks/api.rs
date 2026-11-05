use super::{CreateTaskDraft, EditTaskDraft, TaskFilter};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    pub draft: CreateTaskDraft,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    #[serde(default)]
    pub filter: TaskFilter,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetParams {
    pub task_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelParams {
    pub task_id: String,
    pub expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditParams {
    pub task_id: String,
    pub expected_revision: i64,
    pub draft: EditTaskDraft,
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_create_task(
    db: tauri::State<'_, crate::db::AppDatabase>,
    draft: CreateTaskDraft,
) -> Result<super::TaskSummary, crate::app_error::AppCommandError> {
    super::create_task(&db.conn, draft).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_list_tasks(
    db: tauri::State<'_, crate::db::AppDatabase>,
    filter: TaskFilter,
) -> Result<Vec<super::TaskSummary>, crate::app_error::AppCommandError> {
    super::list_tasks(&db.conn, filter).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_get_task(
    db: tauri::State<'_, crate::db::AppDatabase>,
    task_id: String,
) -> Result<super::TaskDetail, crate::app_error::AppCommandError> {
    super::get_task(&db.conn, task_id).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_edit_task(
    db: tauri::State<'_, crate::db::AppDatabase>,
    task_id: String,
    expected_revision: i64,
    draft: EditTaskDraft,
) -> Result<super::TaskSummary, crate::app_error::AppCommandError> {
    super::edit_task(&db.conn, task_id, expected_revision, draft).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_cancel_task(
    db: tauri::State<'_, crate::db::AppDatabase>,
    task_id: String,
    expected_revision: i64,
) -> Result<super::TaskSummary, crate::app_error::AppCommandError> {
    super::cancel_task(&db.conn, task_id, expected_revision).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_review_task(
    db: tauri::State<'_, crate::db::AppDatabase>,
    task_id: String,
) -> Result<super::review::LaunchReview, crate::app_error::AppCommandError> {
    super::review::preview(&db.conn, &task_id).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_approve_task(
    db: tauri::State<'_, crate::db::AppDatabase>,
    approval: super::review::Approval,
) -> Result<super::queue::QueueReceipt, crate::app_error::AppCommandError> {
    super::review::approve(&db.conn, approval).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_cancel_queued_task(
    db: tauri::State<'_, crate::db::AppDatabase>,
    task_id: String,
    expected_revision: i64,
) -> Result<(), crate::app_error::AppCommandError> {
    super::queue::QueueHost::new(db.conn.clone(), 1)?
        .cancel_queued(&task_id, expected_revision)
        .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveParams {
    pub approval: super::review::Approval,
}
pub mod web {
    use super::*;
    use crate::{app_error::AppCommandError, app_state::AppState};
    use axum::{Extension, Json};
    use std::sync::Arc;
    pub async fn hall_create_task(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<CreateParams>,
    ) -> Result<Json<crate::hall::tasks::TaskSummary>, AppCommandError> {
        crate::hall::tasks::create_task(&s.db.conn, p.draft)
            .await
            .map(Json)
    }
    pub async fn hall_list_tasks(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<ListParams>,
    ) -> Result<Json<Vec<crate::hall::tasks::TaskSummary>>, AppCommandError> {
        crate::hall::tasks::list_tasks(&s.db.conn, p.filter)
            .await
            .map(Json)
    }
    pub async fn hall_get_task(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<GetParams>,
    ) -> Result<Json<crate::hall::tasks::TaskDetail>, AppCommandError> {
        crate::hall::tasks::get_task(&s.db.conn, p.task_id)
            .await
            .map(Json)
    }
    pub async fn hall_edit_task(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<EditParams>,
    ) -> Result<Json<crate::hall::tasks::TaskSummary>, AppCommandError> {
        crate::hall::tasks::edit_task(&s.db.conn, p.task_id, p.expected_revision, p.draft)
            .await
            .map(Json)
    }
    pub async fn hall_cancel_task(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<CancelParams>,
    ) -> Result<Json<crate::hall::tasks::TaskSummary>, AppCommandError> {
        crate::hall::tasks::cancel_task(&s.db.conn, p.task_id, p.expected_revision)
            .await
            .map(Json)
    }
    pub async fn hall_review_task(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<GetParams>,
    ) -> Result<Json<super::super::review::LaunchReview>, AppCommandError> {
        super::super::review::preview(&s.db.conn, &p.task_id)
            .await
            .map(Json)
    }
    pub async fn hall_approve_task(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<ApproveParams>,
    ) -> Result<Json<super::super::queue::QueueReceipt>, AppCommandError> {
        super::super::review::approve(&s.db.conn, p.approval)
            .await
            .map(Json)
    }
    pub async fn hall_cancel_queued_task(
        Extension(s): Extension<Arc<AppState>>,
        Json(p): Json<CancelParams>,
    ) -> Result<Json<()>, AppCommandError> {
        super::super::queue::QueueHost::new(s.db.conn.clone(), 1)?
            .cancel_queued(&p.task_id, p.expected_revision)
            .await
            .map(Json)
    }
}
