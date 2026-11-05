use super::*;
use crate::{
    app_state::AppState,
    db::test_helpers::fresh_disk_db,
    hall::tasks::runtime::tests::Fake,
    hall::tasks_tests::{fixture, input, post, value},
};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
pub(super) async fn setup(
    injected: bool,
) -> (
    tempfile::TempDir,
    Arc<Controller>,
    axum_test::TestServer,
    Arc<Fake>,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let mut state = AppState::new_for_test(db, dir.path().into());
    let fake = Arc::new(Fake::default());
    if injected {
        state.hall_control = Arc::new(Controller {
            fixture: true,
            co: Arc::new(Coordinator::reviewed_fixture(
                QueueHost::new(state.db.conn.clone(), 1).unwrap(),
                fake.clone(),
            )),
            gate: Arc::default(),
            service: state.cli_runs.clone(),
            manager: state.connection_manager.clone_ref(),
            state: Mutex::new(State {
                active: BTreeMap::new(),
                certificate: None,
                probed: None,
            }),
            notify: Arc::new(Notify::new()),
            root: None,
        });
    }
    let control = state.hall_control.clone();
    let router = crate::web::router::build_router(
        Arc::new(state),
        "hall-registry-test-only".into(),
        dir.path().join("static"),
        Arc::new(crate::web::shutdown::ShutdownSignal::new()),
    )
    .layer(axum::Extension(axum::extract::ConnectInfo(
        "127.0.0.1:43210".parse::<std::net::SocketAddr>().unwrap(),
    )));
    let server = axum_test::TestServer::new(router).unwrap();
    (dir, control, server, fake)
}
async fn action(s: &axum_test::TestServer, r: Value) -> Value {
    value(post(s, "hall_team_control", json!({"request":r})).await)
}
pub(super) async fn queued(s: &axum_test::TestServer, dir: &std::path::Path, name: &str) -> String {
    queued_for(s, dir, name, true).await
}
async fn queued_for(
    s: &axum_test::TestServer,
    dir: &std::path::Path,
    name: &str,
    allowed: bool,
) -> String {
    let (m, snap) = fixture(s, dir, name).await;
    let member = value(
        post(
            s,
            "hall_set_member_enabled",
            json!({"memberId":m["memberId"],"enabled":allowed}),
        )
        .await,
    );
    assert_eq!(member["enabled"], allowed);
    let id = value(post(s, "hall_create_task", input(&m, &snap)).await)["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let p = value(post(s, "hall_review_task", json!({"taskId":id})).await);
    value(post(s,"hall_approve_task",json!({"approval":{"taskId":id,"expectedRevision":1,"planHash":p["planHash"],"submissionId":uuid::Uuid::new_v4().to_string()}})).await);
    id
}
#[tokio::test]
async fn production_control_rejects_forged_capability_before_claim_and_api_requires_auth() {
    let (dir, c, s, f) = setup(false).await;
    let id = queued(&s, dir.path(), "closed").await;
    assert_eq!(
        s.post("/api/hall_team_control")
            .json(&json!({"request":{"action":"state"}}))
            .await
            .status_code(),
        401
    );
    let ready = action(&s, json!({"action":"state"})).await;
    assert_eq!(ready["enabled"], false);
    assert_eq!(ready["nativeVerified"], false);
    assert_eq!(ready["reason"], "run_root_unavailable");
    assert_eq!(
        post(
            &s,
            "hall_team_control",
            json!({"request":{"action":"state","enabled":true}})
        )
        .await
        .status_code(),
        422
    );
    assert_eq!(
        post(
            &s,
            "hall_team_control",
            json!({"request":{"action":"enable","scope":{}}})
        )
        .await
        .status_code(),
        422
    );
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    assert!(!c.gate.allows(&id));
    assert_eq!(f.starts.load(Ordering::SeqCst), 0);
    let d = get_task(c.co.database(), id).await.unwrap();
    assert_eq!(d.attempts[0].status, "queued");
}
#[tokio::test]
async fn probe_without_a_run_root_issues_no_evidence() {
    let (_dir, c, s, _f) = setup(false).await;
    assert!(c.probe().await.is_err());
    assert!(!c.readiness().await.admission.native_verified);
    assert_eq!(
        post(
            &s,
            "hall_team_control",
            json!({"request":{"action":"native_probe","path":"/tmp"}})
        )
        .await
        .status_code(),
        422
    );
}
#[tokio::test]
async fn member_switch_off_blocks_dispatch_before_claim() {
    let (dir, c, s, f) = setup(true).await;
    let id = queued_for(&s, dir.path(), "off", false).await;
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    assert_eq!(f.starts.load(Ordering::SeqCst), 0);
    let d = get_task(c.co.database(), id).await.unwrap();
    assert_eq!(d.task.status, "queued");
    assert_eq!(d.attempts[0].status, "queued");
}
#[tokio::test]
async fn authenticated_controls_dispatch_exact_task_cancel_recover_and_reopen_without_replay() {
    let (dir, c, s, f) = setup(true).await;
    let first = queued(&s, dir.path(), "first").await;
    let second = queued(&s, dir.path(), "second").await;
    let d = action(
        &s,
        json!({"action":"dispatch","task_id":second,"expected_revision":2}),
    )
    .await;
    assert_eq!(d["taskId"], second);
    assert_eq!(f.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        get_task(c.co.database(), first.clone())
            .await
            .unwrap()
            .task
            .status,
        "queued"
    );
    let replay = action(
        &s,
        json!({"action":"dispatch","task_id":second,"expected_revision":2}),
    )
    .await;
    assert_eq!(replay["taskId"], second);
    assert_eq!(f.starts.load(Ordering::SeqCst), 1);
    let running = get_task(c.co.database(), second.clone()).await.unwrap();
    assert!(c.cancel(second.clone(), 2).await.is_err());
    let cancelled = action(
        &s,
        json!({"action":"cancel","task_id":second,"expected_revision":running.task.revision}),
    )
    .await;
    assert_eq!(cancelled["attempts"][0]["status"], "cancel_requested");
    action(&s, json!({"action":"refresh","task_id":second})).await;
    assert!(get_task(c.co.database(), second.clone())
        .await
        .unwrap()
        .attempts[0]
        .finished_at
        .is_none());
    assert!(c.dispatch(first.clone(), 2).await.is_err());
    {
        let mut state = c.state.lock().await;
        let a = state.active.get_mut(&second).unwrap();
        a.paused = true;
    }
    let before = c.recover(second.clone()).await.unwrap();
    assert_eq!(before.task.status, "interrupted");
    assert!(before.attempts[0].finished_at.is_none());
    f.exited.store(true, Ordering::SeqCst);
    let after = c.recover(second.clone()).await.unwrap();
    assert_eq!(after.task.status, "cancelled");
    assert!(after.attempts[0].finished_at.is_some());
    let fresh = Controller::closed(
        c.co.database().clone(),
        c.service.clone(),
        c.manager.clone_ref(),
    );
    assert!(!fresh.readiness().await.admission.enabled);
    assert!(fresh.recover(second).await.is_err());
    assert_eq!(f.starts.load(Ordering::SeqCst), 1);
}
#[test]
fn team_concurrency_defaults_to_two_and_ignores_out_of_range_values() {
    assert_eq!(concurrency_from(None), 2);
    assert_eq!(concurrency_from(Some(" 3 ")), 3);
    assert_eq!(concurrency_from(Some("1")), 1);
    for bad in ["0", "9", "-1", "two", ""] {
        assert_eq!(concurrency_from(Some(bad)), 2, "{bad}");
    }
}

#[tokio::test]
async fn local_human_resolution_requires_acknowledgement_and_is_idempotent() {
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "resolution").await;
    let running = c.dispatch(id.clone(), 2).await.unwrap();
    f.best_effort.store(true, Ordering::SeqCst);
    f.exited.store(true, Ordering::SeqCst);
    c.cancel(id.clone(), running.task.revision).await.unwrap();
    let paused = c.refresh(id.clone()).await.unwrap();
    assert_eq!(paused.runtimes[0].stage, "needs_review");
    let request = json!({"action":"resolve_best_effort","task_id":id,"expected_revision":paused.task.revision,"acknowledge_unverified_descendants":true});
    assert_eq!(
        s.post("/api/hall_team_control")
            .json(&json!({"request":request}))
            .await
            .status_code(),
        401
    );
    let mut missing_ack = request.clone();
    missing_ack
        .as_object_mut()
        .unwrap()
        .remove("acknowledge_unverified_descendants");
    assert_eq!(
        post(&s, "hall_team_control", json!({"request":missing_ack}))
            .await
            .status_code(),
        422
    );
    let mut no_ack = request.clone();
    no_ack["acknowledge_unverified_descendants"] = json!(false);
    assert_eq!(
        post(&s, "hall_team_control", json!({"request":no_ack}))
            .await
            .status_code(),
        403
    );
    let mut stale = request.clone();
    stale["expected_revision"] = json!(paused.task.revision - 1);
    assert_eq!(
        post(&s, "hall_team_control", json!({"request":stale}))
            .await
            .status_code(),
        403
    );
    let (first, repeat) = tokio::join!(action(&s, request.clone()), action(&s, request));
    assert_eq!(first["revision"], repeat["revision"]);
    assert_eq!(first["status"], "interrupted");
    assert_eq!(first["runtimes"][0]["stage"], "terminal");
    assert!(first["runtimes"][0]["exitConfirmedAt"].is_null());
    assert!(first["attempts"][0]["finishedAt"].is_null());
    assert_eq!(first["exitChecks"][0]["complete"], false);
    let result = get_task(c.co.database(), id).await.unwrap();
    assert_eq!(
        result
            .history
            .iter()
            .filter(|e| e.action == "human_best_effort_resolution")
            .count(),
        1
    );
}

#[tokio::test]
async fn failed_start_recovery_is_reachable_without_active_binding() {
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "orphan").await;
    f.fail_start.store(true, Ordering::SeqCst);
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    let recovered = c.recover(id.clone()).await;
    assert!(
        recovered.is_ok(),
        "persistent recovery must not require active"
    );
    assert_eq!(recovered.unwrap().task.status, "interrupted");
    assert_eq!(f.starts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn orphan_action_is_denied_to_browser_even_with_local_peer() {
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "orphan-api").await;
    f.fail_start.store(true, Ordering::SeqCst);
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    let d = get_task(c.co.database(), id.clone()).await.unwrap();
    let r = json!({"action":"resolve_orphan", "task_id":id,
        "attempt_id":d.attempts.last().unwrap().attempt_id,
        "expected_revision":d.task.revision,
        "acknowledge_unverified_processes":true,"reason":"已在本机核查运行记录"});
    assert_eq!(
        s.post("/api/hall_team_control")
            .add_header("authorization", "Bearer hall-registry-test-only")
            .add_header("origin", "https://evil.invalid")
            .json(&json!({"request":r}))
            .await
            .status_code(),
        403
    );
}

#[tokio::test]
async fn orphan_api_checks_revision_attempt_and_exact_replay() {
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "orphan-api-contract").await;
    f.fail_start.store(true, Ordering::SeqCst);
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    let d = get_task(c.co.database(), id.clone()).await.unwrap();
    let request = json!({"action":"resolve_orphan","task_id":id,
        "attempt_id":d.attempts[0].attempt_id,"expected_revision":d.task.revision,
        "acknowledge_unverified_processes":true,"reason":"已在本机核查运行记录"});
    for (key, value, status) in [
        ("attempt_id", json!(uuid::Uuid::new_v4().to_string()), 409),
        ("expected_revision", json!(d.task.revision - 1), 409),
        ("acknowledge_unverified_processes", json!(false), 403),
        ("reason", json!("  "), 400),
    ] {
        let mut wrong = request.clone();
        wrong[key] = value;
        assert_eq!(
            post(&s, "hall_team_control", json!({"request":wrong}))
                .await
                .status_code(),
            status
        );
    }
    let first = action(&s, request.clone()).await;
    let repeat = action(&s, request.clone()).await;
    assert_eq!(first, repeat);
    assert_eq!(first["resolution"]["kind"], "operator_released");
    assert!(first["runtimes"][0]["exitConfirmedAt"].is_null());
    let mut changed = request;
    changed["reason"] = json!("different");
    assert_eq!(
        post(&s, "hall_team_control", json!({"request":changed}))
            .await
            .status_code(),
        409
    );
}
