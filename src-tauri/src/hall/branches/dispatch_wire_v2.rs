//! v2 task-exchange endpoints. Metadata is bounded JSON; bytes use separate,
//! resource-scoped streams that never accept a local path.
use super::{dispatch_result, inbox, trust};
use crate::{
    app_error::AppCommandError,
    hall::exchange::{
        ArtifactRef, DispatchOwner, Exchange, ExchangeManifest, MAX_FILE_BYTES, MAX_MANIFEST_BYTES,
        MAX_MANIFEST_ITEMS, MAX_TOTAL_BYTES,
    },
};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Extension, Path},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::TryStreamExt;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::io::{ReaderStream, StreamReader};

pub(super) struct Context {
    pub db: DatabaseConnection,
    pub exchange: Exchange,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvelopeV2 {
    pub dispatch_id: String,
    pub branch_id: String,
    pub member_ref: String,
    pub title: String,
    pub description: String,
    pub manifest: ExchangeManifest,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    protocol_version: u32,
    task_exchange: bool,
    max_file_bytes: u64,
    max_manifest_items: usize,
    max_total_bytes: u64,
    max_manifest_bytes: usize,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReportV2 {
    #[serde(flatten)]
    pub report: dispatch_result::Report,
    pub received_inputs: Vec<String>,
    pub artifacts: Vec<ArtifactRef>,
}

fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Exchange transport is unavailable")
}
fn sql(q: &str, v: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, q, v)
}

/// Routes are mounted behind the same issuer authentication as v1.
pub(super) fn routes() -> Router {
    let bounded = Router::new()
        .route("/hall-dispatch/v2/capabilities", get(capabilities))
        .route("/hall-dispatch/v2/inbox", post(receive))
        .route("/hall-dispatch/v2/inbox/{dispatch_id}", get(detail))
        .layer(DefaultBodyLimit::max(MAX_MANIFEST_BYTES));
    // Streams are limited by the exchange core to the manifest-declared size.
    let streams = Router::new()
        .route(
            "/hall-dispatch/v2/inbox/{dispatch_id}/inputs/{object_id}",
            put(upload),
        )
        .route(
            "/hall-dispatch/v2/inbox/{dispatch_id}/artifacts/{object_id}",
            get(download),
        )
        .layer(DefaultBodyLimit::disable());
    bounded.merge(streams)
}

async fn capabilities() -> Json<Capabilities> {
    Json(Capabilities {
        protocol_version: 2,
        task_exchange: true,
        max_file_bytes: MAX_FILE_BYTES,
        max_manifest_items: MAX_MANIFEST_ITEMS,
        max_total_bytes: MAX_TOTAL_BYTES,
        max_manifest_bytes: MAX_MANIFEST_BYTES,
    })
}

/// The branch id is HQ data bound at first attach; never reused across issuers.
async fn owner(
    db: &DatabaseConnection,
    issuer: &trust::VerifiedIssuer,
    dispatch: &str,
) -> Result<DispatchOwner, AppCommandError> {
    let row = db
        .query_one(sql(
            "SELECT branch_id FROM hall_exchange_attachment WHERE issuer_id=? AND dispatch_id=?",
            vec![issuer.id().into(), dispatch.into()],
        ))
        .await
        .map_err(storage)?
        .ok_or_else(|| AppCommandError::not_found("Exchange dispatch not found"))?;
    let branch: String = row.try_get("", "branch_id").map_err(storage)?;
    Ok(DispatchOwner::authenticated(
        issuer.id(),
        dispatch,
        &branch,
    )?)
}

/// First write takes the SQLite lock, then rechecks live trust.
async fn recheck<C: ConnectionTrait>(
    tx: &C,
    issuer: &trust::VerifiedIssuer,
) -> Result<(), AppCommandError> {
    let ok = tx
        .execute(sql(
            "UPDATE hall_branch_trust SET revision=revision WHERE issuer_id=? AND enabled=1",
            vec![issuer.id().into()],
        ))
        .await
        .map_err(storage)?;
    if ok.rows_affected() != 1 {
        return Err(AppCommandError::permission_denied(
            "Headquarters credentials are invalid",
        ));
    }
    Ok(())
}

async fn receive(
    Extension(ctx): Extension<Arc<Context>>,
    Extension(issuer): Extension<Arc<trust::VerifiedIssuer>>,
    Json(envelope): Json<EnvelopeV2>,
) -> Result<Json<inbox::Receipt>, AppCommandError> {
    let dispatch = uuid::Uuid::parse_str(&envelope.dispatch_id)
        .map_err(|_| AppCommandError::invalid_input("Invalid dispatch identity"))?
        .to_string();
    let branch = uuid::Uuid::parse_str(&envelope.branch_id)
        .map_err(|_| AppCommandError::invalid_input("Invalid branch identity"))?
        .to_string();
    let tx = ctx.db.begin().await.map_err(storage)?;
    recheck(&tx, &issuer).await?;
    // A manifest may only join an item that was never accepted without one.
    let existed = inbox::exists(&tx, issuer.id(), &dispatch).await?;
    let attached = tx
        .query_one(sql(
            "SELECT 1 FROM hall_exchange_attachment WHERE dispatch_id=?",
            vec![dispatch.clone().into()],
        ))
        .await
        .map_err(storage)?
        .is_some();
    if existed && !attached {
        return Err(AppCommandError::already_exists(
            "Dispatch was already accepted without an exchange manifest",
        ));
    }
    let receipt = inbox::store_in(
        &tx,
        issuer.id(),
        inbox::Envelope {
            dispatch_id: dispatch.clone(),
            member_ref: envelope.member_ref,
            title: envelope.title,
            description: envelope.description,
        },
        Some(&envelope.manifest.digest),
    )
    .await?;
    let owner = DispatchOwner::authenticated(issuer.id(), &dispatch, &branch)?;
    ctx.exchange
        .accept_dispatch_in(&tx, &owner, &dispatch, &envelope.manifest)
        .await?;
    tx.commit().await.map_err(storage)?;
    Ok(Json(receipt))
}

async fn detail(
    Extension(ctx): Extension<Arc<Context>>,
    Extension(issuer): Extension<Arc<trust::VerifiedIssuer>>,
    Path(dispatch): Path<String>,
) -> Result<Json<ReportV2>, AppCommandError> {
    let report = dispatch_result::detail(&ctx.db, &issuer, &dispatch).await?;
    let owner = owner(&ctx.db, &issuer, &dispatch).await?;
    let rows = ctx
        .db
        .query_all(sql(
            "SELECT object_id FROM hall_exchange_input WHERE submission_id=? AND confirmed=1 ORDER BY object_id",
            vec![dispatch.clone().into()],
        ))
        .await
        .map_err(storage)?;
    let received_inputs = rows
        .iter()
        .map(|r| r.try_get("", "object_id").map_err(storage))
        .collect::<Result<Vec<String>, _>>()?;
    let artifacts = ctx.exchange.sealed_artifacts(&owner).await?;
    Ok(Json(ReportV2 {
        report,
        received_inputs,
        artifacts,
    }))
}

async fn upload(
    Extension(ctx): Extension<Arc<Context>>,
    Extension(issuer): Extension<Arc<trust::VerifiedIssuer>>,
    Path((dispatch, object)): Path<(String, String)>,
    body: Body,
) -> Result<StatusCode, AppCommandError> {
    let owner = owner(&ctx.db, &issuer, &dispatch).await?;
    let permit = ctx.exchange.prepare_dispatch_input(&owner, &object).await?;
    let reader = StreamReader::new(body.into_data_stream().map_err(std::io::Error::other));
    let received = ctx.exchange.receive_dispatch_input(permit, reader).await?;
    let tx = ctx.db.begin().await.map_err(storage)?;
    recheck(&tx, &issuer).await?;
    ctx.exchange.commit_dispatch_input_in(&tx, received).await?;
    tx.commit().await.map_err(storage)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn download(
    Extension(ctx): Extension<Arc<Context>>,
    Extension(issuer): Extension<Arc<trust::VerifiedIssuer>>,
    Path((dispatch, object)): Path<(String, String)>,
) -> Result<Response, AppCommandError> {
    recheck(&ctx.db, &issuer).await?;
    let owner = owner(&ctx.db, &issuer, &dispatch).await?;
    let file = ctx.exchange.open_artifact(&owner, &object).await?;
    let size = file.metadata().map_err(storage)?.len();
    let body = Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(file)));
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, size.into());
    Ok(response)
}
