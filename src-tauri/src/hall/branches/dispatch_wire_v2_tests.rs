use super::{dispatch_wire, trust};
use crate::{
    db::test_helpers::fresh_disk_db,
    hall::exchange::{service::administrator, Exchange, ExchangeManifest},
};
use axum_test::TestServer;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) const CONTENT: &[u8] =
    "{\"numbers\":[3,5,8,13],\"note\":\"一堂文件中转验收\"}".as_bytes();
pub(super) fn digest(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
pub(super) fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// One host: database, private exchange store, and temp directory.
pub(super) struct Host {
    pub db: sea_orm::DatabaseConnection,
    pub exchange: Exchange,
    pub _dir: tempfile::TempDir,
}
impl Host {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await.conn;
        #[cfg(unix)]
        let root = dir.path().canonicalize().unwrap().join("exchange");
        #[cfg(windows)]
        let root = dir.path().join("exchange");
        let exchange = Exchange::open(db.clone(), &root).unwrap();
        Self {
            db,
            exchange,
            _dir: dir,
        }
    }
    /// HQ-side staging of one input and an output slot named `out.txt`.
    pub async fn manifest(&self, bytes: &[u8]) -> (String, ExchangeManifest) {
        let admin = administrator();
        let submission = uid();
        let ticket = self
            .exchange
            .prepare_input(
                &admin,
                &submission,
                "in.json",
                bytes.len() as u64,
                &digest(bytes),
            )
            .await
            .unwrap();
        self.exchange
            .upload(&admin, &submission, &ticket.transfer_id, bytes)
            .await
            .unwrap();
        self.exchange
            .confirm_input(&admin, &submission, &ticket.object_id)
            .await
            .unwrap();
        let manifest = self
            .exchange
            .prepare_manifest(
                &admin,
                &submission,
                &[ticket.object_id],
                &["out.txt".into()],
            )
            .await
            .unwrap();
        (submission, manifest)
    }
}
pub(super) fn envelope(dispatch: &str, branch: &str, manifest: &ExchangeManifest) -> Value {
    json!({"dispatchId":dispatch,"branchId":branch,"memberRef":"local-worker",
           "title":"Exchange dispatch","description":"Use the staged input.","manifest":manifest})
}

async fn branch() -> (Host, TestServer, String) {
    let host = Host::new().await;
    let issuer = trust::create_local(&host.db, "HQ").await.unwrap();
    let server = TestServer::new(dispatch_wire::routes(
        host.db.clone(),
        Some(host.exchange.clone()),
    ))
    .unwrap();
    (
        host,
        server,
        format!("{}.{}", issuer.issuer_id, issuer.secret),
    )
}

#[tokio::test]
async fn v2_capabilities_require_issuer_credentials_and_v1_only_hosts_have_no_v2() {
    let (_host, server, token) = branch().await;
    server
        .get("/hall-dispatch/v2/capabilities")
        .await
        .assert_status_unauthorized();
    server
        .get("/hall-dispatch/v2/capabilities")
        .authorization_bearer("ordinary-web-token")
        .await
        .assert_status_unauthorized();
    let ok = server
        .get("/hall-dispatch/v2/capabilities")
        .authorization_bearer(&token)
        .await;
    ok.assert_status_ok();
    let body = ok.json::<Value>();
    assert_eq!(body["protocolVersion"], 2);
    assert_eq!(body["taskExchange"], true);
    assert_eq!(body["maxFileBytes"], 512u64 * 1024 * 1024);
    assert_eq!(body["maxManifestItems"], 200);
    let db = Host::new().await;
    let issuer = trust::create_local(&db.db, "HQ").await.unwrap();
    let old = TestServer::new(dispatch_wire::routes(db.db.clone(), None)).unwrap();
    old.get("/hall-dispatch/v2/capabilities")
        .authorization_bearer(format!("{}.{}", issuer.issuer_id, issuer.secret))
        .await
        .assert_status_not_found();
}

#[tokio::test]
async fn v1_rejects_attachment_fields_and_v1_tasks_keep_working() {
    let (_host, server, token) = branch().await;
    let hq = Host::new().await;
    let (_, manifest) = hq.manifest(CONTENT).await;
    let id = uid();
    let mut forged =
        json!({"dispatchId":id,"memberRef":"local-worker","title":"t","description":"d"});
    forged["manifest"] = serde_json::to_value(&manifest).unwrap();
    server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&token)
        .json(&forged)
        .await
        .assert_status_unprocessable_entity();
    forged.as_object_mut().unwrap().remove("manifest");
    server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&token)
        .json(&forged)
        .await
        .assert_status_ok();
    // A plain v1 item can never gain a manifest afterwards.
    server
        .post("/hall-dispatch/v2/inbox")
        .authorization_bearer(&token)
        .json(&envelope(&id, &uid(), &manifest))
        .await
        .assert_status_conflict();
}

#[tokio::test]
async fn inputs_are_accepted_only_for_registered_objects_and_replay_idempotently() {
    let (host, server, token) = branch().await;
    let hq = Host::new().await;
    let (_, manifest) = hq.manifest(CONTENT).await;
    let (dispatch, hq_branch) = (uid(), uid());
    let object = &manifest.inputs[0].object_id;
    let path = format!("/hall-dispatch/v2/inbox/{dispatch}/inputs/{object}");
    // Nothing may be uploaded before the manifest is registered.
    server
        .put(&path)
        .authorization_bearer(&token)
        .bytes(CONTENT.to_vec().into())
        .await
        .assert_status_not_found();
    let post = server
        .post("/hall-dispatch/v2/inbox")
        .authorization_bearer(&token)
        .json(&envelope(&dispatch, &hq_branch, &manifest))
        .await;
    post.assert_status_ok();
    let replay = server
        .post("/hall-dispatch/v2/inbox")
        .authorization_bearer(&token)
        .json(&envelope(&dispatch, &hq_branch, &manifest))
        .await;
    assert_eq!(replay.json::<Value>(), post.json::<Value>());
    let wrong = server
        .put(&path)
        .authorization_bearer(&token)
        .bytes(b"tampered".to_vec().into())
        .await;
    wrong.assert_status_conflict();
    let other = format!("/hall-dispatch/v2/inbox/{dispatch}/inputs/{}", uid());
    server
        .put(&other)
        .authorization_bearer(&token)
        .bytes(CONTENT.to_vec().into())
        .await
        .assert_status_forbidden();
    for _ in 0..2 {
        server
            .put(&path)
            .authorization_bearer(&token)
            .bytes(CONTENT.to_vec().into())
            .await
            .assert_status(axum::http::StatusCode::NO_CONTENT);
    }
    let detail = server
        .get(&format!("/hall-dispatch/v2/inbox/{dispatch}"))
        .authorization_bearer(&token)
        .await;
    detail.assert_status_ok();
    assert_eq!(detail.json::<Value>()["receivedInputs"], json!([object]));
    assert_eq!(detail.json::<Value>()["artifacts"], json!([]));
    // A changed manifest for the same dispatch cannot reuse the old approval.
    let (_, other_manifest) = hq.manifest(b"different").await;
    server
        .post("/hall-dispatch/v2/inbox")
        .authorization_bearer(&token)
        .json(&envelope(&dispatch, &hq_branch, &other_manifest))
        .await
        .assert_status_conflict();
    drop(host);
}

#[tokio::test]
async fn issuers_are_isolated_and_revocation_closes_transfer_and_download() {
    let host = Host::new().await;
    let a = trust::create_local(&host.db, "A").await.unwrap();
    let b = trust::create_local(&host.db, "B").await.unwrap();
    let server = TestServer::new(dispatch_wire::routes(
        host.db.clone(),
        Some(host.exchange.clone()),
    ))
    .unwrap();
    let (ta, tb) = (
        format!("{}.{}", a.issuer_id, a.secret),
        format!("{}.{}", b.issuer_id, b.secret),
    );
    let hq = Host::new().await;
    let (_, manifest) = hq.manifest(CONTENT).await;
    let (dispatch, branch_id) = (uid(), uid());
    server
        .post("/hall-dispatch/v2/inbox")
        .authorization_bearer(&ta)
        .json(&envelope(&dispatch, &branch_id, &manifest))
        .await
        .assert_status_ok();
    let object = &manifest.inputs[0].object_id;
    let path = format!("/hall-dispatch/v2/inbox/{dispatch}/inputs/{object}");
    server
        .put(&path)
        .authorization_bearer(&tb)
        .bytes(CONTENT.to_vec().into())
        .await
        .assert_status_not_found();
    server
        .get(&format!("/hall-dispatch/v2/inbox/{dispatch}"))
        .authorization_bearer(&tb)
        .await
        .assert_status_not_found();
    trust::revoke_local(&host.db, &a.issuer_id, 1)
        .await
        .unwrap();
    server
        .put(&path)
        .authorization_bearer(&ta)
        .bytes(CONTENT.to_vec().into())
        .await
        .assert_status_unauthorized();
    server
        .get(&format!(
            "/hall-dispatch/v2/inbox/{dispatch}/artifacts/{object}"
        ))
        .authorization_bearer(&ta)
        .await
        .assert_status_unauthorized();
}

#[tokio::test]
async fn manifest_json_over_64_kib_is_rejected_before_parsing() {
    let (_host, server, token) = branch().await;
    let mut body = envelope(
        &uid(),
        &uid(),
        &serde_json::from_value(json!({
        "schemaVersion":1,"manifestId":uid(),"inputs":[],"outputs":[],"digest":"0".repeat(64)}))
        .unwrap(),
    );
    body["description"] = "x".repeat(70 * 1024).into();
    server
        .post("/hall-dispatch/v2/inbox")
        .authorization_bearer(&token)
        .json(&body)
        .await
        .assert_status_payload_too_large();
}

#[test]
fn published_contract_samples_parse_with_the_real_types() {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/hall/exchange/contract_samples/"
    );
    let read = |name: &str| std::fs::read_to_string(format!("{dir}{name}")).unwrap();
    let envelope: super::dispatch_wire_v2::EnvelopeV2 =
        serde_json::from_str(&read("envelope_v2.json")).unwrap();
    assert_eq!(envelope.manifest.inputs.len(), 1);
    let report: super::delivery::Report = serde_json::from_str(&read("report_v2.json")).unwrap();
    assert_eq!(report.artifacts.len(), 1);
    assert!(!report.artifacts[0].exit_verified);
    let requests: Vec<crate::hall::exchange::api::Request> =
        serde_json::from_str(&read("hall_exchange_requests.json")).unwrap();
    assert_eq!(requests.len(), 4);
    let caps: Value = serde_json::from_str(&read("capabilities.json")).unwrap();
    assert_eq!(caps["protocolVersion"], 2);
}

#[tokio::test]
async fn failed_store_initialization_neither_advertises_nor_accepts_v2() {
    let host = Host::new().await;
    let blocked = host._dir.path().join("not-a-directory");
    std::fs::write(&blocked, b"not a data directory").unwrap();
    let unavailable = crate::hall::exchange::service::open_default(host.db.clone(), &blocked);
    assert!(unavailable.is_none());
    let issuer = trust::create_local(&host.db, "HQ").await.unwrap();
    let server = TestServer::new(dispatch_wire::routes(host.db.clone(), unavailable)).unwrap();
    let token = format!("{}.{}", issuer.issuer_id, issuer.secret);
    server
        .get("/hall-dispatch/v2/capabilities")
        .authorization_bearer(&token)
        .await
        .assert_status_not_found();
    let (_, manifest) = host.manifest(CONTENT).await;
    let id = uid();
    server
        .post("/hall-dispatch/v2/inbox")
        .authorization_bearer(&token)
        .json(&envelope(&id, &uid(), &manifest))
        .await
        .assert_status_not_found();
}
