use super::control::Controller;
use crate::app_error::AppCommandError;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    State {},
    NativeProbe {},
    Dispatch {
        task_id: String,
        expected_revision: i64,
    },
    Refresh {
        task_id: String,
    },
    Cancel {
        task_id: String,
        expected_revision: i64,
    },
    Recover {
        task_id: String,
    },
    ResolveBestEffort {
        task_id: String,
        expected_revision: i64,
        acknowledge_unverified_descendants: bool,
    },
    ResolveOrphan {
        task_id: String,
        attempt_id: String,
        expected_revision: i64,
        acknowledge_unverified_processes: bool,
        reason: String,
    },
    Session {
        task_id: String,
    },
}
async fn call(c: &Controller, request: Request) -> Result<Value, AppCommandError> {
    fn encode<T: serde::Serialize>(x: T) -> Result<Value, AppCommandError> {
        serde_json::to_value(x)
            .map_err(|_| AppCommandError::io_error("Team control result unavailable"))
    }
    match request {
        Request::State {} => encode(c.readiness().await),
        Request::NativeProbe {} => encode(c.probe().await?),
        Request::Dispatch {
            task_id,
            expected_revision,
        } => encode(c.dispatch(task_id, expected_revision).await?),
        Request::Refresh { task_id } => encode(c.refresh(task_id).await?),
        Request::Cancel {
            task_id,
            expected_revision,
        } => encode(c.cancel(task_id, expected_revision).await?),
        Request::Recover { task_id } => encode(c.recover(task_id).await?),
        Request::ResolveBestEffort {
            task_id,
            expected_revision,
            acknowledge_unverified_descendants,
        } => encode(
            c.resolve_best_effort(
                task_id,
                expected_revision,
                acknowledge_unverified_descendants,
            )
            .await?,
        ),
        Request::ResolveOrphan {
            task_id,
            attempt_id,
            expected_revision,
            acknowledge_unverified_processes,
            reason,
        } => encode(
            c.resolve_orphan(
                task_id,
                attempt_id,
                expected_revision,
                acknowledge_unverified_processes,
                reason,
            )
            .await?,
        ),
        Request::Session { task_id } => encode(c.session(task_id).await?),
    }
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_team_control(
    window: tauri::WebviewWindow,
    control: tauri::State<'_, Arc<Controller>>,
    request: Request,
) -> Result<Value, AppCommandError> {
    if local_only(&request) {
        let denied = || {
            AppCommandError::permission_denied("Team resolution requires a local desktop window")
        };
        let url = window.url().map_err(|_| denied())?;
        crate::update::desktop_auth::validate_window(window.label(), &url).map_err(|_| denied())?;
    }
    call(&control, request).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    pub request: Request,
}
fn local_only(request: &Request) -> bool {
    matches!(
        request,
        Request::ResolveBestEffort { .. } | Request::ResolveOrphan { .. }
    )
}
fn local_peer(peer: Option<std::net::SocketAddr>, headers: &axum::http::HeaderMap) -> bool {
    // Authentication is the existing administrator middleware. Forwarded/Host
    // headers never grant local authority. Native local clients send no Origin.
    peer.is_some_and(|p| p.ip().is_loopback())
        && !headers.contains_key(axum::http::header::ORIGIN)
        && !headers.contains_key("sec-fetch-site")
        && !headers.contains_key("forwarded")
        && !headers.contains_key("x-forwarded-for")
        && !headers.contains_key("x-real-ip")
}
pub async fn web(
    axum::Extension(state): axum::Extension<Arc<crate::app_state::AppState>>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: axum::http::HeaderMap,
    axum::Json(p): axum::Json<Params>,
) -> Result<axum::Json<Value>, AppCommandError> {
    if local_only(&p.request) && !local_peer(peer.map(|p| p.0 .0), &headers) {
        return Err(AppCommandError::permission_denied(
            "Team resolution requires an authenticated native loopback client",
        ));
    }
    call(&state.hall_control, p.request).await.map(axum::Json)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolution_requires_actual_loopback_peer_and_rejects_browser_origins() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "localhost".parse().unwrap());
        h.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
        assert!(!local_peer(None, &h));
        assert!(!local_peer(Some("192.0.2.1:1234".parse().unwrap()), &h));
        assert!(!local_peer(Some("127.0.0.1:1234".parse().unwrap()), &h));
        h.remove("x-forwarded-for");
        assert!(local_peer(Some("127.0.0.1:1234".parse().unwrap()), &h));
        assert!(local_peer(Some("[::1]:1234".parse().unwrap()), &h));
        h.insert("origin", "https://evil.invalid".parse().unwrap());
        assert!(!local_peer(Some("127.0.0.1:1234".parse().unwrap()), &h));
    }
}
