//! Thin adapters; both transports use the same registry validation and storage.

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_save_member_snapshot(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    expected_revision: i64,
    expected_check_version: String,
    expected_snapshot_version: String,
) -> Result<super::SnapshotRecord, crate::app_error::AppCommandError> {
    super::hall_save_member_snapshot_core(
        &db.conn,
        member_id,
        expected_revision,
        expected_check_version,
        expected_snapshot_version,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_list_member_snapshots(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
) -> Result<Vec<super::SnapshotRecord>, crate::app_error::AppCommandError> {
    super::hall_list_member_snapshots_core(&db.conn, member_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_preview_member_execution(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    expected_revision: i64,
    expected_check_version: String,
) -> Result<super::MemberExecutionPreview, crate::app_error::AppCommandError> {
    super::hall_preview_member_execution_core(
        &db.conn,
        member_id,
        expected_revision,
        expected_check_version,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_check_member(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    expected_revision: i64,
    expected_check_version: String,
) -> Result<super::MemberCheckReport, crate::app_error::AppCommandError> {
    super::hall_check_member_core(
        &db.conn,
        member_id,
        expected_revision,
        expected_check_version,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_save_member_config(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    expected_revision: i64,
    expected_file_version: String,
    draft: serde_json::Value,
) -> Result<super::MemberConfigPreview, crate::app_error::AppCommandError> {
    super::hall_save_member_config_core(
        &db.conn,
        member_id,
        expected_revision,
        expected_file_version,
        draft,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_preview_member_config_draft(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    expected_revision: i64,
    expected_file_version: String,
    draft: serde_json::Value,
) -> Result<super::MemberConfigDraftPreview, crate::app_error::AppCommandError> {
    super::hall_preview_member_config_draft_core(
        &db.conn,
        member_id,
        expected_revision,
        expected_file_version,
        draft,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_preview_member_config(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    expected_revision: i64,
) -> Result<super::MemberConfigPreview, crate::app_error::AppCommandError> {
    super::hall_preview_member_config_core(&db.conn, member_id, expected_revision).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_list_member_projects(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<Vec<super::MemberProject>, crate::app_error::AppCommandError> {
    super::hall_list_member_projects_core(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_register_member(
    db: tauri::State<'_, crate::db::AppDatabase>,
    draft: super::RegisterMemberDraft,
) -> Result<super::HallMember, crate::app_error::AppCommandError> {
    super::hall_register_member_core(&db.conn, draft).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_list_members(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<Vec<super::HallMember>, crate::app_error::AppCommandError> {
    super::hall_list_members_core(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_update_member(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    expected_revision: i64,
    draft: super::UpdateMemberDraft,
) -> Result<super::HallMember, crate::app_error::AppCommandError> {
    super::hall_update_member_core(&db.conn, member_id, expected_revision, draft).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_set_member_enabled(
    db: tauri::State<'_, crate::db::AppDatabase>,
    member_id: String,
    enabled: bool,
) -> Result<super::HallMember, crate::app_error::AppCommandError> {
    super::hall_set_member_enabled_core(&db.conn, member_id, enabled).await
}

pub mod web {
    use std::sync::Arc;

    use axum::{Extension, Json};
    use serde::Deserialize;

    use super::super::{
        hall_list_member_projects_core, hall_list_members_core, hall_register_member_core,
        hall_set_member_enabled_core, hall_update_member_core, HallMember, MemberProject,
        RegisterMemberDraft, UpdateMemberDraft,
    };
    use crate::app_error::AppCommandError;
    use crate::app_state::AppState;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct RegisterParams {
        draft: RegisterMemberDraft,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct UpdateParams {
        member_id: String,
        expected_revision: i64,
        draft: UpdateMemberDraft,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct EnabledParams {
        member_id: String,
        enabled: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct PreviewParams {
        member_id: String,
        expected_revision: i64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct CheckParams {
        member_id: String,
        expected_revision: i64,
        expected_check_version: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct SaveSnapshotParams {
        member_id: String,
        expected_revision: i64,
        expected_check_version: String,
        expected_snapshot_version: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct ListSnapshotsParams {
        member_id: String,
    }

    pub async fn hall_save_member_snapshot(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<SaveSnapshotParams>,
    ) -> Result<Json<super::super::SnapshotRecord>, AppCommandError> {
        super::super::hall_save_member_snapshot_core(
            &state.db.conn,
            params.member_id,
            params.expected_revision,
            params.expected_check_version,
            params.expected_snapshot_version,
        )
        .await
        .map(Json)
    }
    pub async fn hall_list_member_snapshots(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<ListSnapshotsParams>,
    ) -> Result<Json<Vec<super::super::SnapshotRecord>>, AppCommandError> {
        super::super::hall_list_member_snapshots_core(&state.db.conn, params.member_id)
            .await
            .map(Json)
    }

    pub async fn hall_preview_member_execution(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<CheckParams>,
    ) -> Result<Json<super::super::MemberExecutionPreview>, AppCommandError> {
        super::super::hall_preview_member_execution_core(
            &state.db.conn,
            params.member_id,
            params.expected_revision,
            params.expected_check_version,
        )
        .await
        .map(Json)
    }

    pub async fn hall_check_member(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<CheckParams>,
    ) -> Result<Json<super::super::MemberCheckReport>, AppCommandError> {
        super::super::hall_check_member_core(
            &state.db.conn,
            params.member_id,
            params.expected_revision,
            params.expected_check_version,
        )
        .await
        .map(Json)
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct PreviewDraftParams {
        member_id: String,
        expected_revision: i64,
        expected_file_version: String,
        draft: serde_json::Value,
    }

    pub async fn hall_save_member_config(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<PreviewDraftParams>,
    ) -> Result<Json<super::super::MemberConfigPreview>, AppCommandError> {
        super::super::hall_save_member_config_core(
            &state.db.conn,
            params.member_id,
            params.expected_revision,
            params.expected_file_version,
            params.draft,
        )
        .await
        .map(Json)
    }

    pub async fn hall_preview_member_config_draft(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<PreviewDraftParams>,
    ) -> Result<Json<super::super::MemberConfigDraftPreview>, AppCommandError> {
        super::super::hall_preview_member_config_draft_core(
            &state.db.conn,
            params.member_id,
            params.expected_revision,
            params.expected_file_version,
            params.draft,
        )
        .await
        .map(Json)
    }

    pub async fn hall_preview_member_config(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<PreviewParams>,
    ) -> Result<Json<super::super::MemberConfigPreview>, AppCommandError> {
        super::super::hall_preview_member_config_core(
            &state.db.conn,
            params.member_id,
            params.expected_revision,
        )
        .await
        .map(Json)
    }

    pub async fn hall_register_member(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<RegisterParams>,
    ) -> Result<Json<HallMember>, AppCommandError> {
        hall_register_member_core(&state.db.conn, params.draft)
            .await
            .map(Json)
    }

    pub async fn hall_list_members(
        Extension(state): Extension<Arc<AppState>>,
    ) -> Result<Json<Vec<HallMember>>, AppCommandError> {
        hall_list_members_core(&state.db.conn).await.map(Json)
    }

    pub async fn hall_list_member_projects(
        Extension(state): Extension<Arc<AppState>>,
    ) -> Result<Json<Vec<MemberProject>>, AppCommandError> {
        hall_list_member_projects_core(&state.db.conn)
            .await
            .map(Json)
    }

    pub async fn hall_update_member(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<UpdateParams>,
    ) -> Result<Json<HallMember>, AppCommandError> {
        hall_update_member_core(
            &state.db.conn,
            params.member_id,
            params.expected_revision,
            params.draft,
        )
        .await
        .map(Json)
    }

    pub async fn hall_set_member_enabled(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<EnabledParams>,
    ) -> Result<Json<HallMember>, AppCommandError> {
        hall_set_member_enabled_core(&state.db.conn, params.member_id, params.enabled)
            .await
            .map(Json)
    }
}
