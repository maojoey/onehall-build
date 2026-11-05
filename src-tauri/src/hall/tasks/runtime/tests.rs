use super::*;
use super::{
    authority::{DerivedRequest, Registry},
    port::RuntimePort,
};
use crate::{
    app_error::AppCommandError,
    db::test_helpers::fresh_disk_db,
    hall::{
        members_tests::server,
        tasks::{get_task, queue::QueueHost},
        tasks_tests::{fixture, input, post, value},
    },
};
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
#[derive(Default)]
pub(super) struct Fake {
    pub(super) starts: AtomicUsize,
    pub(super) best_effort: AtomicBool,
    pub(super) cancels: AtomicUsize,
    pub(super) exited: AtomicBool,
    pub(super) fail_start: AtomicBool,
    pub(super) fail_reserve: AtomicBool,
    pub(super) fail_activate: AtomicBool,
    pub(super) bad_binding: AtomicBool,
    pub(super) aborts: AtomicUsize,
    pub(super) abort_result: AtomicUsize,
    pub(super) gap: AtomicBool,
    /// Pretend the host is Windows for the execution policy preflight.
    pub(super) windows: AtomicBool,
    pub(super) reserves: AtomicUsize,
}
#[async_trait::async_trait]
impl RuntimePort for Fake {
    async fn validate_start(&self, p: &RunPlan) -> Result<(), AppCommandError> {
        use crate::hall::members::execution_policy::{check_start_on, Platform};
        let platform = if self.windows.load(Ordering::SeqCst) {
            Platform::Windows
        } else {
            Platform::Macos
        };
        check_start_on(p.launch_profile(), platform)
    }
    async fn reserve(&self) -> Result<String, AppCommandError> {
        self.reserves.fetch_add(1, Ordering::SeqCst);
        if self.fail_reserve.load(Ordering::SeqCst) {
            return Err(AppCommandError::io_error("fixture reservation"));
        }
        Ok(uuid::Uuid::new_v4().to_string())
    }
    async fn start(&self, run: &str, p: &RunPlan) -> Result<Binding, AppCommandError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        assert!(!p.cwd().is_empty());
        if self.fail_start.load(Ordering::SeqCst) {
            return Err(AppCommandError::io_error("fixture uncertain start"));
        }
        Ok(Binding {
            owner: p.owner.clone(),
            lease_id: run.into(),
            run_id: run.into(),
            connection_id: uuid::Uuid::new_v4().to_string(),
            conversation_id: if self.bad_binding.load(Ordering::SeqCst) {
                0
            } else {
                1
            },
            external_session_id: uuid::Uuid::new_v4().to_string(),
            started_at: Utc::now(),
        })
    }
    async fn activate(&self, _: &Binding) -> Result<(), AppCommandError> {
        if self.fail_activate.load(Ordering::SeqCst) {
            Err(AppCommandError::io_error("fixture activation"))
        } else {
            Ok(())
        }
    }
    async fn abort_start(&self, _: &StartIdentity) -> Result<AbortStatus, AppCommandError> {
        self.aborts.fetch_add(1, Ordering::SeqCst);
        Ok(match self.abort_result.load(Ordering::SeqCst) {
            1 => AbortStatus::NotStarted,
            2 => AbortStatus::ExitVerified,
            _ => AbortStatus::ExitUnverified,
        })
    }
    async fn cancel(&self, _: &Binding) -> Result<(), AppCommandError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn events(
        &self,
        _: &Binding,
        _: u64,
    ) -> Result<Vec<(u64, RuntimeEvent)>, AppCommandError> {
        if self.gap.load(Ordering::SeqCst) {
            Err(AppCommandError::io_error("fixture gap"))
        } else {
            Ok(vec![])
        }
    }
    async fn confirm_exit(&self, b: &Binding) -> Result<Option<ExitEvidence>, AppCommandError> {
        Ok(
            (self.exited.load(Ordering::SeqCst) && !self.best_effort.load(Ordering::SeqCst)).then(
                || ExitEvidence {
                    owner: b.owner.clone(),
                    lease_id: b.lease_id.clone(),
                    run_id: b.run_id.clone(),
                    connection_id: b.connection_id.clone(),
                    confirmed_at: Utc::now(),
                },
            ),
        )
    }
    async fn exit_observation(&self, b: &Binding) -> Option<ExitObservation> {
        self.best_effort
            .load(Ordering::SeqCst)
            .then(|| ExitObservation {
                attempt_id: b.owner.attempt.clone(),
                lease_id: b.lease_id.clone(),
                scope_hash: b.owner.digest(),
                root_birth_hash: Some("fixture-kernel-birth".into()),
                root_reaped: self.exited.load(Ordering::SeqCst),
                known_processes: 1,
                known_reaped: usize::from(self.exited.load(Ordering::SeqCst)),
                pending_spawns: 0,
                coverage: "macos_process_group_best_effort".into(),
                complete: false,
                reason: "descendant_coverage_unconfirmed".into(),
                observed_at: Utc::now(),
            })
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    db: DatabaseConnection,
    server: axum_test::TestServer,
    host: QueueHost,
    port: Arc<Fake>,
    co: Coordinator,
    member: Value,
    snapshot: Value,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await;
        let conn = db.conn.clone();
        let s = server(db, dir.path());
        let (m, snap) = fixture(&s, dir.path(), "member").await;
        let host = QueueHost::new(conn.clone(), 2).unwrap();
        let port = Arc::new(Fake::default());
        let co = Coordinator::fixture(host.clone(), port.clone());
        Self {
            _dir: dir,
            db: conn,
            server: s,
            host,
            port,
            co,
            member: m,
            snapshot: snap,
        }
    }
    async fn queued(&self) -> String {
        self.queued_for(&self.member, &self.snapshot).await
    }
    async fn queued_for(&self, m: &Value, s: &Value) -> String {
        let t = value(post(&self.server, "hall_create_task", input(m, s)).await);
        let id = t["taskId"].as_str().unwrap();
        self.host
            .enqueue(id, 1, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        id.into()
    }
    async fn detail(&self, id: &str) -> Value {
        serde_json::to_value(get_task(&self.db, id.into()).await.unwrap()).unwrap()
    }
}
fn result(outcome: Outcome) -> RuntimeEvent {
    RuntimeEvent::Result {
        outcome,
        body: "durable fixture result".into(),
    }
}
#[tokio::test]
async fn production_gate_denies_before_claim_or_runtime_side_effect() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    let closed = Coordinator::production_closed(f.host.clone(), f.port.clone());
    assert!(closed.dispatch_next().await.is_err());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 0);
    assert_eq!(f.detail(&id).await["attempts"][0]["status"], "queued");
}
#[tokio::test]
async fn two_members_serialize_and_capacity_releases_only_with_exit_proof() {
    let f = Fixture::new().await;
    let a = f.queued().await;
    f.queued().await;
    let (m, s) = fixture(&f.server, f._dir.path(), "b").await;
    let b = f.queued_for(&m, &s).await;
    let (c1, b1) = f.co.dispatch_next().await.unwrap().unwrap();
    let (c2, _) = f.co.dispatch_next().await.unwrap().unwrap();
    assert_eq!(c1.task_id(), a);
    assert_eq!(c2.task_id(), b);
    assert!(f.co.dispatch_next().await.unwrap().is_none());
    f.co.accept_event(&c1, &b1, 1, result(Outcome::Succeeded))
        .await
        .unwrap();
    assert!(!f.co.confirm_exit(&c1, &b1).await.unwrap());
    assert!(f.co.dispatch_next().await.unwrap().is_none());
    f.port.exited.store(true, Ordering::SeqCst);
    assert!(f.co.confirm_exit(&c1, &b1).await.unwrap());
    assert!(f.co.dispatch_next().await.unwrap().is_some());
}
#[tokio::test]
async fn events_are_fenced_idempotent_and_wait_cards_are_independent() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    let (c, b) = f.co.dispatch_next().await.unwrap().unwrap();
    let e = RuntimeEvent::WaitingInput {
        key: "first".into(),
        waiting: true,
    };
    let (x, y) = tokio::join!(
        f.co.accept_event(&c, &b, 1, e.clone()),
        f.co.accept_event(&c, &b, 1, e)
    );
    assert_ne!(x.unwrap(), y.unwrap());
    assert!(f
        .co
        .accept_event(&c, &b, 1, result(Outcome::Failed))
        .await
        .is_err());
    f.co.accept_event(
        &c,
        &b,
        2,
        RuntimeEvent::WaitingInput {
            key: "second".into(),
            waiting: true,
        },
    )
    .await
    .unwrap();
    f.co.accept_event(
        &c,
        &b,
        3,
        RuntimeEvent::WaitingInput {
            key: "first".into(),
            waiting: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(f.detail(&id).await["status"], "awaiting_permission");
    assert!(!f
        .co
        .accept_event(&c, &b, 2, result(Outcome::Failed))
        .await
        .unwrap());
    f.co.accept_event(
        &c,
        &b,
        4,
        RuntimeEvent::WaitingInput {
            key: "second".into(),
            waiting: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(f.detail(&id).await["status"], "running");
    let other = Binding {
        owner: b.owner.clone(),
        lease_id: b.lease_id.clone(),
        run_id: b.run_id.clone(),
        connection_id: "forged".into(),
        conversation_id: b.conversation_id,
        external_session_id: b.external_session_id.clone(),
        started_at: b.started_at,
    };
    assert!(f
        .co
        .accept_event(&c, &other, 5, result(Outcome::Succeeded))
        .await
        .is_err());
}
#[tokio::test]
async fn cancel_ack_is_not_exit_late_success_cannot_reverse_cancel_and_retry_preserves_result() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    let (c, b) = f.co.dispatch_next().await.unwrap().unwrap();
    assert!(f
        .co
        .accept_event(&c, &b, 1, RuntimeEvent::CancelAcknowledged)
        .await
        .is_err());
    f.co.request_cancel(&c, &b).await.unwrap();
    f.co.request_cancel(&c, &b).await.unwrap();
    assert_eq!(f.port.cancels.load(Ordering::SeqCst), 2);
    f.co.accept_event(&c, &b, 1, RuntimeEvent::CancelAcknowledged)
        .await
        .unwrap();
    f.co.accept_event(&c, &b, 2, result(Outcome::Succeeded))
        .await
        .unwrap();
    let d = f.detail(&id).await;
    assert_eq!(d["attempts"][0]["status"], "cancel_requested");
    assert!(d["attempts"][0]["finishedAt"].is_null());
    assert!(!d["runtimes"][0]["cancelAckAt"].is_null());
    assert!(f
        .host
        .enqueue(
            &id,
            d["revision"].as_i64().unwrap(),
            &uuid::Uuid::new_v4().to_string()
        )
        .await
        .is_err());
    assert!(!f.co.confirm_exit(&c, &b).await.unwrap());
    f.port.exited.store(true, Ordering::SeqCst);
    f.co.confirm_exit(&c, &b).await.unwrap();
    let d = f.detail(&id).await;
    assert_eq!(d["status"], "cancelled");
    assert_eq!(d["results"][0]["outcome"], "succeeded");
    f.host
        .enqueue(
            &id,
            d["revision"].as_i64().unwrap(),
            &uuid::Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
    let d = f.detail(&id).await;
    assert_eq!(d["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(d["attempts"][0]["status"], "cancelled");
    assert_eq!(d["results"][0]["body"], "durable fixture result");
    assert!(f
        .co
        .accept_event(&c, &b, 3, result(Outcome::Failed))
        .await
        .is_err());
}
#[tokio::test]
async fn queued_cancel_never_records_start_or_exit_and_can_be_retried() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.host.cancel_queued(&id, 2).await.unwrap();
    let d = f.detail(&id).await;
    assert_eq!(d["status"], "cancelled");
    assert!(d["attempts"][0]["startedAt"].is_null());
    assert!(d["attempts"][0]["finishedAt"].is_null());
    assert!(f.co.dispatch_next().await.unwrap().is_none());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 0);
    f.host
        .enqueue(&id, 3, &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert_eq!(f.detail(&id).await["attempts"][1]["attemptNumber"], 2);
}
#[tokio::test]
async fn changed_snapshot_identity_rejects_before_start_and_keeps_times_empty() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "UPDATE hall_member SET revision=revision+1",
    ))
    .await
    .unwrap();
    assert!(f.co.dispatch_next().await.is_err());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 0);
    let d = f.detail(&id).await;
    assert_eq!(d["status"], "failed");
    assert_eq!(d["runtimes"][0]["stage"], "rejected");
    assert!(d["attempts"][0]["startedAt"].is_null());
    assert!(d["attempts"][0]["finishedAt"].is_null());
}
#[tokio::test]
async fn uncertain_start_and_restart_never_replay_or_free_unknown_capacity() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.port.fail_start.store(true, Ordering::SeqCst);
    assert!(f.co.dispatch_next().await.is_err());
    let d = f.detail(&id).await;
    assert_eq!(d["status"], "interrupted");
    assert!(d["attempts"][0]["finishedAt"].is_null());
    f.queued().await;
    let reboot = QueueHost::new(f.db.clone(), 1).unwrap();
    reboot.reconcile_previous_boots().await.unwrap();
    let co = Coordinator::fixture(reboot, f.port.clone());
    assert!(co.dispatch_next().await.unwrap().is_none());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn reboot_fences_old_claim_and_lost_event_stream_requires_review() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    let (c, b) = f.co.dispatch_next().await.unwrap().unwrap();
    let reboot = QueueHost::new(f.db.clone(), 2).unwrap();
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 1);
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 0);
    assert!(f
        .co
        .accept_event(&c, &b, 1, result(Outcome::Succeeded))
        .await
        .is_err());
    let d = f.detail(&id).await;
    assert_eq!(d["runtimes"][0]["stage"], "needs_review");
    assert!(d["attempts"][0]["finishedAt"].is_null());
    let (m, s) = fixture(&f.server, f._dir.path(), "b").await;
    f.queued_for(&m, &s).await;
    let (c, b) = f.co.dispatch_next().await.unwrap().unwrap();
    f.port.gap.store(true, Ordering::SeqCst);
    assert!(f.co.poll_once(&c, &b).await.is_err());
    assert_eq!(f.detail(c.task_id()).await["status"], "interrupted");
}
#[tokio::test]
async fn result_is_immutable_and_readable_after_database_reopen_without_cli_cache() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    let (c, b) = f.co.dispatch_next().await.unwrap().unwrap();
    f.co.accept_event(&c, &b, 1, result(Outcome::Failed))
        .await
        .unwrap();
    f.co.accept_event(&c, &b, 2, result(Outcome::Succeeded))
        .await
        .unwrap();
    f.port.exited.store(true, Ordering::SeqCst);
    f.co.confirm_exit(&c, &b).await.unwrap();
    let d = f.detail(&id).await;
    assert_eq!(d["status"], "failed");
    assert_eq!(d["results"].as_array().unwrap().len(), 1);
    let url = format!(
        "sqlite:{}?mode=rwc",
        f._dir.path().join("source.db").display()
    );
    let reopened = sea_orm::Database::connect(url).await.unwrap();
    let reread = get_task(&reopened, id).await.unwrap();
    assert_eq!(reread.results[0].body, "durable fixture result");
    assert_eq!(reread.results[0].outcome, "failed");
    assert!(!reread.task.launch_allowed);
    assert!(!reread.task.runtime_verified);
}
#[tokio::test]
async fn trusted_identity_requires_actual_binding_and_derived_scope_cannot_expand() {
    let f = Fixture::new().await;
    f.queued().await;
    let (c, b) = f.co.dispatch_next().await.unwrap().unwrap();
    let task = crate::hall::tasks::store::by_id(&f.db, c.task_id())
        .await
        .unwrap();
    let profile =
        crate::hall::members::snapshot_launch_profile(&f.db, &task.member_id, &task.snapshot_id)
            .await
            .unwrap();
    let p = RunPlan {
        owner: b.owner.clone(),
        profile,
        task_id: task.task_id,
        attempt_id: c.attempt_id().into(),
        snapshot_id: task.snapshot_id,
        description: task.description,
    };
    let r = Registry::default();
    r.prepare(b.run_id(), &p);
    assert!(r
        .actor_for_bound_connection(b.run_id(), b.connection_id())
        .is_err());
    let a = r.bind(&b).unwrap();
    assert!(r.protects_connection(b.connection_id()));
    assert!(r.authorize_pm(&a).is_err());
    assert!(r.authorize_member(&a, "forged PM").is_err());
    let request = |cwd| DerivedRequest {
        member: p.member_id(),
        snapshot: &p.snapshot_id,
        cwd,
        engine: "codex",
        assistant: p.profile.assistant_uid.as_deref(),
        mode: p.mode(),
        config: p.options(),
    };
    assert!(r.authorize_derived(&a, request(p.cwd())).is_ok());
    assert!(r.authorize_derived(&a, request("/")).is_err());
    r.configure_fixture_pm(p.member_id());
    assert!(r.authorize_pm(&a).is_err());
    r.revoke(b.run_id());
    // Revocation removes authority while preserving restrictions on a process
    // whose exit has not necessarily been proved.
    assert!(r.protects_connection(b.connection_id()));
    assert!(r.own_task(&a).is_err());
    r.prepare(b.run_id(), &p);
    assert!(r.bind(&b).is_err(), "a revoked run cannot be resurrected");
    let fresh = Binding {
        run_id: uuid::Uuid::new_v4().to_string(),
        connection_id: uuid::Uuid::new_v4().to_string(),
        ..b
    };
    r.prepare(fresh.run_id(), &p);
    let a = r.bind(&fresh).unwrap();
    assert!(r.authorize_pm(&a).is_ok());
}
#[cfg(unix)]
mod owned_process_fixture {
    use super::*;
    use std::{
        collections::HashMap,
        io::{BufRead, BufReader, Write},
        process::{Child, Command, Stdio},
        sync::Mutex,
    };
    struct Owned {
        child: Child,
        output: BufReader<std::process::ChildStdout>,
        cancelled: bool,
        reaped: bool,
        connection: String,
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            if let Some(mut input) = self.child.stdin.take() {
                let _ = input.write_all(b"cancel\n");
            }
            let _ = self.child.wait();
        }
    }
    #[derive(Default)]
    struct Processes {
        rows: Mutex<HashMap<String, Owned>>,
        fake: Fake,
    }
    #[async_trait::async_trait]
    impl RuntimePort for Processes {
        async fn reserve(&self) -> Result<String, AppCommandError> {
            self.fake.reserve().await
        }
        async fn start(&self, run: &str, p: &RunPlan) -> Result<Binding, AppCommandError> {
            let b = self.fake.start(run, p).await?;
            // Disposable fixture only: this Python parent owns exactly one
            // sleep descendant, and reports REAPED only after wait(). No
            // process-name/PID scans or group/global kill operations.
            let code = "import subprocess,sys\np=subprocess.Popen(['/bin/sleep',\
                '120'])\nprint('READY',flush=True)\ntry:\n \
                sys.stdin.readline()\nfinally:\n p.terminate()\n p.wait()\n \
                print('REAPED',flush=True)\n";
            let mut child = Command::new("python3")
                .args(["-u", "-c", code])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), "READY");
            self.rows.lock().unwrap().insert(
                run.into(),
                Owned {
                    child,
                    output,
                    cancelled: false,
                    reaped: false,
                    connection: b.connection_id.clone(),
                },
            );
            Ok(b)
        }
        async fn cancel(&self, b: &Binding) -> Result<(), AppCommandError> {
            let mut rows = self.rows.lock().unwrap();
            let owned = rows.get_mut(b.run_id()).unwrap();
            assert_eq!(owned.connection, b.connection_id);
            if !owned.cancelled {
                owned
                    .child
                    .stdin
                    .as_mut()
                    .unwrap()
                    .write_all(b"cancel\n")
                    .unwrap();
                owned.cancelled = true;
            }
            Ok(())
        }
        async fn events(
            &self,
            _: &Binding,
            _: u64,
        ) -> Result<Vec<(u64, RuntimeEvent)>, AppCommandError> {
            Ok(vec![])
        }
        async fn confirm_exit(&self, b: &Binding) -> Result<Option<ExitEvidence>, AppCommandError> {
            let mut rows = self.rows.lock().unwrap();
            let owned = rows.get_mut(b.run_id()).unwrap();
            assert_eq!(owned.connection, b.connection_id);
            if !owned.cancelled {
                return Ok(None);
            }
            if !owned.reaped {
                let mut line = String::new();
                owned.output.read_line(&mut line).unwrap();
                assert_eq!(line.trim(), "REAPED");
                assert!(owned.child.wait().unwrap().success());
                owned.reaped = true;
            }
            Ok(Some(ExitEvidence {
                owner: b.owner.clone(),
                lease_id: b.lease_id.clone(),
                run_id: b.run_id.clone(),
                connection_id: b.connection_id.clone(),
                confirmed_at: Utc::now(),
            }))
        }
    }
    #[tokio::test]
    async fn cancelling_owned_parent_reaps_its_descendant_and_keeps_other_member_alive() {
        let f = Fixture::new().await;
        f.queued().await;
        let (m, s) = fixture(&f.server, f._dir.path(), "other").await;
        f.queued_for(&m, &s).await;
        let port = Arc::new(Processes::default());
        let co = Coordinator::fixture(f.host.clone(), port.clone());
        let (ca, a) = co.dispatch_next().await.unwrap().unwrap();
        let (cb, b) = co.dispatch_next().await.unwrap().unwrap();
        co.request_cancel(&ca, &a).await.unwrap();
        co.accept_event(&ca, &a, 1, RuntimeEvent::CancelAcknowledged)
            .await
            .unwrap();
        assert!(f.detail(ca.task_id()).await["attempts"][0]["finishedAt"].is_null());
        assert!(co.confirm_exit(&ca, &a).await.unwrap());
        {
            let mut rows = port.rows.lock().unwrap();
            assert!(rows
                .get_mut(b.run_id())
                .unwrap()
                .child
                .try_wait()
                .unwrap()
                .is_none());
            assert!(!rows[b.run_id()].cancelled);
        }
        assert_eq!(f.detail(cb.task_id()).await["status"], "running");
        co.request_cancel(&cb, &b).await.unwrap();
        assert!(co.confirm_exit(&cb, &b).await.unwrap());
    }
}
pub(crate) struct ReleasedRuntime062;
#[async_trait::async_trait]
impl sea_orm_migration::MigratorTrait for ReleasedRuntime062 {
    fn migrations() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
        crate::db::migration::Migrator::migrations()
            .into_iter()
            .filter(|m| {
                let n = m.name();
                (n.starts_with('m') && n <= "m20261022_000001_tool_approval")
                    || (n.starts_with("onehall_") && n <= "onehall_m20261025_000004_team_runtime")
            })
            .collect()
    }
}
#[tokio::test]
async fn exact_queue_61_upgrade_preserves_history_ids_and_runtime_backup_survives_reopen() {
    use crate::{
        commands::backup::{core, sections::LiveRoots},
        hall::tasks::queue_tests::ReleasedQueue061,
        web::event_bridge::EventEmitter,
    };
    use sea_orm_migration::MigratorTrait;
    let dir = tempfile::tempdir().unwrap();
    let url = format!(
        "sqlite:{}?mode=rwc",
        dir.path().join("upgrade.db").display()
    );
    let db = sea_orm::Database::connect(&url).await.unwrap();
    ReleasedQueue061::up(&db, None).await.unwrap();
    assert_eq!(
        ReleasedQueue061::get_applied_migrations(&db)
            .await
            .unwrap()
            .len(),
        61
    );
    let server = server(crate::db::AppDatabase { conn: db.clone() }, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let t = value(post(&server, "hall_create_task", input(&m, &s)).await);
    let id = t["taskId"].as_str().unwrap();
    // Frozen schema-61 writer fixture: explicit original columns, no current
    // queue methods (which use schema-62 runtime safety).
    let attempt = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_task_attempt(attempt_id,submission_id,task_id,\
        member_id,snapshot_id,snapshot_version,attempt_number,status,\
        revision,created_at,updated_at) SELECT ?,?,task_id,member_id,\
        snapshot_id,snapshot_version,1,'queued',1,?,? FROM hall_task \
        WHERE task_id=?",
        vec![
            attempt.clone().into(),
            uuid::Uuid::new_v4().to_string().into(),
            now.into(),
            now.into(),
            id.to_owned().into(),
        ],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_task SET status='queued',revision=2 WHERE task_id=?",
        vec![id.to_owned().into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_task_event(event_id,task_id,attempt_id,\
        task_revision,action,created_at) VALUES(?,?,?,2,'task_queued',?)",
        vec![
            event_id.clone().into(),
            id.to_owned().into(),
            attempt.into(),
            now.into(),
        ],
    ))
    .await
    .unwrap();
    let before = serde_json::to_value(get_task(&db, id.into()).await.unwrap()).unwrap();
    ReleasedRuntime062::up(&db, None).await.unwrap();
    ReleasedRuntime062::up(&db, None).await.unwrap();
    assert_eq!(
        ReleasedRuntime062::get_applied_migrations(&db)
            .await
            .unwrap()
            .len(),
        62
    );
    assert_eq!(
        serde_json::to_value(get_task(&db, id.into()).await.unwrap()).unwrap(),
        before
    );
    let saved = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT event_id,source_key FROM hall_task_event",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.try_get::<String>("", "event_id").unwrap(), event_id);
    assert_eq!(
        saved.try_get::<String>("", "source_key").unwrap(),
        "host:task_queued"
    );
    assert!(ReleasedQueue061::get_pending_migrations(&db).await.is_err());
    let backup = core::create_backup_core(
        core::BackupInputs {
            conn: &db,
            data_dir: dir.path(),
            live_roots: LiveRoots::rooted_at(&dir.path().join("empty")),
            app_version: "0.39.0",
            runtime_label: "server",
        },
        core::BackupOptions::default(),
        &dir.path().join("runtime.zip"),
        &EventEmitter::Noop,
        "runtime-fixture",
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        backup.latest_migration,
        crate::db::migration::Migrator::migrations()
            .last()
            .unwrap()
            .name()
    );
    assert!(core::evaluate_compat(&backup).0);
    assert!(!ReleasedQueue061::migrations()
        .iter()
        .any(|m| m.name() == backup.latest_migration));
    drop(server);
    db.close().await.unwrap();
    let reopened = sea_orm::Database::connect(&url).await.unwrap();
    ReleasedRuntime062::up(&reopened, None).await.unwrap();
    assert_eq!(
        serde_json::to_value(get_task(&reopened, id.into()).await.unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn prepared_intent_survives_crash_without_sending_first_prompt() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    let old = f
        .host
        .claim_runtime(false, "fixture")
        .await
        .unwrap()
        .unwrap();
    let d = f.detail(&id).await;
    assert_eq!(d["runtimes"][0]["stage"], "prepared");
    assert!(d["attempts"][0]["startedAt"].is_null());
    let reboot = QueueHost::new(f.db.clone(), 1).unwrap();
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 1);
    let co = Coordinator::fixture(reboot, f.port.clone());
    assert!(co.dispatch_next().await.unwrap().is_none());
    assert!(!f.host.is_current(&old).await.unwrap());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn runtime_cursor_advances_without_fake_history_and_result_waits_for_exit() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    let (c, b) = f.co.dispatch_next().await.unwrap().unwrap();
    let rev = f.detail(&id).await["revision"].clone();
    f.co.accept_event(&c, &b, 20, RuntimeEvent::Observed)
        .await
        .unwrap();
    assert_eq!(f.detail(&id).await["revision"], rev);
    f.co.accept_event(&c, &b, 21, result(Outcome::Succeeded))
        .await
        .unwrap();
    f.co.accept_event(
        &c,
        &b,
        22,
        RuntimeEvent::WaitingInput {
            key: "late resolved".into(),
            waiting: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(f.detail(&id).await["runtimes"][0]["stage"], "finishing");
    f.port.exited.store(true, Ordering::SeqCst);
    assert!(f.co.confirm_exit(&c, &b).await.unwrap());
}
#[tokio::test]
async fn populated_runtime_schema_refuses_downgrade_without_discarding_evidence() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.co.dispatch_next().await.unwrap();
    assert!(sea_orm_migration::MigrationTrait::down(
        &super::migration::Migration,
        &sea_orm_migration::SchemaManager::new(&f.db)
    )
    .await
    .is_err());
    assert_eq!(f.detail(&id).await["runtimes"][0]["stage"], "running");
}

pub(crate) async fn start_fixture(host: QueueHost) -> (crate::hall::tasks::queue::Claim, Binding) {
    Coordinator::fixture(host, Arc::new(Fake::default()))
        .dispatch_next()
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn two_reviewed_members_complete_shared_workflow_without_opening_real_admission() {
    use crate::hall::{
        tasks::review,
        team::{mailbox, TeamService, ToolRequest},
    };
    let f = Fixture::new().await;
    let (m, s) = fixture(&f.server, f._dir.path(), "worker").await;
    let mut ids = Vec::new();
    for (member, snapshot) in [(&f.member, &f.snapshot), (&m, &s)] {
        let task = value(post(&f.server, "hall_create_task", input(member, snapshot)).await);
        let id = task["taskId"].as_str().unwrap();
        let plan = review::preview(&f.db, id).await.unwrap();
        assert!(!plan.real_execution_allowed);
        review::approve(
            &f.db,
            review::Approval {
                task_id: id.into(),
                expected_revision: 1,
                plan_hash: plan.plan_hash,
                submission_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .await
        .unwrap();
        ids.push(id.to_string());
    }
    let co = Coordinator::reviewed_fixture(f.host.clone(), f.port.clone());
    let (c1, b1) = co.dispatch_next().await.unwrap().unwrap();
    let (c2, b2) = co.dispatch_next().await.unwrap().unwrap();
    assert_eq!(c1.task_id(), ids[0]);
    assert_eq!(c2.task_id(), ids[1]);
    assert!(co.dispatch_next().await.unwrap().is_none());
    let registry = Arc::new(Registry::default());
    let pm = registry.fixture_actor(&f.db, &c1, &b1).await;
    let actor = registry.fixture_actor(&f.db, &c2, &b2).await;
    let pm_id = f.member["memberId"].as_str().unwrap();
    mailbox::bind_pm(&f.db, Some(pm_id.into()), 1)
        .await
        .unwrap();
    let team = TeamService::new(f.host.clone(), registry);
    let submission = uuid::Uuid::new_v4().to_string();
    let report = team
        .call(
            &actor,
            ToolRequest::Report {
                submission_id: submission.clone(),
                body: "fixture progress".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        team.call(
            &actor,
            ToolRequest::Report {
                submission_id: submission,
                body: "fixture progress".into()
            }
        )
        .await
        .unwrap(),
        report
    );
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 2);
    let inbox = team.call(&pm, ToolRequest::ReadMailbox {}).await.unwrap();
    assert_eq!(inbox.as_array().unwrap().len(), 1);
    team.call(
        &pm,
        ToolRequest::AckMessage {
            message_id: report["messageId"].as_str().unwrap().into(),
        },
    )
    .await
    .unwrap();
    co.accept_event(
        &c2,
        &b2,
        1,
        RuntimeEvent::WaitingInput {
            key: "fixture-permission".into(),
            waiting: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(f.detail(&ids[1]).await["status"], "awaiting_permission");
    co.request_cancel(&c2, &b2).await.unwrap();
    assert!(!co.confirm_exit(&c2, &b2).await.unwrap());
    assert!(f.detail(&ids[1]).await["attempts"][0]["finishedAt"].is_null());
    co.accept_event(&c1, &b1, 1, result(Outcome::Succeeded))
        .await
        .unwrap();
    f.port.exited.store(true, Ordering::SeqCst);
    assert!(co.confirm_exit(&c1, &b1).await.unwrap());
    assert_eq!(f.detail(&ids[0]).await["status"], "succeeded");
    assert!(co.confirm_exit(&c2, &b2).await.unwrap());
    assert_eq!(f.detail(&ids[1]).await["status"], "cancelled");
    assert!(team.call(&pm, ToolRequest::ListMembers {}).await.is_err());
    let again = sea_orm::Database::connect(format!(
        "sqlite:{}?mode=rwc",
        f._dir.path().join("source.db").display()
    ))
    .await
    .unwrap();
    assert_eq!(
        get_task(&again, ids[0].clone())
            .await
            .unwrap()
            .results
            .len(),
        1
    );
    assert!(mailbox::inbox(&again, pm_id).await.unwrap()[0]
        .acknowledged_at
        .is_some());
    let closed = Coordinator::production_closed(f.host.clone(), f.port.clone());
    assert!(closed.dispatch_next().await.is_err());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_start_without_binding_still_aborts_reserved_run() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.port.fail_start.store(true, Ordering::SeqCst);
    assert!(f.co.dispatch_next().await.is_err());
    assert_eq!(f.port.aborts.load(Ordering::SeqCst), 1);
    assert_eq!(f.detail(&id).await["status"], "interrupted");
}
#[tokio::test]
async fn activation_and_invalid_binding_use_the_same_abort_path() {
    for activation in [true, false] {
        let f = Fixture::new().await;
        f.queued().await;
        f.port.fail_activate.store(activation, Ordering::SeqCst);
        f.port.bad_binding.store(!activation, Ordering::SeqCst);
        assert!(f.co.dispatch_next().await.is_err());
        assert_eq!(f.port.aborts.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn binding_transaction_failure_still_aborts_the_started_run() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.db.execute_unprepared("CREATE TRIGGER fixture_binding_failure BEFORE INSERT ON hall_task_event WHEN NEW.action='attempt_running' BEGIN SELECT RAISE(FAIL,'fixture binding write failure'); END").await.unwrap();
    assert!(f.co.dispatch_next().await.is_err());
    assert_eq!(f.port.aborts.load(Ordering::SeqCst), 1);
    assert_eq!(f.detail(&id).await["status"], "interrupted");
}

#[tokio::test]
async fn reservation_failure_is_durably_not_started() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.port.fail_reserve.store(true, Ordering::SeqCst);
    assert!(f.co.dispatch_next().await.is_err());
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.detail(&id).await["startRecovery"][0]["failure"],
        "not_started"
    );
}

#[tokio::test]
async fn a_policy_refusal_happens_before_any_reservation_or_process() {
    let f = Fixture::new().await;
    let id = f.queued().await;
    f.port.windows.store(true, Ordering::SeqCst);
    assert!(f.co.dispatch_next().await.is_err());
    assert_eq!(f.port.reserves.load(Ordering::SeqCst), 0);
    assert_eq!(f.port.starts.load(Ordering::SeqCst), 0);
    assert_eq!(f.port.aborts.load(Ordering::SeqCst), 0);
    assert_eq!(f.detail(&id).await["startRecovery"][0]["failure"], "not_started");
}

#[tokio::test]
async fn revoked_run_loses_authority_but_remains_hall_restricted() {
    let f = Fixture::new().await;
    f.queued().await;
    let (claim, binding) = f.co.dispatch_next().await.unwrap().unwrap();
    let registry = Registry::default();
    let actor = registry.fixture_actor(&f.db, &claim, &binding).await;
    registry.revoke(binding.run_id());
    assert!(registry.own_task(&actor).is_err());
    assert!(registry.protects_run(binding.run_id()));
    assert!(registry.protects_connection(binding.connection_id()));
}

#[path = "../../exchange/wire_tests.rs"]
mod exchange_wire;

#[path = "../../team/tool_tests.rs"]
mod team_tools;
