//! Desktop byte adapters for the operator's own window (contract §4).
//! The window names a local file, a NEW destination and, for a remote
//! headquarters, a saved connection id. Stored credentials stay in Rust and
//! the receiving host still checks authentication, ownership, size and hash.
//! Nothing here is reachable from the HTTP router, relays or model tools.
#![cfg_attr(not(feature = "tauri-runtime"), allow(dead_code))]
use super::{
    files::{Directory, Source},
    manifest::validate_name,
    ObjectRef, MAX_FILE_BYTES,
};
use crate::{
    app_error::AppCommandError,
    db::service::remote_workspace_connection_service,
    models::{RemoteWorkspaceConnectionInfo, ToHeaderMap},
};
use sea_orm::ConnectionTrait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{path::Path, path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Idle limit between body chunks; a large transfer may take far longer overall.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Size and hash computed by the host, never by the page.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InspectedFile {
    pub display_name: String,
    pub size: u64,
    pub sha256: String,
}

fn not_regular() -> AppCommandError {
    AppCommandError::invalid_input(
        "Choose a regular file by its real path (no links, hard links or devices)",
    )
}
fn stopped() -> AppCommandError {
    AppCommandError::io_error("File transfer stopped")
}

/// Reuses the core's handle-anchored walk, so every ancestor and the file
/// itself are refused on links, reparse points, hard links and devices.
fn open_chosen(path: &Path) -> Result<Source, AppCommandError> {
    if !path.is_absolute() {
        return Err(not_regular());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(not_regular)?;
    let parent = path.parent().ok_or_else(not_regular)?;
    validate_name(name).map_err(|_| not_regular())?;
    let directory = Directory::absolute(parent).map_err(|_| not_regular())?;
    Source::open(directory, name).map_err(|_| not_regular())
}

/// Stream the file once to compute what `prepare_input` announces.
pub async fn inspect(path: PathBuf) -> Result<InspectedFile, AppCommandError> {
    tokio::task::spawn_blocking(move || {
        let mut source = open_chosen(&path)?;
        if source.len() > MAX_FILE_BYTES {
            return Err(super::ExchangeError::Quota.into());
        }
        let (size, sha256) = source.copy(&mut std::io::sink(), MAX_FILE_BYTES)?;
        Ok(InspectedFile {
            display_name: source.name().to_owned(),
            size,
            sha256,
        })
    })
    .await
    .map_err(|_| stopped())?
}

/// Open for streaming; the receiving core re-verifies size and hash.
pub async fn open_for_upload(path: PathBuf) -> Result<(tokio::fs::File, u64), AppCommandError> {
    let (file, size) = tokio::task::spawn_blocking(move || {
        open_chosen(&path).map(|source| {
            let size = source.len();
            (source.into_file(), size)
        })
    })
    .await
    .map_err(|_| stopped())??;
    Ok((tokio::fs::File::from_std(file), size))
}

/// A saved headquarters connection. Branch-linked connections are excluded
/// by the service, so a branch credential can never carry exchange bytes.
pub async fn headquarters<C: ConnectionTrait>(
    db: &C,
    connection_id: i32,
) -> Result<RemoteWorkspaceConnectionInfo, AppCommandError> {
    remote_workspace_connection_service::get(db, connection_id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| AppCommandError::not_found("Remote connection not found"))
}

/// Host-assigned ids only; they become URL path segments.
fn resource_id(value: &str) -> Result<&str, AppCommandError> {
    uuid::Uuid::parse_str(value)
        .map(|_| value)
        .map_err(|_| AppCommandError::invalid_input("Exchange request is invalid"))
}
fn client() -> Result<reqwest::Client, AppCommandError> {
    // A streamed body cannot be replayed, and a redirect would hand the
    // connection's credentials to whatever the hop names: refuse both.
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AppCommandError::network("Remote HTTP client is unavailable"))
}
fn endpoint(conn: &RemoteWorkspaceConnectionInfo, tail: &str) -> String {
    format!("{}/api/{tail}", conn.base_url.trim_end_matches('/'))
}
fn network(error: reqwest::Error) -> AppCommandError {
    // Without the URL: it is the operator's, but the error may be logged.
    AppCommandError::network("Remote HTTP request failed")
        .with_detail(error.without_url().to_string())
}
/// Control replies are JSON metadata, never the artifact byte stream.
async fn bounded_reply(mut response: reqwest::Response) -> Result<Vec<u8>, AppCommandError> {
    const LIMIT: usize = 64 * 1024;
    let too_large = || AppCommandError::network("Remote reply exceeds 64 KiB");
    if response
        .content_length()
        .is_some_and(|size| size > LIMIT as u64)
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if chunk.len() > LIMIT - body.len() {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Same mapping as the JSON proxy: 401 is an expired token, a structured
/// body keeps the remote code, anything else is a network failure.
async fn failure(response: reqwest::Response) -> AppCommandError {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return AppCommandError::authentication_failed("Remote Workspace token is invalid");
    }
    let body = match bounded_reply(response).await {
        Ok(body) => body,
        Err(error) => return error,
    };
    serde_json::from_slice::<AppCommandError>(&body)
        .unwrap_or_else(|_| AppCommandError::network(format!("Remote returned HTTP {status}")))
}

/// Stream an operator-chosen file to the remote HQ's transfer endpoint.
pub async fn remote_upload(
    conn: &RemoteWorkspaceConnectionInfo,
    transfer_id: &str,
    path: PathBuf,
) -> Result<ObjectRef, AppCommandError> {
    let transfer = resource_id(transfer_id)?;
    let (file, size) = open_for_upload(path).await?;
    let response = client()?
        .put(endpoint(
            conn,
            &format!("hall-exchange/transfers/{transfer}"),
        ))
        .bearer_auth(conn.token.trim())
        .headers(conn.headers.to_header_map())
        .header(reqwest::header::CONTENT_LENGTH, size)
        .body(reqwest::Body::wrap_stream(ReaderStream::new(file)))
        .send()
        .await
        .map_err(network)?;
    if !response.status().is_success() {
        return Err(failure(response).await);
    }
    let body = bounded_reply(response).await?;
    serde_json::from_slice(&body).map_err(|_| AppCommandError::network("Malformed upload receipt"))
}

fn mismatch() -> AppCommandError {
    AppCommandError::io_error("Downloaded artifact did not match its recorded size and hash")
}

/// Download a collected artifact into a NEW operator-chosen file. The bytes
/// must match the artifact's recorded size and hash, otherwise the partial
/// file this call created is removed. An existing file is never touched.
pub async fn remote_save(
    conn: &RemoteWorkspaceConnectionInfo,
    object_id: &str,
    size: u64,
    sha256: &str,
    destination: PathBuf,
) -> Result<(), AppCommandError> {
    let object = resource_id(object_id)?;
    let expected = sha256.to_ascii_lowercase();
    if size > MAX_FILE_BYTES
        || expected.len() != 64
        || !expected.bytes().all(|b| b.is_ascii_hexdigit())
        || !destination.is_absolute()
    {
        return Err(AppCommandError::invalid_input(
            "Exchange request is invalid",
        ));
    }
    let mut response = client()?
        .get(endpoint(conn, &format!("hall-exchange/artifacts/{object}")))
        .bearer_auth(conn.token.trim())
        .headers(conn.headers.to_header_map())
        .send()
        .await
        .map_err(network)?;
    if !response.status().is_success() {
        return Err(failure(response).await);
    }
    if response.content_length().is_some_and(|len| len != size) {
        return Err(mismatch());
    }
    let parent = destination.parent().ok_or_else(not_regular)?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(not_regular)?;
    validate_name(name).map_err(|_| not_regular())?;
    let directory = Directory::absolute(parent).map_err(|_| not_regular())?;
    match tokio::fs::symlink_metadata(&destination).await {
        Ok(_) => {
            return Err(AppCommandError::io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Destination already exists",
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(AppCommandError::io(error)),
    }
    // The random private file owns its own cleanup even when this future is
    // cancelled. Nothing removes or replaces the operator's destination path.
    let pending = tempfile::Builder::new()
        .prefix(".onehall-download-")
        .tempfile_in(parent)
        .map_err(AppCommandError::io)?;
    let mut target =
        tokio::fs::File::from_std(pending.as_file().try_clone().map_err(AppCommandError::io)?);
    let mut hash = Sha256::new();
    let mut received = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        received = received.saturating_add(chunk.len() as u64);
        if received > size {
            return Err(mismatch());
        }
        hash.update(&chunk);
        target
            .write_all(&chunk)
            .await
            .map_err(AppCommandError::io)?;
    }
    if received != size || format!("{:x}", hash.finalize()) != expected {
        return Err(mismatch());
    }
    target.sync_all().await.map_err(AppCommandError::io)?;
    drop(target.into_std().await);
    directory.validate().map_err(|_| stopped())?;
    // Windows needs write access to the final parent during rename. Keep a
    // non-removable child there so it cannot become a reparse point, and keep
    // DELETE sharing and all ancestor WRITE sharing denied.
    #[cfg(windows)]
    let _publication = directory.publication_guard().map_err(AppCommandError::io)?;
    // Persist is atomic and refuses a name created while the download ran.
    pending
        .persist_noclobber(destination)
        .map_err(|error| AppCommandError::io(error.error))?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum FileScope {
    Inspect,
    Local,
    Remote(i32),
}

/// Byte dialogs belong to trusted local UI, including a saved remote HQ
/// workspace. This is intentionally separate from device updater authority.
fn validate_file_window(label: &str, url: &url::Url, scope: FileScope) -> Result<(), &'static str> {
    const DENIED: &str = "Requires the local desktop or its matching remote workspace";
    let local_origin = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1")));
    if !local_origin {
        return Err(DENIED);
    }
    let mut remote = None;
    let mut instance = false;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "remoteConnectionId" => {
                if remote.is_some() {
                    return Err(DENIED);
                }
                remote = Some(
                    value
                        .parse::<i32>()
                        .ok()
                        .filter(|id| *id > 0)
                        .ok_or(DENIED)?,
                );
            }
            "remoteWindowId" => {
                if instance || uuid::Uuid::parse_str(&value).is_err() {
                    return Err(DENIED);
                }
                instance = true;
            }
            _ => {}
        }
    }
    if instance && remote.is_none() {
        return Err(DENIED);
    }
    if !matches!(label, "main" | "settings") {
        let id = remote.ok_or(DENIED)?;
        if label != format!("remote-workspace-{id}") {
            return Err(DENIED);
        }
    }
    match scope {
        FileScope::Inspect => Ok(()),
        FileScope::Local if remote.is_none() => Ok(()),
        FileScope::Remote(id) if id > 0 && (remote.is_none() || remote == Some(id)) => Ok(()),
        _ => Err(DENIED),
    }
}

#[cfg(feature = "tauri-runtime")]
fn file_window(window: &tauri::WebviewWindow, scope: FileScope) -> Result<(), AppCommandError> {
    let url = window
        .url()
        .map_err(|_| AppCommandError::permission_denied("Unable to verify local desktop"))?;
    validate_file_window(window.label(), &url, scope).map_err(AppCommandError::permission_denied)
}
#[cfg(feature = "tauri-runtime")]
pub(super) fn local_window(window: &tauri::WebviewWindow) -> Result<(), AppCommandError> {
    file_window(window, FileScope::Local)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_exchange_inspect_file(
    window: tauri::WebviewWindow,
    path: PathBuf,
) -> Result<InspectedFile, AppCommandError> {
    file_window(&window, FileScope::Inspect)?;
    inspect(path).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_exchange_remote_upload_file(
    window: tauri::WebviewWindow,
    db: tauri::State<'_, crate::db::AppDatabase>,
    connection_id: i32,
    transfer_id: String,
    path: PathBuf,
) -> Result<ObjectRef, AppCommandError> {
    file_window(&window, FileScope::Remote(connection_id))?;
    let conn = headquarters(&db.conn, connection_id).await?;
    remote_upload(&conn, &transfer_id, path).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn hall_exchange_remote_save_artifact(
    window: tauri::WebviewWindow,
    db: tauri::State<'_, crate::db::AppDatabase>,
    connection_id: i32,
    object_id: String,
    size: u64,
    sha256: String,
    destination: PathBuf,
) -> Result<(), AppCommandError> {
    file_window(&window, FileScope::Remote(connection_id))?;
    let conn = headquarters(&db.conn, connection_id).await?;
    remote_save(&conn, &object_id, size, &sha256, destination).await
}

#[cfg(test)]
#[path = "desktop_transfer_tests.rs"]
mod tests;
