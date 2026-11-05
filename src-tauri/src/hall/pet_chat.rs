//! OneHall's desktop-wide pet chat shortcut. No key-watching permissions:
//! reuse the computer stop key's ordinary-key validation and OS registration.

use crate::app_error::AppCommandError;
use crate::computer::keys::Platform;
use crate::computer::stop_shortcut::StopShortcut;
use crate::db::service::app_metadata_service;
use sea_orm::DatabaseConnection;

const SETTING: &str = "hall.pet_chat_shortcut";
pub const DEFAULT_SHORTCUT: &str = "Control+Shift+KeyH";

fn checked(value: &str) -> Result<String, AppCommandError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    StopShortcut::parse(value, Platform::current())
        .map(|shortcut| shortcut.to_string())
        .map_err(|err| AppCommandError::configuration_invalid(err.to_string()))
}

async fn saved(db: &DatabaseConnection) -> Result<String, AppCommandError> {
    app_metadata_service::get_value(db, SETTING)
        .await
        .map(|value| value.unwrap_or_else(|| DEFAULT_SHORTCUT.to_string()))
        .map_err(AppCommandError::db)
}

#[cfg(feature = "tauri-runtime")]
mod desktop {
    use super::*;
    use crate::computer::stop_key::StopKey;
    use crate::computer::stop_shortcut::StopKeyStatus;
    use crate::db::AppDatabase;
    use serde::Serialize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tauri::{AppHandle, Manager, State};
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    #[derive(Default)]
    pub struct Runtime {
        gate: tokio::sync::Mutex<()>,
        key: StopKey,
        opening: Arc<AtomicBool>,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Settings {
        shortcut: String,
        status: StopKeyStatus,
    }

    fn apply(app: &AppHandle, runtime: &Runtime, spelling: &str) -> StopKeyStatus {
        let wanted = StopShortcut::parse(spelling, Platform::current()).ok();
        let handle = app.clone();
        let opening = runtime.opening.clone();
        runtime.key.sync(app, wanted.as_ref(), move || {
            if opening.swap(true, Ordering::AcqRel) {
                return;
            }
            let handle = handle.clone();
            let opening = opening.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = crate::commands::windows::show_pet_chat(handle).await {
                    tracing::warn!("[hall] opening pet chat failed: {err}");
                }
                opening.store(false, Ordering::Release);
            });
        });
        runtime.key.status()
    }

    /// Registration waits for the main thread: start asynchronously, never
    /// block the Tauri setup callback waiting for OS shortcut registration.
    pub fn init(app: &AppHandle) {
        app.manage(Arc::new(Runtime::default()));
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let runtime = handle.state::<Arc<Runtime>>();
            let _guard = runtime.gate.lock().await;
            let db = handle.state::<AppDatabase>();
            match saved(&db.conn).await.and_then(|value| checked(&value)) {
                Ok(value) => {
                    apply(&handle, &runtime, &value);
                }
                Err(err) => tracing::warn!("[hall] loading pet chat shortcut failed: {err}"),
            }
        });
    }

    #[tauri::command]
    pub async fn hall_get_pet_chat_settings(
        db: State<'_, AppDatabase>,
        runtime: State<'_, Arc<Runtime>>,
    ) -> Result<Settings, AppCommandError> {
        let _guard = runtime.gate.lock().await;
        Ok(Settings {
            shortcut: saved(&db.conn).await?,
            status: runtime.key.status(),
        })
    }

    #[tauri::command]
    pub async fn hall_set_pet_chat_shortcut(
        app: AppHandle,
        db: State<'_, AppDatabase>,
        runtime: State<'_, Arc<Runtime>>,
        shortcut: String,
    ) -> Result<Settings, AppCommandError> {
        let shortcut = checked(&shortcut)?;
        let _guard = runtime.gate.lock().await;
        let previous = saved(&db.conn).await?;
        if !shortcut.is_empty() && runtime.key.status().active.as_ref() != Some(&shortcut) {
            if let Some(key) = StopShortcut::parse(&shortcut, Platform::current())
                .ok()
                .and_then(|key| key.hotkey())
            {
                if app.global_shortcut().is_registered(key) {
                    return Err(AppCommandError::configuration_invalid(
                        "Shortcut is already registered",
                    ));
                }
            }
        }
        let status = apply(&app, &runtime, &shortcut);
        if let Some(detail) = status.detail {
            // A conflicting key must not silently discard the old working key.
            apply(&app, &runtime, &previous);
            return Err(AppCommandError::configuration_invalid(detail));
        }
        if let Err(err) = app_metadata_service::upsert_value(&db.conn, SETTING, &shortcut).await {
            apply(&app, &runtime, &previous);
            return Err(AppCommandError::db(err));
        }
        Ok(Settings {
            shortcut,
            status: runtime.key.status(),
        })
    }
}

#[cfg(feature = "tauri-runtime")]
pub use desktop::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_validation_preserves_disable_and_rejects_key_watching() {
        assert_eq!(checked("  ").unwrap(), "");
        assert_eq!(checked(DEFAULT_SHORTCUT).unwrap(), DEFAULT_SHORTCUT);
        assert!(checked("Control+Shift+MediaPlayPause").is_err());
        assert!(checked("KeyH").is_err());
        assert!(checked("Shift+KeyH").is_err());
        assert!(checked("Control+Control+KeyH").is_err());
    }

    #[tokio::test]
    async fn missing_setting_defaults_but_explicit_disable_survives_reload() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        assert_eq!(saved(&db.conn).await.unwrap(), DEFAULT_SHORTCUT);
        app_metadata_service::upsert_value(&db.conn, SETTING, "")
            .await
            .unwrap();
        assert_eq!(saved(&db.conn).await.unwrap(), "");
    }
}
