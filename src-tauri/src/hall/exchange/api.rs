//! Operator-facing `hall_exchange` command and byte routes. Authority comes from
//! the surrounding authenticated router/window, never from the request body.
use super::{service::administrator, ArtifactRef, Exchange, ExchangeManifest, ObjectRef};
use crate::app_error::AppCommandError;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Extension, Path},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::TryStreamExt;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde::{Deserialize, Serialize};
use tokio_util::io::{ReaderStream, StreamReader};

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Request {
    PrepareInput {
        submission_id: String,
        display_name: String,
        size: u64,
        sha256: String,
    },
    ConfirmInput {
        submission_id: String,
        object_id: String,
    },
    PrepareManifest {
        submission_id: String,
        input_object_ids: Vec<String>,
        output_names: Vec<String>,
    },
    Artifacts {
        dispatch_id: String,
    },
}
#[derive(Serialize)]
#[serde(untagged)]
pub enum Reply {
    Ticket {
        #[serde(rename = "objectId")]
        object_id: String,
        #[serde(rename = "transferId")]
        transfer_id: String,
    },
    Object(ObjectRef),
    Manifest(ExchangeManifest),
    Artifacts(Vec<ArtifactRef>),
}

fn closed() -> AppCommandError {
    AppCommandError::configuration_missing("File exchange is unavailable on this host")
}
pub async fn execute(
    exchange: Option<&Exchange>,
    request: Request,
) -> Result<serde_json::Value, AppCommandError> {
    let exchange = exchange.ok_or_else(closed)?;
    let admin = administrator();
    let response = match request {
        Request::PrepareInput {
            submission_id,
            display_name,
            size,
            sha256,
        } => {
            let t = exchange
                .prepare_input(&admin, &submission_id, &display_name, size, &sha256)
                .await?;
            Reply::Ticket {
                object_id: t.object_id,
                transfer_id: t.transfer_id,
            }
        }
        Request::ConfirmInput {
            submission_id,
            object_id,
        } => Reply::Object(
            exchange
                .confirm_input(&admin, &submission_id, &object_id)
                .await?,
        ),
        Request::PrepareManifest {
            submission_id,
            input_object_ids,
            output_names,
        } => Reply::Manifest(
            exchange
                .prepare_manifest(&admin, &submission_id, &input_object_ids, &output_names)
                .await?,
        ),
        Request::Artifacts { dispatch_id } => {
            Reply::Artifacts(exchange.collected_artifacts(&admin, &dispatch_id).await?)
        }
    };
    serde_json::to_value(response)
        .map_err(|_| AppCommandError::io_error("Exchange response failed"))
}

#[derive(Clone)]
struct State {
    db: DatabaseConnection,
    exchange: Option<Exchange>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    request: Request,
}
/// Mounted only inside the already-authenticated `/api` router.
pub fn routes(db: DatabaseConnection, exchange: Option<Exchange>) -> Router {
    Router::new()
        .route("/hall_exchange", post(command))
        .route(
            "/hall-exchange/transfers/{transfer_id}",
            put(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/hall-exchange/artifacts/{object_id}", get(download))
        .layer(Extension(State { db, exchange }))
}
async fn command(
    Extension(state): Extension<State>,
    Json(payload): Json<Payload>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    execute(state.exchange.as_ref(), payload.request)
        .await
        .map(Json)
}
async fn upload(
    Extension(state): Extension<State>,
    Path(transfer): Path<String>,
    body: Body,
) -> Result<Json<ObjectRef>, AppCommandError> {
    let exchange = state.exchange.as_ref().ok_or_else(closed)?;
    // The transfer id names a resource; ownership is still checked by `upload`.
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT submission_id FROM hall_exchange_input WHERE transfer_id=?",
            vec![transfer.clone().into()],
        ))
        .await
        .map_err(|_| AppCommandError::io_error("Exchange storage is unavailable"))?
        .ok_or_else(|| AppCommandError::permission_denied("Exchange object is not available"))?;
    let submission: String = row
        .try_get("", "submission_id")
        .map_err(|_| AppCommandError::io_error("Exchange storage is unavailable"))?;
    let reader = StreamReader::new(body.into_data_stream().map_err(std::io::Error::other));
    Ok(Json(
        exchange
            .upload(&administrator(), &submission, &transfer, reader)
            .await?,
    ))
}
async fn download(
    Extension(state): Extension<State>,
    Path(object): Path<String>,
) -> Result<Response, AppCommandError> {
    let exchange = state.exchange.as_ref().ok_or_else(closed)?;
    let file = exchange.open_collected(&administrator(), &object).await?;
    let size = file
        .metadata()
        .map_err(|_| AppCommandError::io_error("Exchange storage is unavailable"))?
        .len();
    let mut response =
        Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(file))).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, size.into());
    Ok(response)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_exchange(
    exchange: tauri::State<'_, super::service::HostExchange>,
    request: Request,
) -> Result<serde_json::Value, AppCommandError> {
    execute(exchange.0.as_ref(), request).await
}

/// Desktop byte adapter: the operator's own window names a local file to send.
/// The file is opened through the core's anchored walk (links, hard links and
/// devices refused); the core still verifies size/hash and ownership, and the
/// path never becomes model-visible.
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_exchange_upload_file(
    window: tauri::WebviewWindow,
    exchange: tauri::State<'_, super::service::HostExchange>,
    db: tauri::State<'_, crate::db::AppDatabase>,
    transfer_id: String,
    path: std::path::PathBuf,
) -> Result<ObjectRef, AppCommandError> {
    super::desktop_transfer::local_window(&window)?;
    let exchange = exchange.0.as_ref().ok_or_else(closed)?;
    let row = db
        .conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT submission_id FROM hall_exchange_input WHERE transfer_id=?",
            vec![transfer_id.clone().into()],
        ))
        .await
        .map_err(|_| AppCommandError::io_error("Exchange storage is unavailable"))?
        .ok_or_else(|| AppCommandError::permission_denied("Exchange object is not available"))?;
    let submission: String = row
        .try_get("", "submission_id")
        .map_err(|_| AppCommandError::io_error("Exchange storage is unavailable"))?;
    let (file, _) = super::desktop_transfer::open_for_upload(path).await?;
    Ok(exchange
        .upload(&administrator(), &submission, &transfer_id, file)
        .await?)
}

/// Copy a collected artifact to a NEW file chosen by the operator.
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_exchange_save_artifact(
    window: tauri::WebviewWindow,
    exchange: tauri::State<'_, super::service::HostExchange>,
    object_id: String,
    destination: std::path::PathBuf,
) -> Result<(), AppCommandError> {
    super::desktop_transfer::local_window(&window)?;
    let exchange = exchange.0.as_ref().ok_or_else(closed)?;
    if !destination.is_absolute() {
        return Err(AppCommandError::invalid_input("Choose an absolute path"));
    }
    let source = exchange
        .open_collected(&administrator(), &object_id)
        .await?;
    let mut target = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .await
        .map_err(AppCommandError::io)?;
    let mut source = tokio::fs::File::from_std(source);
    tokio::io::copy(&mut source, &mut target)
        .await
        .map_err(AppCommandError::io)?;
    Ok(())
}
