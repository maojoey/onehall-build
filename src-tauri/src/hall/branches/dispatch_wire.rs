//! Dedicated issuer-scoped intake, separate from workspace authentication.
//! Only delivery and status reads are exposed; confirmation remains local.
use super::{dispatch_wire_v2 as v2, inbox, trust};
use crate::hall::exchange::Exchange;
use axum::{
    extract::{DefaultBodyLimit, Extension, Path, Request},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tokio::sync::Semaphore;

struct Runtime {
    db: DatabaseConnection,
    requests: Arc<Semaphore>,
}

/// `exchange: None` serves only the v1 text protocol; v2 paths are then absent.
pub fn routes(db: DatabaseConnection, exchange: Option<Exchange>) -> Router {
    let v2_routes = exchange.map(|exchange| {
        v2::routes().layer(Extension(Arc::new(v2::Context {
            db: db.clone(),
            exchange,
        })))
    });
    let mut router = Router::new()
        .route("/hall-dispatch/v1/inbox", post(receive))
        .route("/hall-dispatch/v1/inbox/{dispatch_id}", get(detail))
        .layer(DefaultBodyLimit::max(64 * 1024));
    if let Some(v2_routes) = v2_routes {
        router = router.merge(v2_routes);
    }
    router
        .layer(middleware::from_fn(authorize))
        .layer(Extension(Arc::new(Runtime {
            db,
            requests: Arc::new(Semaphore::new(8)),
        })))
}

async fn authorize(
    Extension(runtime): Extension<Arc<Runtime>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Ok(_permit) = runtime.requests.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    if request.headers().get_all(AUTHORIZATION).iter().count() != 1 {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let credentials = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() < 128)
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|v| v.split_once('.'));
    let Some((issuer, secret)) = credentials else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Ok(verified) = trust::authenticate(&runtime.db, issuer, secret).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    request.extensions_mut().insert(Arc::new(verified));
    next.run(request).await
}

async fn receive(
    Extension(runtime): Extension<Arc<Runtime>>,
    Extension(issuer): Extension<Arc<trust::VerifiedIssuer>>,
    Json(envelope): Json<inbox::Envelope>,
) -> Result<Json<inbox::Receipt>, crate::app_error::AppCommandError> {
    inbox::receive_verified(&runtime.db, &issuer, envelope)
        .await
        .map(Json)
}

async fn detail(
    Extension(runtime): Extension<Arc<Runtime>>,
    Extension(issuer): Extension<Arc<trust::VerifiedIssuer>>,
    Path(dispatch_id): Path<String>,
) -> Result<Json<super::dispatch_result::Report>, crate::app_error::AppCommandError> {
    super::dispatch_result::detail(&runtime.db, &issuer, &dispatch_id)
        .await
        .map(Json)
}
