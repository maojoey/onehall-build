//! onehall: the hall module — OneHall's own code lives here, not in upstream
//! files (see `docs/onehall/CUSTOMIZATIONS.md`). For now: the single-window
//! workspace setting. The team / PM pieces (`docs/onehall/team-pm-design.md`)
//! land here too.

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;

pub(crate) mod codex_cancel;
pub(crate) mod engine;
pub mod members;
pub mod release;
pub(crate) mod schema_migration;
#[cfg(test)]
mod upstream_040_tests;

#[cfg(test)]
mod members_tests;

#[cfg(test)]
mod members_config_save_tests;
#[cfg(test)]
mod members_config_tests;
#[cfg(test)]
mod members_execution_tests;
#[cfg(test)]
mod members_execution_mode_tests;
#[cfg(test)]
mod members_preflight_tests;
#[cfg(test)]
mod members_snapshot_tests;

#[cfg(any(feature = "tauri-runtime", test))]
pub mod pet_chat;

#[cfg(any(feature = "tauri-runtime", test))]
pub mod pet_panel;

/// `app_metadata` key holding [`HallSettings`] as JSON.
pub const HALL_SETTINGS_KEY: &str = "hall.settings";

/// Desktop-shell settings for OneHall.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HallSettings {
    /// One main window with a bottom workspace tab bar (local + each remote
    /// connection as a tab) instead of upstream's one-window-per-backend.
    /// Read by the restore plan (no remote windows are reopened), by
    /// `open_remote_workspace` (selects the tab instead of opening a window)
    /// and by `focus_conversation` (routes to the main window). Default on:
    /// this is OneHall's product shape; switching it off restores upstream
    /// behaviour exactly.
    #[serde(default = "default_true")]
    pub single_window: bool,
}

fn default_true() -> bool {
    true
}

impl Default for HallSettings {
    fn default() -> Self {
        Self {
            single_window: true,
        }
    }
}

pub async fn load_settings(db: &DatabaseConnection) -> HallSettings {
    match app_metadata_service::get_value(db, HALL_SETTINGS_KEY).await {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
        Ok(None) => HallSettings::default(),
        Err(err) => {
            tracing::warn!("[hall] reading hall settings failed, using defaults: {err}");
            HallSettings::default()
        }
    }
}

/// Convenience for the window-management call sites: the setting with a
/// defaulted fallback, never an error.
pub async fn single_window(db: &DatabaseConnection) -> bool {
    load_settings(db).await.single_window
}

pub async fn hall_get_settings_core(
    db: &DatabaseConnection,
) -> Result<HallSettings, AppCommandError> {
    Ok(load_settings(db).await)
}

pub async fn hall_set_settings_core(
    db: &DatabaseConnection,
    settings: HallSettings,
) -> Result<HallSettings, AppCommandError> {
    let json = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::io_error(format!("Failed to serialize hall settings: {e}"))
    })?;
    app_metadata_service::upsert_value(db, HALL_SETTINGS_KEY, &json)
        .await
        .map_err(AppCommandError::db)?;
    Ok(settings)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn hall_get_settings(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<HallSettings, AppCommandError> {
    hall_get_settings_core(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn hall_set_settings(
    db: tauri::State<'_, crate::db::AppDatabase>,
    settings: HallSettings,
) -> Result<HallSettings, AppCommandError> {
    hall_set_settings_core(&db.conn, settings).await
}

pub mod web {
    use std::sync::Arc;

    use axum::{Extension, Json};

    use super::{hall_get_settings_core, hall_set_settings_core, HallSettings};
    use crate::app_error::AppCommandError;
    use crate::app_state::AppState;

    pub async fn hall_get_settings(
        Extension(state): Extension<Arc<AppState>>,
    ) -> Result<Json<HallSettings>, AppCommandError> {
        hall_get_settings_core(&state.db.conn).await.map(Json)
    }

    pub async fn hall_set_settings(
        Extension(state): Extension<Arc<AppState>>,
        Json(settings): Json<HallSettings>,
    ) -> Result<Json<HallSettings>, AppCommandError> {
        hall_set_settings_core(&state.db.conn, settings)
            .await
            .map(Json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_single_window_and_tolerate_missing_fields() {
        let parsed: HallSettings = serde_json::from_str("{}").unwrap();
        assert!(parsed.single_window);
        assert_eq!(parsed, HallSettings::default());
        let off: HallSettings = serde_json::from_str(r#"{"singleWindow":false}"#).unwrap();
        assert!(!off.single_window);
    }
}

pub mod tasks;
#[cfg(test)]
mod tasks_tests;

pub mod team;

#[cfg(test)]
mod original_040_restore_tests;

pub mod branches;

pub mod exchange;
