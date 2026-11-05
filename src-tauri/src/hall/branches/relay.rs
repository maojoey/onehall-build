//! Each request captures an immutable branch target. There is no global selection.
use super::{relay_error::RelayError, tickets::TicketStore};
use crate::web::auth::{self, WebAuth};
use axum::{
    extract::Extension,
    middleware,
    routing::{any, get, post},
    Router,
};
use sea_orm::DatabaseConnection;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub(super) struct Limits {
    pub request_bytes: u64,
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub http_timeout: Duration,
    pub transfer_timeout: Duration,
    pub http_concurrency: usize,
    pub ws_concurrency: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            request_bytes: 32 * 1024 * 1024,
            upload_bytes: 512 * 1024 * 1024,
            download_bytes: 2 * 1024 * 1024 * 1024,
            http_timeout: Duration::from_secs(90),
            transfer_timeout: Duration::from_secs(300),
            http_concurrency: 32,
            ws_concurrency: 16,
        }
    }
}
pub(super) struct Runtime {
    pub db: DatabaseConnection,
    pub http: Arc<Semaphore>,
    pub ws: Arc<Semaphore>,
    pub tickets: TicketStore,
    pub limits: Limits,
}
pub fn routes(db: DatabaseConnection, auth: WebAuth) -> Router {
    routes_with_limits(db, auth, Limits::default())
}
pub(super) fn routes_with_limits(db: DatabaseConnection, auth: WebAuth, limits: Limits) -> Router {
    let runtime = Arc::new(Runtime {
        db,
        http: Arc::new(Semaphore::new(limits.http_concurrency)),
        ws: Arc::new(Semaphore::new(limits.ws_concurrency)),
        tickets: TicketStore::new(Duration::from_secs(90), 256),
        limits,
    });
    let protected = Router::new()
        .route(
            "/branches/{branchId}/api/{command}",
            post(super::relay_http::command),
        )
        .route(
            "/branches/{branchId}/ws/events",
            get(super::relay_ws::events),
        )
        .route("/branches/{branchId}", any(denied))
        .route("/branches/{branchId}/{*rest}", any(denied))
        .layer(middleware::from_fn(move |request, next| {
            auth::require_auth(request, next, auth.clone())
        }));
    protected
        .merge(
            Router::new()
                .route(
                    "/branch-download/{ticket}",
                    get(super::relay_http::download),
                )
                .route(
                    "/branches/{branchId}/branch-download/{ticket}",
                    get(super::relay_http::branch_download),
                ),
        )
        // Bodies are streamed and bounded in relay_http, including unknown lengths.
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(Extension(runtime))
}
async fn denied() -> RelayError {
    RelayError::new(
        axum::http::StatusCode::FORBIDDEN,
        "branch_route_not_permitted",
        "This branch resource is unavailable; HTML previews require an isolated origin",
    )
}
