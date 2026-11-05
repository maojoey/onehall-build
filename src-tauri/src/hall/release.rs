//! OneHall release policy. Upstream alignment is development metadata, never
//! an application update source. Server self-updates remain manual; desktop
//! signed updates use the separate OneHall desktop updater policy.
use crate::app_error::AppCommandError;

pub const AUTOMATIC_UPDATES_ENABLED: bool = false;

pub fn require_managed_updates() -> Result<(), AppCommandError> {
    Err(AppCommandError::permission_denied(
        "OneHall uses manual updates; install a OneHall build",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onehall_version_and_upstream_alignment_are_separate() {
        let policy: serde_json::Value =
            serde_json::from_str(include_str!("../../../onehall.release.json")).unwrap();
        let package: serde_json::Value =
            serde_json::from_str(include_str!("../../../package.json")).unwrap();
        let tauri: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let desktop: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.hall.conf.json")).unwrap();
        assert_eq!(policy["product"], "onehall");
        assert_eq!(policy["updateMode"], "desktop-signed");
        assert_eq!(desktop["plugins"]["updater"]["requireSignedVersion"], true);
        assert_eq!(
            desktop["plugins"]["updater"]["endpoints"][0],
            "https://updates.misq.in/latest.json"
        );
        assert_eq!(policy["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(policy["version"], package["version"]);
        assert_eq!(policy["version"], tauri["version"]);
        assert_eq!(policy["upstream"]["version"], "0.40.0");
        assert_eq!(
            policy["upstream"]["commit"],
            "95fd81df4374753c460b5cb0ed01de8f1ea24702"
        );
        assert_eq!(
            tauri["plugins"]["updater"]["endpoints"][0],
            crate::update::version::UPDATE_MANIFEST_URL
        );
        assert!(crate::update::version::UPDATE_MANIFEST_URL
            .starts_with("https://github.com/maojoey/onehall/"));
        assert!(crate::update::version::RELEASE_DOWNLOAD_BASE
            .starts_with("https://github.com/maojoey/onehall/"));
        assert_eq!(tauri["bundle"]["createUpdaterArtifacts"], false);
        assert!(require_managed_updates().is_err());
    }

    #[tokio::test]
    async fn update_entrypoints_reject_before_locks_downloads_or_restart() {
        use crate::update::state::AppUpdateLifecycle;
        use crate::web::handlers::{app_update, web_server};
        use axum::extract::Extension;
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let state = std::sync::Arc::new(crate::app_state::AppState::new_for_test(
            db,
            dir.path().to_owned(),
        ));
        let _guard = state.system_op_lock.clone().lock_owned().await;
        let errors = [
            app_update::perform_app_update(Extension(state.clone()))
                .await
                .unwrap_err(),
            app_update::restart_app(Extension(state.clone()))
                .await
                .unwrap_err(),
            app_update::rollback_app(Extension(state.clone()))
                .await
                .unwrap_err(),
        ];
        for error in errors {
            assert_eq!(
                serde_json::to_value(error).unwrap()["code"],
                "permission_denied"
            );
        }
        assert_eq!(
            crate::update::state::snapshot(&state.update_state).status,
            AppUpdateLifecycle::Idle
        );
        let status = serde_json::to_value(web_server::app_update_status().await.0).unwrap();
        assert_eq!(status["selfUpdateSupported"], false);
        assert_eq!(status["rollbackAvailable"], false);
        assert!(web_server::check_app_update().await.is_err());
        assert!(crate::update::version::fetch_latest_manifest()
            .await
            .is_err());
    }
}
