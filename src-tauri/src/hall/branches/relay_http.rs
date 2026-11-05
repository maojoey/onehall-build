use super::{
    policy::{self, Kind},
    relay::Runtime,
    relay_error::{check_upstream, RelayError},
    service,
};
use crate::models::ToHeaderMap;
use axum::{
    body::{Body, Bytes},
    extract::{Extension, Path, Request},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use futures::StreamExt;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::OwnedSemaphorePermit;

fn too_large() -> RelayError {
    RelayError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "branch_body_too_large",
        "Branch request exceeds the transfer limit",
    )
}
fn busy() -> RelayError {
    RelayError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "branch_busy",
        "Branch relay is busy; retry shortly",
    )
}
fn known_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}
fn json_content(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
}
pub(super) async fn command(
    Extension(runtime): Extension<Arc<Runtime>>,
    Path((branch, command)): Path<(String, String)>,
    request: Request,
) -> Result<Response, RelayError> {
    service::validate_uuid(&branch)?;
    let kind = policy::classify(&command).ok_or_else(|| {
        if policy::isolated_preview(&command) {
            RelayError::new(
                StatusCode::FORBIDDEN,
                "branch_preview_unavailable",
                "Branch HTML previews require an isolated origin and are unavailable",
            )
        } else {
            RelayError::new(
                StatusCode::FORBIDDEN,
                "branch_command_not_permitted",
                "This command is not available through a branch workspace",
            )
        }
    })?;
    if request.uri().query().is_some() {
        return Err(RelayError::new(
            StatusCode::BAD_REQUEST,
            "branch_invalid_request",
            "Branch command query parameters are not accepted",
        ));
    }
    let permit = runtime
        .http
        .clone()
        .try_acquire_owned()
        .map_err(|_| busy())?;
    let target = service::target(&runtime.db, &branch).await?;
    let (parts, body) = request.into_parts();
    let limit = if kind == Kind::Upload {
        runtime.limits.upload_bytes
    } else {
        runtime.limits.request_bytes
    };
    if known_length(&parts.headers).is_some_and(|n| n > limit) {
        return Err(too_large());
    }
    let content_type = parts.headers.get(header::CONTENT_TYPE).cloned();
    if kind == Kind::Upload
        && !content_type
            .as_ref()
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("multipart/form-data;"))
    {
        return Err(RelayError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "branch_invalid_upload",
            "A multipart upload is required",
        ));
    }
    if kind != Kind::Upload && !json_content(&parts.headers) {
        return Err(RelayError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "branch_invalid_json",
            "A JSON request is required",
        ));
    }
    let oversized = Arc::new(AtomicBool::new(false));
    let exceeded = oversized.clone();
    let mut received = 0u64;
    let input = body
        .into_data_stream()
        .map(move |chunk| -> Result<Bytes, io::Error> {
            let chunk = chunk.map_err(|_| io::Error::other("Branch request body failed"))?;
            received = received.saturating_add(chunk.len() as u64);
            if received > limit {
                exceeded.store(true, Ordering::Relaxed);
                return Err(io::Error::other("Branch request limit exceeded"));
            }
            Ok(chunk)
        });
    let mut outgoing = service::client()?
        .post(format!("{}/api/{command}", target.connection.base_url))
        .bearer_auth(target.connection.token.trim())
        .headers(target.connection.headers.to_header_map())
        .header(header::ACCEPT, "application/json")
        .timeout(if kind == Kind::Upload || kind == Kind::Download {
            runtime.limits.transfer_timeout
        } else {
            runtime.limits.http_timeout
        })
        .body(reqwest::Body::wrap_stream(input));
    // Never copy incoming authentication, cookies, identity, framing or Host.
    if let Some(value) = content_type {
        outgoing = outgoing.header(header::CONTENT_TYPE, value);
    }
    let response = outgoing.send().await.map_err(|e| {
        if oversized.load(Ordering::Relaxed) {
            too_large()
        } else {
            RelayError::upstream(e)
        }
    })?;
    check_upstream(&response)?;
    if kind == Kind::Ticket && response.status().is_success() {
        if !json_content(response.headers()) {
            return Err(RelayError::invalid_response());
        }
        let json = read_json(response, 64 * 1024).await?;
        let issued = runtime
            .tickets
            .mint(&branch, target.summary.revision, &json)
            .await
            .map_err(|_| RelayError::invalid_response())?;
        drop(permit);
        let bytes = serde_json::to_vec(&issued).map_err(|_| RelayError::invalid_response())?;
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .body(Body::from(bytes))
            .map_err(|_| RelayError::invalid_response());
    }
    let download = kind == Kind::Download && response.status().is_success();
    if !download && !json_content(response.headers()) {
        return Err(RelayError::invalid_response());
    }
    let limit = if download {
        runtime.limits.download_bytes
    } else {
        runtime.limits.request_bytes
    };
    stream_response(response, permit, download, limit)
}

pub(super) async fn read_json(
    response: reqwest::Response,
    limit: u64,
) -> Result<serde_json::Value, RelayError> {
    if response.content_length().is_some_and(|n| n > limit) {
        return Err(RelayError::invalid_response());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(RelayError::upstream)?;
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > limit {
            return Err(RelayError::invalid_response());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| RelayError::invalid_response())
}
fn stream_response(
    response: reqwest::Response,
    permit: OwnedSemaphorePermit,
    download: bool,
    limit: u64,
) -> Result<Response, RelayError> {
    if response.content_length().is_some_and(|n| n > limit) {
        return Err(RelayError::invalid_response());
    }
    let mut builder = Response::builder()
        .status(response.status())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    for name in [
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(value) = response.headers().get(&name) {
            builder = builder.header(name, value);
        }
    }
    if download {
        if let Some(value) = response.headers().get(header::CONTENT_TYPE) {
            builder = builder.header(header::CONTENT_TYPE, value);
        }
        // A remote HTML/SVG file must remain a downloaded file, never a script
        // at the operator's headquarters origin. Preserve a safe attachment name.
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .filter(|v| {
                v.to_str()
                    .ok()
                    .is_some_and(|s| s.to_ascii_lowercase().starts_with("attachment;"))
            })
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("attachment"));
        builder = builder
            .header(header::CONTENT_DISPOSITION, disposition)
            .header(
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'",
            );
    } else {
        // Pin JSON regardless of any upstream extension/charset value.
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let mut received = 0u64;
    let stream = response
        .bytes_stream()
        .map(move |chunk| -> Result<Bytes, io::Error> {
            // Keep the concurrency slot until body consumption/drop, including a
            // client which reads slowly. reqwest's deadline also covers the body.
            let _hold = &permit;
            let chunk = chunk.map_err(|_| io::Error::other("Branch response stream failed"))?;
            received = received.saturating_add(chunk.len() as u64);
            if received > limit {
                return Err(io::Error::other("Branch response limit exceeded"));
            }
            Ok(chunk)
        });
    builder
        .body(Body::from_stream(stream))
        .map_err(|_| RelayError::invalid_response())
}

pub(super) async fn download(
    Extension(runtime): Extension<Arc<Runtime>>,
    Path(cap): Path<String>,
    headers: HeaderMap,
) -> Result<Response, RelayError> {
    consume(runtime, None, cap, headers).await
}
pub(super) async fn branch_download(
    Extension(runtime): Extension<Arc<Runtime>>,
    Path((branch, cap)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, RelayError> {
    service::validate_uuid(&branch)?;
    consume(runtime, Some(branch), cap, headers).await
}
async fn consume(
    runtime: Arc<Runtime>,
    branch: Option<String>,
    cap: String,
    headers: HeaderMap,
) -> Result<Response, RelayError> {
    let permit = runtime
        .http
        .clone()
        .try_acquire_owned()
        .map_err(|_| busy())?;
    let ticket = runtime
        .tickets
        .take(&cap, branch.as_deref())
        .await
        .ok_or_else(|| {
            RelayError::new(
                StatusCode::NOT_FOUND,
                "branch_ticket_invalid",
                "Download ticket is invalid or expired",
            )
        })?;
    let target = service::target(&runtime.db, &ticket.branch_id).await?;
    if target.summary.revision != ticket.revision {
        return Err(RelayError::new(
            StatusCode::FORBIDDEN,
            "branch_ticket_revoked",
            "Branch registration changed; request a new download",
        ));
    }
    let mut request = service::client()?
        .get(format!(
            "{}/api/workspace_download/{}",
            target.connection.base_url, ticket.upstream
        ))
        .bearer_auth(target.connection.token.trim())
        .headers(target.connection.headers.to_header_map())
        .timeout(runtime.limits.transfer_timeout);
    if let Some(range) = headers.get(header::RANGE) {
        request = request.header(header::RANGE, range);
    }
    let response = request.send().await.map_err(RelayError::upstream)?;
    check_upstream(&response)?;
    // Error bodies are never rendered as HTML, even for a capability endpoint.
    if !response.status().is_success() {
        return Err(RelayError::invalid_response());
    }
    stream_response(response, permit, true, runtime.limits.download_bytes)
}
