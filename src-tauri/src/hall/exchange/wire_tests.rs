//! HQ <-> branch exchange over real HTTP and two real SQLite databases. The
//! branch side reuses the runtime fixture, so claims, runs and exits are real.
use super::{result, Fixture, Outcome};
use crate::hall::exchange::{
    runtime::{attempt_owner, current_owner},
    service::administrator,
    DispatchOwner, Exchange, ExchangeManifest,
};
use crate::{
    db::test_helpers::fresh_disk_db,
    hall::branches::{
        delivery, dispatch, dispatch_wire, local, model::RegisterBranchInput, service,
    },
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;

pub(super) const CONTENT: &[u8] = "{\"numbers\":[3,5,8,13],\"note\":\"一堂文件中转验收\"}".as_bytes();
pub(super) const OUTPUT: &[u8] = "result: 29\n".as_bytes();
pub(super) fn digest(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
pub(super) fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub(super) fn sql(q: &str, v: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, q, v)
}
pub(super) fn open(db: &DatabaseConnection, dir: &std::path::Path) -> Exchange {
    #[cfg(unix)]
    let root = dir.canonicalize().unwrap().join("exchange");
    #[cfg(windows)]
    let root = dir.join("exchange");
    Exchange::open(db.clone(), &root).unwrap()
}
pub(super) struct Hq {
    db: DatabaseConnection,
    exchange: Exchange,
    _dir: tempfile::TempDir,
}
impl Hq {
    pub(super) async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await.conn;
        let exchange = open(&db, dir.path());
        Self {
            db,
            exchange,
            _dir: dir,
        }
    }
    pub(super) async fn stage(&self) -> (String, ExchangeManifest) {
        let admin = administrator();
        let submission = uid();
        let ticket = self
            .exchange
            .prepare_input(
                &admin,
                &submission,
                "in.json",
                CONTENT.len() as u64,
                &digest(CONTENT),
            )
            .await
            .unwrap();
        self.exchange
            .upload(&admin, &submission, &ticket.transfer_id, CONTENT)
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
/// Branch-side ledger rows that local acceptance would have written.
pub(super) async fn bind_task(db: &DatabaseConnection, issuer: &str, dispatch: &str, task: &str) {
    db.execute(sql(
        "INSERT INTO hall_branch_task_binding(issuer_id,dispatch_id,task_id,request_hash) VALUES(?,?,?,?)",
        vec![issuer.into(), dispatch.into(), task.into(), "fixture".into()],
    ))
    .await
    .unwrap();
    db.execute(sql(
        "UPDATE hall_branch_inbox SET status='accepted',revision=revision+1 WHERE issuer_id=? AND dispatch_id=?",
        vec![issuer.into(), dispatch.into()],
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn staged_input_runs_seals_and_collects_once_even_when_replayed() {
    let f = Fixture::new().await;
    let branch_ex = open(&f.db, f._dir.path());
    f.co.attach_exchange(branch_ex.clone());
    // Branch host: real router, real credential export.
    let credential = f._dir.path().join("credential.json");
    local::execute_json(
        &f.db,
        &serde_json::to_vec(&json!({"action":"create_trust","name":"HQ","exportPath":credential}))
            .unwrap(),
    )
    .await
    .unwrap();
    let packet: Value = serde_json::from_slice(&std::fs::read(&credential).unwrap()).unwrap();
    let (issuer, secret) = (
        packet["issuerId"].as_str().unwrap().to_owned(),
        packet["secret"].as_str().unwrap().to_owned(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let router = dispatch_wire::routes(f.db.clone(), Some(branch_ex.clone()));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    // HQ: stage, register the branch, freeze the manifest into the dispatch.
    let hq = Hq::new().await;
    let (submission, manifest) = hq.stage().await;
    let branch = service::register(
        &hq.db,
        RegisterBranchInput {
            name: "Remote".into(),
            base_url: endpoint.clone(),
            token: "workspace-fixture".into(),
            headers: vec![],
        },
    )
    .await
    .unwrap();
    let base: String = hq
        .db
        .query_one(sql(
            "SELECT r.base_url FROM hall_branch b JOIN remote_workspace_connection r ON r.id=b.connection_id WHERE b.id=?",
            vec![branch.id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "base_url")
        .unwrap();
    hq.db
        .execute(sql(
            "INSERT INTO hall_branch_dispatch_credential(branch_id,issuer_id,secret,endpoint) VALUES(?,?,?,?)",
            vec![branch.id.clone().into(), issuer.clone().into(), secret.into(), base.into()],
        ))
        .await
        .unwrap();
    let draft = |submission_id: &str, manifest_id: &str| dispatch::Draft {
        submission_id: submission_id.into(),
        branch_id: branch.id.clone(),
        member_ref: "worker".into(),
        title: "Exchange".into(),
        description: "Use the staged input".into(),
        exchange_manifest_id: Some(manifest_id.into()),
    };
    // Wrong submission for a manifest, and a plain create with a manifest, are refused.
    assert!(dispatch::create_with_exchange(
        &hq.db,
        Some(&hq.exchange),
        draft(&uid(), &manifest.manifest_id)
    )
    .await
    .is_err());
    assert!(
        dispatch::create(&hq.db, draft(&submission, &manifest.manifest_id))
            .await
            .is_err()
    );
    let item = dispatch::create_with_exchange(
        &hq.db,
        Some(&hq.exchange),
        draft(&submission, &manifest.manifest_id),
    )
    .await
    .unwrap();
    // Without the exchange handle an attachment dispatch is never sent as v1.
    assert!(delivery::sync(&hq.db, &item.dispatch_id)
        .await
        .unwrap()
        .sync_error
        .is_some());
    let first = delivery::sync_with(&hq.db, Some(&hq.exchange), &item.dispatch_id)
        .await
        .unwrap();
    assert_eq!(first.status, "awaiting_acceptance");
    assert!(first.sync_error.is_none());

    // Branch: inputs are verified on disk; a queued task is bound and launched.
    let confirmed: i64 =
        f.db.query_one(sql(
            "SELECT COUNT(*) AS n FROM hall_exchange_input WHERE confirmed=1",
            vec![],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap();
    assert_eq!(confirmed, 1);
    let task = f.queued().await;
    bind_task(&f.db, &issuer, &item.dispatch_id, &task).await;
    // Branch review shows the real names, sizes and output slots, bound by the
    // digest; the display copy does not enter the plan hash.
    let plan = crate::hall::tasks::review::preview(&f.db, &task)
        .await
        .unwrap();
    assert_eq!(
        plan.exchange_manifest_digest.as_deref(),
        Some(manifest.digest.as_str())
    );
    assert_eq!(plan.exchange_manifest.as_ref(), Some(&manifest));
    let wire = serde_json::to_value(&plan).unwrap();
    assert_eq!(
        wire["exchangeManifest"]["inputs"][0]["displayName"],
        "in.json"
    );
    assert_eq!(
        wire["exchangeManifest"]["outputs"][0]["displayName"],
        "out.txt"
    );
    // Persist an approval, then remove the newly added display copy to model
    // a pre-upgrade approved row. Both replay and the real dispatch guard must
    // accept it without changing the digest-bound approval or starting twice.
    let submission_id = uid();
    let approve = || crate::hall::tasks::review::Approval {
        task_id: task.clone(),
        expected_revision: plan.task_revision,
        plan_hash: plan.plan_hash.clone(),
        submission_id: submission_id.clone(),
    };
    let approved = crate::hall::tasks::review::approve(&f.db, approve())
        .await
        .unwrap();
    let mut legacy = wire.clone();
    legacy.as_object_mut().unwrap().remove("exchangeManifest");
    let legacy = serde_json::to_string(&legacy).unwrap();
    f.db.execute(sql(
        "UPDATE hall_task_review SET plan_json=? WHERE attempt_id=?",
        vec![legacy.into(), approved.attempt_id.clone().into()],
    ))
    .await
    .unwrap();
    let replay = crate::hall::tasks::review::approve(&f.db, approve())
        .await
        .unwrap();
    assert_eq!(replay.attempt_id, approved.attempt_id);
    crate::hall::tasks::review::validate_dispatch(&f.db, &approved.attempt_id, &task)
        .await
        .unwrap();
    let inbox = local::execute_json(&f.db, br#"{"action":"list_inbox"}"#)
        .await
        .unwrap();
    assert_eq!(
        inbox[0]["exchangeManifest"],
        serde_json::to_value(&manifest).unwrap()
    );
    f.port.best_effort.store(true, Ordering::SeqCst); // macOS: exit cannot be proven
    let (claim, binding) = f.co.dispatch_next().await.unwrap().unwrap();
    let actor = attempt_owner(&f.db, &task, claim.attempt_id(), binding.run_id())
        .await
        .unwrap()
        .expect("attached task has an exchange owner");
    assert_eq!(
        branch_ex.list_inputs(&actor).await.unwrap(),
        manifest.inputs
    );
    let (current, verification) = current_owner(&f.db, &task).await.unwrap().unwrap();
    let paths = branch_ex.materialize(&current).await.unwrap();
    let slot = manifest.outputs[0].slot_id.clone();
    std::fs::write(&paths.outputs[&slot], OUTPUT).unwrap();
    let sealed = branch_ex.seal(&current, &slot, verification).await.unwrap();
    assert!(!sealed.exit_verified);

    // HQ pulls the artifact; replays and a lost response never duplicate it.
    for _ in 0..3 {
        let synced = delivery::sync_with(&hq.db, Some(&hq.exchange), &item.dispatch_id)
            .await
            .unwrap();
        assert!(synced.sync_error.is_none());
    }
    let admin = administrator();
    let collected = hq
        .exchange
        .collected_artifacts(&admin, &item.dispatch_id)
        .await
        .unwrap();
    assert_eq!(collected, vec![sealed.clone()]);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut hq
            .exchange
            .open_collected(&admin, &sealed.object.object_id)
            .await
            .unwrap(),
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes, OUTPUT);
    // Collecting files must not turn an unverified exit into a verified success.
    let row = hq
        .db
        .query_one(sql(
            "SELECT status,task_status,remote_exit_verified,artifact_sync_error FROM hall_branch_dispatch WHERE dispatch_id=?",
            vec![item.dispatch_id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(row.try_get::<String>("", "status").unwrap(), "succeeded");
    assert_eq!(
        row.try_get::<Option<bool>>("", "remote_exit_verified")
            .unwrap(),
        Some(false)
    );
    assert!(row
        .try_get::<Option<String>>("", "artifact_sync_error")
        .unwrap()
        .is_none());

    // Finishing and releasing the run revokes further writes but keeps history.
    f.co.accept_event(&claim, &binding, 1, result(Outcome::Succeeded))
        .await
        .unwrap();
    f.port.best_effort.store(false, Ordering::SeqCst);
    f.port.exited.store(true, Ordering::SeqCst);
    f.co.confirm_exit(&claim, &binding).await.unwrap();
    assert!(branch_ex.seal(&current, &slot, verification).await.is_err());
    assert_eq!(
        branch_ex
            .sealed_artifacts(
                &DispatchOwner::authenticated(&issuer, &item.dispatch_id, &branch.id).unwrap()
            )
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn unverified_inputs_block_review_and_launch_and_failed_start_leaves_a_revocation() {
    let f = Fixture::new().await;
    let branch_ex = open(&f.db, f._dir.path());
    f.co.attach_exchange(branch_ex.clone());
    let hq = Hq::new().await;
    let (_, manifest) = hq.stage().await;
    let (issuer, dispatch, hq_branch) = (uid(), uid(), uid());
    let owner = DispatchOwner::authenticated(&issuer, &dispatch, &hq_branch).unwrap();
    {
        use sea_orm::TransactionTrait;
        let tx = f.db.begin().await.unwrap();
        branch_ex
            .accept_dispatch_in(&tx, &owner, &dispatch, &manifest)
            .await
            .unwrap();
        tx.execute(sql(
            "INSERT INTO hall_branch_inbox(issuer_id,dispatch_id,request_hash,member_ref,title,description,status,revision,created_at,updated_at) VALUES(?,?,?,?,?,?,'awaiting_confirmation',1,'t','t')",
            vec![issuer.clone().into(), dispatch.clone().into(), "h".into(), "worker".into(), "t".into(), "d".into()],
        ))
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let task = f.queued().await;
    bind_task(&f.db, &issuer, &dispatch, &task).await;
    // The reviewed plan cannot even be built before every byte is verified.
    assert!(crate::hall::tasks::review::preview(&f.db, &task)
        .await
        .is_err());
    assert!(f.co.dispatch_next().await.is_err());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 0);
    let revoked: i64 =
        f.db.query_one(sql(
            "SELECT COUNT(*) AS n FROM hall_exchange_attempt WHERE revoked=1",
            vec![],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap();
    assert_eq!(revoked, 1, "failed start leaves a tombstone for its run");
}

/// Drops the response of the first matching write AFTER the branch handled it,
/// i.e. exactly the "committed but the reply was lost" case.
fn lossy(
    router: axum::Router,
    lose: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
) -> axum::Router {
    router.layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let lose = lose.clone();
            async move {
                let tag = match (request.method().as_str(), request.uri().path()) {
                    ("POST", p) if p.ends_with("/v2/inbox") => "post_inbox",
                    ("PUT", p) if p.contains("/inputs/") => "put_input",
                    ("GET", p) if p.contains("/artifacts/") => "get_artifact",
                    _ => "",
                };
                let response = next.run(request).await;
                let mut pending = lose.lock().unwrap();
                if let Some(at) = pending.iter().position(|t| *t == tag) {
                    pending.remove(at);
                    return axum::response::IntoResponse::into_response(
                        axum::http::StatusCode::BAD_GATEWAY,
                    );
                }
                response
            }
        },
    ))
}
async fn count(db: &DatabaseConnection, table: &str) -> i64 {
    db.query_one(sql(&format!("SELECT COUNT(*) AS n FROM {table}"), vec![]))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}

#[tokio::test]
async fn lost_responses_at_every_network_write_and_a_hq_restart_never_duplicate_anything() {
    let f = Fixture::new().await;
    let branch_ex = open(&f.db, f._dir.path());
    f.co.attach_exchange(branch_ex.clone());
    let credential = f._dir.path().join("credential.json");
    local::execute_json(
        &f.db,
        &serde_json::to_vec(&json!({"action":"create_trust","name":"HQ","exportPath":credential}))
            .unwrap(),
    )
    .await
    .unwrap();
    let packet: Value = serde_json::from_slice(&std::fs::read(&credential).unwrap()).unwrap();
    let (issuer, secret) = (
        packet["issuerId"].as_str().unwrap().to_owned(),
        packet["secret"].as_str().unwrap().to_owned(),
    );
    let lose = std::sync::Arc::new(std::sync::Mutex::new(vec![
        "post_inbox",
        "put_input",
        "get_artifact",
    ]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let router = lossy(
        dispatch_wire::routes(f.db.clone(), Some(branch_ex.clone())),
        lose.clone(),
    );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let hq = Hq::new().await;
    let (submission, manifest) = hq.stage().await;
    let branch = service::register(
        &hq.db,
        RegisterBranchInput {
            name: "Remote".into(),
            base_url: endpoint,
            token: "workspace-fixture".into(),
            headers: vec![],
        },
    )
    .await
    .unwrap();
    let base: String = hq
        .db
        .query_one(sql(
            "SELECT r.base_url FROM hall_branch b JOIN remote_workspace_connection r ON r.id=b.connection_id WHERE b.id=?",
            vec![branch.id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "base_url")
        .unwrap();
    hq.db
        .execute(sql(
            "INSERT INTO hall_branch_dispatch_credential(branch_id,issuer_id,secret,endpoint) VALUES(?,?,?,?)",
            vec![branch.id.clone().into(), issuer.clone().into(), secret.into(), base.into()],
        ))
        .await
        .unwrap();
    let item = dispatch::create_with_exchange(
        &hq.db,
        Some(&hq.exchange),
        dispatch::Draft {
            submission_id: submission,
            branch_id: branch.id.clone(),
            member_ref: "worker".into(),
            title: "Exchange".into(),
            description: "Use the staged input".into(),
            exchange_manifest_id: Some(manifest.manifest_id.clone()),
        },
    )
    .await
    .unwrap();
    let sync = || delivery::sync_with(&hq.db, Some(&hq.exchange), &item.dispatch_id);

    // 1. The POST committed on the branch but its reply was lost.
    assert!(sync().await.unwrap().sync_error.is_some());
    assert_eq!(count(&f.db, "hall_branch_inbox").await, 1);
    // 2. The input PUT committed but its reply was lost.
    assert!(sync().await.unwrap().sync_error.is_some());
    assert_eq!(
        count(&f.db, "hall_exchange_input WHERE confirmed=1").await,
        1
    );
    // 3. Recovery: nothing is re-sent, still exactly one inbox item and input.
    let recovered = sync().await.unwrap();
    assert!(recovered.sync_error.is_none());
    assert_eq!(recovered.status, "awaiting_acceptance");
    assert_eq!(count(&f.db, "hall_branch_inbox").await, 1);
    assert_eq!(count(&f.db, "hall_exchange_input").await, 1);

    // Branch accepts, runs once and seals.
    let task = f.queued().await;
    bind_task(&f.db, &issuer, &item.dispatch_id, &task).await;
    let (claim, binding) = f.co.dispatch_next().await.unwrap().unwrap();
    let (current, verification) = current_owner(&f.db, &task).await.unwrap().unwrap();
    let paths = branch_ex.materialize(&current).await.unwrap();
    let slot = manifest.outputs[0].slot_id.clone();
    std::fs::write(&paths.outputs[&slot], OUTPUT).unwrap();
    let sealed = branch_ex.seal(&current, &slot, verification).await.unwrap();
    let _ = (&claim, &binding);

    // 4. Sealed on the branch, but the artifact download reply was lost.
    let lost = sync().await.unwrap();
    assert!(lost.sync_error.is_none(), "status itself was applied");
    let admin = administrator();
    assert!(hq
        .exchange
        .collected_artifacts(&admin, &item.dispatch_id)
        .await
        .unwrap()
        .is_empty());
    let pending: i64 = count(
        &hq.db,
        &format!(
            "hall_branch_dispatch WHERE dispatch_id='{}' AND artifact_sync_error IS NOT NULL",
            item.dispatch_id
        ),
    )
    .await;
    assert_eq!(pending, 1, "unfinished collection is recorded, not hidden");
    // 5. Retry collects it and clears the marker.
    sync().await.unwrap();
    assert_eq!(
        hq.exchange
            .collected_artifacts(&admin, &item.dispatch_id)
            .await
            .unwrap(),
        vec![sealed.clone()]
    );
    assert!(lose.lock().unwrap().is_empty(), "every loss point was hit");

    // 6. HQ restarts: a fresh Exchange over the same database and store.
    let restarted = open(&hq.db, hq._dir.path());
    for _ in 0..2 {
        delivery::sync_with(&hq.db, Some(&restarted), &item.dispatch_id)
            .await
            .unwrap();
    }
    let after = restarted
        .collected_artifacts(&admin, &item.dispatch_id)
        .await
        .unwrap();
    assert_eq!(after, vec![sealed.clone()]);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut restarted
            .open_collected(&admin, &sealed.object.object_id)
            .await
            .unwrap(),
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes, OUTPUT);
    assert_eq!(count(&hq.db, "hall_exchange_collection").await, 1);
    // Exactly one dispatch, task, attempt and sealed artifact on the branch.
    assert_eq!(count(&hq.db, "hall_branch_dispatch").await, 1);
    assert_eq!(count(&f.db, "hall_branch_task_binding").await, 1);
    assert_eq!(count(&f.db, "hall_task_attempt").await, 1);
    assert_eq!(
        count(&f.db, "hall_exchange_slot WHERE blob_id IS NOT NULL").await,
        1
    );
}
