use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::db::service::quick_message_service;
use crate::models::QuickMessageInfo;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateQuickMessageParams {
    pub title: String,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateQuickMessageParams {
    pub id: i32,
    pub title: Option<String>,
    pub content: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteQuickMessageParams {
    pub id: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderQuickMessagesParams {
    pub ids: Vec<i32>,
}

pub async fn quick_messages_list(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<QuickMessageInfo>>, AppCommandError> {
    let result = quick_message_service::list(&state.db.conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn quick_messages_create(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateQuickMessageParams>,
) -> Result<Json<QuickMessageInfo>, AppCommandError> {
    let result = quick_message_service::create(&state.db.conn, &params.title, &params.content)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn quick_messages_update(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateQuickMessageParams>,
) -> Result<Json<QuickMessageInfo>, AppCommandError> {
    let result =
        quick_message_service::update(&state.db.conn, params.id, params.title, params.content)
            .await
            .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn quick_messages_delete(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeleteQuickMessageParams>,
) -> Result<Json<()>, AppCommandError> {
    quick_message_service::delete(&state.db.conn, params.id)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}

pub async fn quick_messages_reorder(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ReorderQuickMessagesParams>,
) -> Result<Json<()>, AppCommandError> {
    quick_message_service::reorder(&state.db.conn, params.ids)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}
