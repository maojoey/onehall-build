use super::{
    model::{HallBranchSummary, RegisterBranchInput},
    service,
};
use crate::app_error::AppCommandError;
use crate::hall::exchange::Exchange;
use axum::{extract::Extension, routing::post, Json, Router};
use sea_orm::DatabaseConnection;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisterRequest {
    input: RegisterBranchInput,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BranchRequest {
    branch_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnabledRequest {
    branch_id: String,
    enabled: bool,
    expected_revision: i64,
}

/// Mounted only inside the existing authenticated API router.
pub fn routes(db: DatabaseConnection, exchange: Option<Exchange>) -> Router {
    Router::new()
        .route("/hall_list_branches", post(list))
        .route("/hall_dispatch_control", post(dispatch_control))
        .route("/hall_register_branch", post(register))
        .route("/hall_set_branch_enabled", post(set_enabled))
        .route("/hall_probe_branch", post(probe))
        .layer(Extension(db))
        .layer(Extension(exchange))
}
async fn list(
    Extension(db): Extension<DatabaseConnection>,
) -> Result<Json<Vec<HallBranchSummary>>, AppCommandError> {
    service::list(&db).await.map(Json)
}
async fn register(
    Extension(db): Extension<DatabaseConnection>,
    Json(p): Json<RegisterRequest>,
) -> Result<Json<HallBranchSummary>, AppCommandError> {
    service::register(&db, p.input).await.map(Json)
}
async fn set_enabled(
    Extension(db): Extension<DatabaseConnection>,
    Json(p): Json<EnabledRequest>,
) -> Result<Json<HallBranchSummary>, AppCommandError> {
    service::set_enabled(&db, &p.branch_id, p.enabled, p.expected_revision)
        .await
        .map(Json)
}
async fn probe(
    Extension(db): Extension<DatabaseConnection>,
    Json(p): Json<BranchRequest>,
) -> Result<Json<HallBranchSummary>, AppCommandError> {
    service::probe(&db, &p.branch_id).await.map(Json)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_list_branches(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<Vec<HallBranchSummary>, AppCommandError> {
    service::list(&db.conn).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_register_branch(
    db: tauri::State<'_, crate::db::AppDatabase>,
    input: RegisterBranchInput,
) -> Result<HallBranchSummary, AppCommandError> {
    service::register(&db.conn, input).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_set_branch_enabled(
    db: tauri::State<'_, crate::db::AppDatabase>,
    branch_id: String,
    enabled: bool,
    expected_revision: i64,
) -> Result<HallBranchSummary, AppCommandError> {
    service::set_enabled(&db.conn, &branch_id, enabled, expected_revision).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_probe_branch(
    db: tauri::State<'_, crate::db::AppDatabase>,
    branch_id: String,
) -> Result<HallBranchSummary, AppCommandError> {
    service::probe(&db.conn, &branch_id).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_connect_branch(
    db: tauri::State<'_, crate::db::AppDatabase>,
    headquarters_connection_id: i32,
    branch_id: String,
) -> Result<super::model::BranchConnection, AppCommandError> {
    service::connect_branch(&db.conn, headquarters_connection_id, &branch_id).await
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DispatchRequest {
    List {},
    Create { draft: super::dispatch::Draft },
    Sync { dispatch_id: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchPayload {
    request: DispatchRequest,
}
async fn control(
    db: &DatabaseConnection,
    exchange: Option<&Exchange>,
    request: DispatchRequest,
) -> Result<serde_json::Value, AppCommandError> {
    let value = match request {
        DispatchRequest::List {} => serde_json::to_value(super::dispatch::list(db).await?),
        DispatchRequest::Create { draft } => {
            serde_json::to_value(super::dispatch::create_with_exchange(db, exchange, draft).await?)
        }
        DispatchRequest::Sync { dispatch_id } => {
            serde_json::to_value(super::delivery::sync_with(db, exchange, &dispatch_id).await?)
        }
    };
    value.map_err(|_| AppCommandError::io_error("Dispatch response is unavailable"))
}
async fn dispatch_control(
    Extension(db): Extension<DatabaseConnection>,
    Extension(exchange): Extension<Option<Exchange>>,
    Json(payload): Json<DispatchPayload>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    control(&db, exchange.as_ref(), payload.request)
        .await
        .map(Json)
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_dispatch_control(
    db: tauri::State<'_, crate::db::AppDatabase>,
    exchange: tauri::State<'_, crate::hall::exchange::service::HostExchange>,
    request: DispatchRequest,
) -> Result<serde_json::Value, AppCommandError> {
    control(&db.conn, exchange.0.as_ref(), request).await
}
