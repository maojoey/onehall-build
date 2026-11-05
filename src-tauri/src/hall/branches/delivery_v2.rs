//! Headquarters-driven v2 delivery: manifest, input bytes, status, artifacts.
//! Every network step re-resolves the branch credential, and a collected
//! artifact never changes task execution status.
use super::{
    delivery::{apply, client, failure, intent, sql, storage, target, Intent, Report, Target},
    dispatch_wire_v2::EnvelopeV2,
};
use crate::{
    app_error::AppCommandError,
    hall::exchange::{
        service::administrator, ArtifactRef, AttemptOwner, DispatchBinding, Exchange,
    },
};
use futures_util::TryStreamExt;
use reqwest::{header, Method, RequestBuilder};
use sea_orm::{ConnectionTrait, DatabaseConnection, TransactionTrait};
use serde::Deserialize;
use std::time::Duration;
use tokio_util::io::{ReaderStream, StreamReader};

const LIMIT: usize = 256 * 1024;
/// task id, attempt id, run id, attempt number as reported by the branch.
type TaskIdentity = (String, Option<String>, Option<String>, Option<i64>);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    protocol_version: u32,
    task_exchange: bool,
}

/// Streaming client: bounded connect and idle time, but no total deadline so a
/// 512 MiB file can finish on a slow link.
fn stream_client() -> Result<&'static reqwest::Client, AppCommandError> {
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client, reqwest::Error>> =
        std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(4))
                .read_timeout(Duration::from_secs(30))
                .gzip(false)
                .brotli(false)
                .build()
        })
        .as_ref()
        .map_err(|_| failure())
}

/// Re-resolve before EVERY send; any edit to trust/endpoint aborts the sync.
async fn build(
    db: &DatabaseConnection,
    before: &Target,
    method: Method,
    path: &str,
    streaming: bool,
) -> Result<RequestBuilder, AppCommandError> {
    let current = target(db, &before.branch_id).await?;
    if current.issuer_id != before.issuer_id
        || current.secret != before.secret
        || current.endpoint != before.endpoint
    {
        return Err(failure());
    }
    let mut authorization =
        header::HeaderValue::from_str(&format!("Bearer {}.{}", current.issuer_id, current.secret))
            .map_err(|_| failure())?;
    authorization.set_sensitive(true);
    let client = if streaming {
        stream_client()?
    } else {
        client()?
    };
    Ok(client
        .request(
            method,
            format!("{}/hall-dispatch/v2{}", current.endpoint, path),
        )
        .header(header::AUTHORIZATION, authorization))
}

async fn json_body<T: for<'de> Deserialize<'de>>(
    mut response: reqwest::Response,
) -> Result<T, AppCommandError> {
    if response.content_length().is_some_and(|n| n > LIMIT as u64) {
        return Err(failure());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
        if bytes.len().saturating_add(chunk.len()) > LIMIT {
            return Err(failure());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| failure())
}

async fn detail(
    db: &DatabaseConnection,
    target: &Target,
    id: &str,
) -> Result<Option<Report>, AppCommandError> {
    let response = build(db, target, Method::GET, &format!("/inbox/{id}"), false)
        .await?
        .send()
        .await
        .map_err(|_| failure())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(failure());
    }
    json_body(response).await.map(Some)
}

pub(super) async fn deliver(
    db: &DatabaseConnection,
    exchange: &Exchange,
    item: &Intent,
    target: &Target,
    binding: DispatchBinding,
) -> Result<(), AppCommandError> {
    // The frozen attachment belongs to one issuer; a re-pointed credential
    // cannot receive its bytes.
    if binding.owner.issuer() != target.issuer_id {
        return Err(failure());
    }
    let id = item.dispatch_id.as_str();
    // Explicit version/capability check: no silent downgrade to v1.
    let caps = build(db, target, Method::GET, "/capabilities", false)
        .await?
        .send()
        .await
        .map_err(|_| failure())?;
    if !caps.status().is_success() {
        return Err(failure());
    }
    let caps: Capabilities = json_body(caps).await?;
    if caps.protocol_version != 2 || !caps.task_exchange {
        return Err(failure());
    }
    let report = match detail(db, target, id).await? {
        Some(report) => report,
        None => {
            if item.remote_revision > 0 {
                return Err(failure());
            }
            let envelope = EnvelopeV2 {
                dispatch_id: item.dispatch_id.clone(),
                branch_id: item.branch_id.clone(),
                member_ref: item.member_ref.clone(),
                title: item.title.clone(),
                description: item.description.clone(),
                manifest: binding.manifest.clone(),
            };
            let posted = build(db, target, Method::POST, "/inbox", false)
                .await?
                .json(&envelope)
                .send()
                .await
                .map_err(|_| failure())?;
            if !posted.status().is_success() {
                return Err(failure());
            }
            detail(db, target, id).await?.ok_or_else(failure)?
        }
    };
    let admin = administrator();
    // Only missing inputs travel; each is fully verified before the branch commits it.
    for object in &binding.manifest.inputs {
        if report.received_inputs.contains(&object.object_id) {
            continue;
        }
        let file = exchange
            .open_uploaded(&admin, &binding.submission, &object.object_id)
            .await
            .map_err(|_| failure())?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(tokio::fs::File::from_std(file)));
        let sent = build(
            db,
            target,
            Method::PUT,
            &format!("/inbox/{id}/inputs/{}", object.object_id),
            true,
        )
        .await?
        .header(header::CONTENT_LENGTH, object.size)
        .body(body)
        .send()
        .await
        .map_err(|_| failure())?;
        if !sent.status().is_success() {
            return Err(failure());
        }
    }
    let report = detail(db, target, id).await?.ok_or_else(failure)?;
    let artifacts = report.artifacts.clone();
    let task = report.task.as_ref().map(|t| {
        (
            t.task_id.clone(),
            t.attempt_id.clone(),
            t.run_id.clone(),
            t.attempt_number,
        )
    });
    apply(db, target, id, report).await?;
    collect(db, exchange, target, &binding, id, task, artifacts).await;
    Ok(())
}

/// Collection failures are recorded separately; the applied status stays.
async fn collect(
    db: &DatabaseConnection,
    exchange: &Exchange,
    target: &Target,
    binding: &DispatchBinding,
    id: &str,
    task: Option<TaskIdentity>,
    artifacts: Vec<ArtifactRef>,
) {
    let outcome = collect_inner(db, exchange, target, binding, id, task, artifacts).await;
    let message = outcome
        .err()
        .map(|_| "Artifact collection failed; sync again to retry".to_owned());
    let _ = db
        .execute(sql(
            "UPDATE hall_branch_dispatch SET artifact_sync_error=? WHERE dispatch_id=?",
            vec![message.into(), id.into()],
        ))
        .await;
}

async fn collect_inner(
    db: &DatabaseConnection,
    exchange: &Exchange,
    target: &Target,
    binding: &DispatchBinding,
    id: &str,
    task: Option<TaskIdentity>,
    artifacts: Vec<ArtifactRef>,
) -> Result<(), AppCommandError> {
    if artifacts.is_empty() {
        return Ok(());
    }
    let (Some((task_id, Some(attempt), Some(run), Some(number))), admin) = (task, administrator())
    else {
        return Err(failure());
    };
    let have: Vec<String> = exchange
        .collected_artifacts(&admin, id)
        .await
        .map_err(|_| failure())?
        .into_iter()
        .map(|a| a.object.object_id)
        .collect();
    for artifact in artifacts {
        // Only the current attempt's output is accepted; stale ones stay out.
        if artifact.task_id != task_id || artifact.attempt_id != attempt || artifact.run_id != run {
            continue;
        }
        if have.contains(&artifact.object.object_id) {
            continue;
        }
        let actor = AttemptOwner::authenticated(&binding.owner, &task_id, &attempt, &run, number)
            .map_err(|_| failure())?;
        let response = build(
            db,
            target,
            Method::GET,
            &format!("/inbox/{id}/artifacts/{}", artifact.object.object_id),
            true,
        )
        .await?
        .send()
        .await
        .map_err(|_| failure())?;
        if !response.status().is_success() {
            return Err(failure());
        }
        let reader = StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
        let received = exchange
            .receive_artifact(&admin, &actor, &artifact, reader)
            .await
            .map_err(|_| failure())?;
        let tx = db.begin().await.map_err(storage)?;
        // Same transaction: the branch must still be enabled with this credential.
        let live = target_in(&tx, target).await?;
        if !live {
            return Err(failure());
        }
        exchange
            .commit_collection_in(&tx, received)
            .await
            .map_err(|_| failure())?;
        tx.commit().await.map_err(storage)?;
    }
    Ok(())
}

async fn target_in<C: ConnectionTrait>(tx: &C, target: &Target) -> Result<bool, AppCommandError> {
    let updated = tx
        .execute(sql(
            "UPDATE hall_branch SET revision=revision WHERE id=? AND enabled=1 AND EXISTS \
             (SELECT 1 FROM hall_branch_dispatch_credential c WHERE c.branch_id=? AND c.issuer_id=? AND c.secret=? AND c.endpoint=?)",
            vec![
                target.branch_id.clone().into(),
                target.branch_id.clone().into(),
                target.issuer_id.clone().into(),
                target.secret.clone().into(),
                target.endpoint.clone().into(),
            ],
        ))
        .await
        .map_err(storage)?;
    Ok(updated.rows_affected() == 1)
}

/// True while an attachment dispatch still owes artifact collection.
pub(super) async fn collection_pending(
    db: &DatabaseConnection,
    id: &str,
) -> Result<bool, AppCommandError> {
    intent(db, id).await?;
    Ok(db
        .query_one(sql(
            "SELECT 1 FROM hall_branch_dispatch d JOIN hall_exchange_attachment a ON a.dispatch_id=d.dispatch_id WHERE d.dispatch_id=? AND d.artifact_sync_error IS NOT NULL",
            vec![id.into()],
        ))
        .await
        .map_err(storage)?
        .is_some())
}
