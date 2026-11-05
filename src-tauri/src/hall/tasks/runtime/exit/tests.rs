use super::*;
use crate::hall::tasks::runtime::{
    Coordinator, ExecutionOwner, ExitEvidence, RunPlan, RuntimeEvent,
};
use crate::{app_error::AppCommandError, hall::tasks::runtime::port::RuntimePort};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
fn owner() -> ExecutionOwner {
    ExecutionOwner {
        member: uuid::Uuid::new_v4().to_string(),
        task: uuid::Uuid::new_v4().to_string(),
        attempt: uuid::Uuid::new_v4().to_string(),
        snapshot: uuid::Uuid::new_v4().to_string(),
        claim: uuid::Uuid::new_v4().to_string(),
        epoch: uuid::Uuid::new_v4().to_string(),
    }
}
fn root(domain: &Arc<Domain>) {
    let t = domain.begin(true).unwrap();
    let owned = domain.started(t, std::process::id());
    // Protocol-only fixture identity on unsupported platforms; never production.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        domain
            .state
            .lock()
            .unwrap()
            .records
            .get_mut(&owned.id)
            .unwrap()
            .birth = Some(process::Birth {
            pid: std::process::id(),
            start: 1,
            fraction: 0,
        });
    }
    let _ = owned;
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn real_start_identity_is_stable_but_not_a_pid_zero_or_text_assertion() {
    let a = process::birth(std::process::id()).unwrap();
    let b = process::birth(std::process::id()).unwrap();
    assert!(a == b);
    assert!(process::birth(u32::MAX).is_none());
}
#[test]
fn root_wait_alone_is_never_proof_for_native_descendants() {
    let d = Domain::new(owner(), Coverage::Native);
    root(&d);
    d.root_reaped();
    let o = d.observation();
    assert!(o.root_reaped);
    assert!(o.root_birth_hash.is_some());
    assert!(!o.complete);
    assert_eq!(o.reason, "descendant_coverage_unconfirmed");
}
#[test]
fn empty_domain_without_an_owned_root_never_certifies_exit() {
    let d = Domain::fixture(owner(), Arc::new(|| Some(true)));
    assert!(!d.observation().complete);
    d.root_reaped();
    assert!(!d.observation().complete);
}
#[test]
fn unknown_birth_or_unreaped_owned_child_keeps_the_slot() {
    let d = Domain::fixture(owner(), Arc::new(|| Some(true)));
    let t = d.begin(true).unwrap();
    d.started(t, u32::MAX);
    d.root_reaped();
    assert_eq!(d.observation().reason, "root_birth_unavailable");
    let d = Domain::fixture(owner(), Arc::new(|| Some(true)));
    root(&d);
    let t = d.begin(false).unwrap();
    let child = d.started(t, std::process::id());
    d.root_reaped();
    assert!(!d.observation().complete);
    child.reaped();
    assert!(d.observation().complete);
}
#[test]
fn close_fences_new_spawns_and_waits_for_already_owned_pending_spawn() {
    let d = Domain::fixture(owner(), Arc::new(|| Some(true)));
    root(&d);
    let pending = d.begin(false).unwrap();
    d.root_reaped();
    assert!(!d.observation().complete);
    assert!(d.begin(false).is_err());
    let late = d.started(pending, std::process::id());
    assert!(!d.observation().complete);
    late.reaped();
    assert!(d.observation().complete);
}
#[test]
fn repeated_root_and_cross_run_connection_owner_or_lease_are_rejected() {
    let book = Book::default();
    let own = owner();
    let d = book.prepare("run", own.clone()).unwrap();
    assert!(book.prepare("run", owner()).is_err());
    let t = d.begin(true).unwrap();
    assert!(d.begin(true).is_err());
    d.started(t, std::process::id());
    assert!(d.begin(true).is_err());
    book.for_spawn("run", "conn").unwrap();
    assert!(book.for_spawn("run", "other").is_err());
    let mut b = Binding {
        owner: own,
        lease_id: d.lease_id().into(),
        run_id: "run".into(),
        connection_id: "conn".into(),
        conversation_id: 1,
        external_session_id: "session".into(),
        started_at: Utc::now(),
    };
    assert!(book.bound(&b).is_some());
    b.owner.claim = uuid::Uuid::new_v4().to_string();
    assert!(book.bound(&b).is_none());
}
#[test]
fn a_regular_directory_cannot_masquerade_as_a_verified_kernel_domain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("run");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("cgroup.events"), "populated 0\n").unwrap();
    assert!(cgroup::Cgroup::open(&path, "run").is_err());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn actual_factory_tracks_multigeneration_detached_children_and_keeps_other_work_alive() {
    use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let middle = "import os,subprocess,sys,json\np=subprocess.Popen(['/bin/sleep',\
        '120'],start_new_session=True)\nprint(json.dumps([os.getpid(),\
        p.pid]),flush=True)\ntry:\n sys.stdin.readline()\nfinally:\n \
        p.terminate()\n p.wait()\n";
    let root_code = format!(
        "import subprocess,sys\np=subprocess.Popen([sys.executable,'-u',\
        '-c',{}],stdin=subprocess.PIPE,stdout=subprocess.PIPE,\
        text=True)\nprint(p.stdout.readline().strip(),\
        flush=True)\ntry:\n sys.stdin.readline()\nfinally:\n \
        p.stdin.write('stop\\n')\n p.stdin.flush()\n p.wait()\n",
        serde_json::to_string(middle).unwrap()
    );
    let inventory = Arc::new(Mutex::new(Vec::<process::Birth>::new()));
    let check = inventory.clone();
    let d = Domain::fixture(
        owner(),
        Arc::new(move || {
            let entries = check.lock().unwrap();
            Some(
                !entries.is_empty()
                    && entries
                        .iter()
                        .all(|old| process::birth(old.pid) != Some(*old)),
            )
        }),
    );
    let agent = crate::acp::agent_process::AcpAgent::new(McpServer::Stdio(
        McpServerStdio::new("owned-fixture", std::path::PathBuf::from("python3")).args(vec![
            "-u".into(),
            "-c".into(),
            root_code,
        ]),
    ))
    .with_process_domain(Some(d.clone()));
    let (mut input, output, _errors, mut child) = agent.spawn_process().unwrap();
    let mut reader = BufReader::new(output);
    let mut line = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(8),
        reader.read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    let pids: Vec<u32> = serde_json::from_str(&line).unwrap();
    assert_eq!(pids.len(), 2);
    let starts = pids
        .iter()
        .map(|pid| process::birth(*pid).unwrap())
        .collect::<Vec<_>>();
    *inventory.lock().unwrap() = starts.clone();
    let mut other = tokio::process::Command::new("/bin/sleep")
        .arg("120")
        .spawn()
        .unwrap();
    let mut old_background = tokio::process::Command::new("/bin/sleep")
        .arg("120")
        .spawn()
        .unwrap();
    let before = d.observation();
    input.write_all(b"stop\n").await.unwrap();
    let status = tokio::time::timeout(std::time::Duration::from_secs(8), child.wait())
        .await
        .unwrap()
        .unwrap();
    d.root_reaped();
    let after = d.observation();
    let unrelated_alive =
        other.try_wait().unwrap().is_none() && old_background.try_wait().unwrap().is_none();
    other.kill().await.unwrap();
    old_background.kill().await.unwrap();
    assert!(status.success());
    assert!(!before.complete);
    assert!(after.complete);
    assert!(unrelated_alive);
    assert!(starts
        .iter()
        .all(|old| process::birth(old.pid) != Some(*old)));
    assert_eq!(after.coverage, "fixture_owned_creation");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn actual_root_wait_and_late_owned_spawn_cannot_hide_a_live_child() {
    let d = Domain::fixture(owner(), Arc::new(|| Some(true)));
    let ticket = d.begin(true).unwrap();
    let mut root = tokio::process::Command::new("/bin/sleep")
        .arg("120")
        .spawn()
        .unwrap();
    d.started(ticket, root.id().unwrap());
    let pending = d.begin(false).unwrap();
    root.kill().await.unwrap();
    d.root_reaped();
    assert!(!d.observation().complete);
    assert!(d.begin(false).is_err());
    let mut late = tokio::process::Command::new("/bin/sleep")
        .arg("120")
        .spawn()
        .unwrap();
    let actual = process::birth(late.id().unwrap()).unwrap();
    let reused = process::Birth {
        start: actual.start + 1,
        ..actual
    };
    assert!(process::birth(actual.pid) != Some(reused));
    let owned = d.started(pending, late.id().unwrap());
    assert_eq!(d.observation().reason, "owned_child_not_reaped");
    assert!(late.try_wait().unwrap().is_none());
    late.kill().await.unwrap();
    owned.reaped();
    assert!(d.observation().complete);
}

struct Port {
    domains: Mutex<BTreeMap<String, Arc<Domain>>>,
    closed: Arc<AtomicBool>,
    native: bool,
    checks: AtomicUsize,
    wrong: bool,
}
#[async_trait::async_trait]
impl RuntimePort for Port {
    async fn reserve(&self) -> Result<String, AppCommandError> {
        Ok(uuid::Uuid::new_v4().to_string())
    }
    async fn start(&self, run: &str, p: &RunPlan) -> Result<Binding, AppCommandError> {
        let d = if self.native {
            Domain::new(p.owner.clone(), Coverage::Native)
        } else {
            let closed = self.closed.clone();
            Domain::fixture(
                p.owner.clone(),
                Arc::new(move || Some(closed.load(Ordering::SeqCst))),
            )
        };
        root(&d);
        let lease_id = d.lease_id().into();
        self.domains.lock().unwrap().insert(run.into(), d);
        Ok(Binding {
            owner: p.owner.clone(),
            lease_id,
            run_id: run.into(),
            connection_id: uuid::Uuid::new_v4().to_string(),
            conversation_id: 1,
            external_session_id: uuid::Uuid::new_v4().to_string(),
            started_at: Utc::now(),
        })
    }
    async fn cancel(&self, _: &Binding) -> Result<(), AppCommandError> {
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
        self.checks.fetch_add(1, Ordering::SeqCst);
        let domains = self.domains.lock().unwrap();
        let d = &domains[&b.run_id];
        d.root_reaped();
        let o = d.observation();
        let mut owner = b.owner.clone();
        if self.wrong {
            owner.claim = uuid::Uuid::new_v4().to_string();
        }
        Ok(o.complete.then(|| ExitEvidence {
            owner,
            lease_id: b.lease_id.clone(),
            run_id: b.run_id.clone(),
            connection_id: b.connection_id.clone(),
            confirmed_at: o.observed_at,
        }))
    }
    async fn exit_observation(&self, b: &Binding) -> Option<ExitObservation> {
        self.domains
            .lock()
            .unwrap()
            .get(&b.run_id)
            .map(|d| d.observation())
    }
}
async fn fixture(
    native: bool,
    wrong: bool,
) -> (
    tempfile::TempDir,
    sea_orm::DatabaseConnection,
    Coordinator,
    Arc<Port>,
    String,
) {
    use crate::{
        db::test_helpers::fresh_disk_db,
        hall::{
            members_tests::server,
            tasks::queue::QueueHost,
            tasks_tests::{fixture, input, post, value},
        },
    };
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let id = value(post(&server, "hall_create_task", input(&m, &s)).await)["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    let host = QueueHost::new(conn.clone(), 1).unwrap();
    host.enqueue(&id, 1, &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    let port = Arc::new(Port {
        domains: Mutex::new(BTreeMap::new()),
        closed: Arc::new(AtomicBool::new(false)),
        native,
        checks: AtomicUsize::new(0),
        wrong,
    });
    let co = Coordinator::fixture(host, port.clone());
    (dir, conn, co, port, id)
}
#[tokio::test]
async fn coordinator_checks_phase_before_any_exit_side_effect_and_persists_observations() {
    let (_dir, db, co, port, id) = fixture(false, false).await;
    let (c, b) = co.dispatch_next().await.unwrap().unwrap();
    assert!(co.confirm_exit(&c, &b).await.is_err());
    assert_eq!(port.checks.load(Ordering::SeqCst), 0);
    co.request_cancel(&c, &b).await.unwrap();
    assert!(!co.confirm_exit(&c, &b).await.unwrap());
    let before = crate::hall::tasks::get_task(&db, id.clone()).await.unwrap();
    assert_eq!(before.exit_checks.len(), 1);
    assert!(!before.exit_checks[0].complete);
    assert!(before.attempts[0].finished_at.is_none());
    port.closed.store(true, Ordering::SeqCst);
    assert!(co.confirm_exit(&c, &b).await.unwrap());
    let after = crate::hall::tasks::get_task(&db, id.clone()).await.unwrap();
    assert_eq!(after.task.status, "cancelled");
    assert!(after.exit_checks[0].complete);
    assert!(after.attempts[0].finished_at.is_some());
    let again = sea_orm::Database::connect(format!(
        "sqlite:{}?mode=rwc",
        _dir.path().join("source.db").display()
    ))
    .await
    .unwrap();
    assert!(
        crate::hall::tasks::get_task(&again, id)
            .await
            .unwrap()
            .exit_checks[0]
            .complete
    );
}
#[tokio::test]
async fn native_incomplete_exit_is_review_required_and_does_not_free_capacity() {
    let (_dir, db, co, _port, id) = fixture(true, false).await;
    let (c, b) = co.dispatch_next().await.unwrap().unwrap();
    co.request_cancel(&c, &b).await.unwrap();
    assert!(!co.confirm_exit(&c, &b).await.unwrap());
    let detail = crate::hall::tasks::get_task(&db, id).await.unwrap();
    assert_eq!(detail.task.status, "interrupted");
    assert!(detail.attempts[0].finished_at.is_none());
    assert_eq!(detail.exit_checks[0].coverage, "native_observation_only");
    assert!(!detail.exit_checks[0].complete);
}
#[tokio::test]
async fn old_claim_owner_cannot_turn_a_valid_scope_exit_into_current_completion() {
    let (_dir, db, co, port, id) = fixture(false, true).await;
    let (c, b) = co.dispatch_next().await.unwrap().unwrap();
    co.request_cancel(&c, &b).await.unwrap();
    port.closed.store(true, Ordering::SeqCst);
    assert!(co.confirm_exit(&c, &b).await.is_err());
    assert!(crate::hall::tasks::get_task(&db, id)
        .await
        .unwrap()
        .attempts[0]
        .finished_at
        .is_none());
}

#[tokio::test]
async fn exact_mailbox_63_upgrade_preserves_role_mail_and_new_exit_history_backup_gate() {
    use crate::{
        commands::backup::{core, sections::LiveRoots},
        db::{migration::Migrator, AppDatabase},
        hall::{
            members_tests::server,
            tasks_tests::{fixture, input, post, value},
            team::{mailbox, tests::ReleasedMailbox063},
        },
        web::event_bridge::EventEmitter,
    };
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;
    let dir = tempfile::tempdir().unwrap();
    let url = format!(
        "sqlite:{}?mode=rwc",
        dir.path().join("upgrade.db").display()
    );
    let db = sea_orm::Database::connect(&url).await.unwrap();
    ReleasedMailbox063::up(&db, None).await.unwrap();
    assert_eq!(
        ReleasedMailbox063::get_applied_migrations(&db)
            .await
            .unwrap()
            .len(),
        63
    );
    let server = server(AppDatabase { conn: db.clone() }, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let id = value(post(&server, "hall_create_task", input(&m, &s)).await)["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    let member = m["memberId"].as_str().unwrap();
    mailbox::bind_pm(&db, Some(member.into()), 1).await.unwrap();
    let msg = mailbox::send_as_user(
        &db,
        mailbox::MessageDraft {
            submission_id: uuid::Uuid::new_v4().to_string(),
            recipient_member_id: member.into(),
            body: "migration fixture".into(),
        },
    )
    .await
    .unwrap();
    crate::hall::tasks::review::tests::ReleasedExit064::up(&db, None)
        .await
        .unwrap();
    crate::hall::tasks::review::tests::ReleasedExit064::up(&db, None)
        .await
        .unwrap();
    assert_eq!(
        crate::hall::tasks::review::tests::ReleasedExit064::get_applied_migrations(&db)
            .await
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        mailbox::inbox(&db, member).await.unwrap()[0].message_id,
        msg.message_id
    );
    assert_eq!(
        mailbox::role(&db).await.unwrap().member_id.as_deref(),
        Some(member)
    );
    assert!(crate::hall::tasks::get_task(&db, id.clone())
        .await
        .unwrap()
        .exit_checks
        .is_empty());
    assert!(ReleasedMailbox063::get_pending_migrations(&db)
        .await
        .is_err());
    let backup = core::create_backup_core(
        core::BackupInputs {
            conn: &db,
            data_dir: dir.path(),
            live_roots: LiveRoots::rooted_at(&dir.path().join("empty")),
            app_version: "0.39.0",
            runtime_label: "server",
        },
        core::BackupOptions::default(),
        &dir.path().join("exit.zip"),
        &EventEmitter::Noop,
        "exit-fixture",
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        backup.latest_migration,
        Migrator::migrations().last().unwrap().name()
    );
    assert!(!ReleasedMailbox063::migrations()
        .iter()
        .any(|m| m.name() == backup.latest_migration));
    assert!(core::evaluate_compat(&backup).0);
    assert!(db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA foreign_key_check"
        ))
        .await
        .unwrap()
        .is_empty());
    drop(server);
    db.close().await.unwrap();
    let again = sea_orm::Database::connect(&url).await.unwrap();
    Migrator::up(&again, None).await.unwrap();
    assert_eq!(
        mailbox::inbox(&again, member).await.unwrap()[0].message_id,
        msg.message_id
    );
    assert!(crate::hall::tasks::get_task(&again, id)
        .await
        .unwrap()
        .exit_checks
        .is_empty());
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test]
async fn native_probe_uses_real_acp_factory_and_reports_platform_guarantee() {
    let root = host::RunRoot::detect().unwrap();
    let nonce = uuid::Uuid::new_v4().to_string();
    let path = root.create(&nonce).unwrap();
    let proof = probe::verify(&path, &nonce).await.unwrap();
    root.remove(&nonce);
    assert!(proof.valid());
    let report = proof.report();
    assert!(report.mechanism_verified && report.unrelated_alive);
    assert_eq!(report.complete, cfg!(windows));
    if cfg!(target_os = "macos") {
        assert_eq!(report.coverage, "macos_process_group_best_effort");
    } else {
        assert_eq!(report.coverage, "windows_job_object");
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn human_resolution_releases_capacity_once_without_fabricating_descendant_exit() {
    use crate::hall::tasks::{create_task, get_task, CreateTaskDraft, TaskPurpose};
    let (_dir, db, co, port, id) = fixture(true, false).await;
    let (claim, binding) = co.dispatch_next().await.unwrap().unwrap();
    {
        let mut domains = port.domains.lock().unwrap();
        let domain = Arc::get_mut(domains.get_mut(&binding.run_id).unwrap()).unwrap();
        let nonce = uuid::Uuid::new_v4().to_string();
        domain.coverage = Coverage::Managed(Arc::new(
            platform::Group::open(std::path::Path::new(&nonce), &nonce).unwrap(),
        ));
    }
    let before = get_task(&db, id.clone()).await.unwrap();
    assert!(co
        .resolve_best_effort(&claim, &binding, before.task.revision)
        .await
        .is_err());
    co.accept_event(
        &claim,
        &binding,
        1,
        RuntimeEvent::Result {
            outcome: super::super::Outcome::Succeeded,
            body: "completed work, uncertain descendants".into(),
        },
    )
    .await
    .unwrap();
    assert!(!co.confirm_exit(&claim, &binding).await.unwrap());
    let paused = get_task(&db, id.clone()).await.unwrap();
    assert_eq!(paused.task.status, "interrupted");
    let next = create_task(
        &db,
        CreateTaskDraft {
            submission_id: uuid::Uuid::new_v4().to_string(),
            member_id: binding.owner.member.clone(),
            snapshot_id: binding.owner.snapshot.clone(),
            project_id: None,
            purpose: TaskPurpose::Work,
            title: "next fixture".into(),
            description: "next independent work".into(),
        },
    )
    .await
    .unwrap();
    co.host
        .enqueue(&next.task_id, 1, &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert!(co.host.claim_next().await.unwrap().is_none());
    assert!(co
        .resolve_best_effort(&claim, &binding, paused.task.revision - 1)
        .await
        .is_err());
    // Non-quiescent observations must not be administratively released.
    port.domains
        .lock()
        .unwrap()
        .get(&binding.run_id)
        .unwrap()
        .state
        .lock()
        .unwrap()
        .pending = 1;
    assert!(co
        .resolve_best_effort(&claim, &binding, paused.task.revision)
        .await
        .is_err());
    port.domains
        .lock()
        .unwrap()
        .get(&binding.run_id)
        .unwrap()
        .state
        .lock()
        .unwrap()
        .pending = 0;
    let forged = Binding {
        owner: owner(),
        lease_id: binding.lease_id.clone(),
        run_id: binding.run_id.clone(),
        connection_id: binding.connection_id.clone(),
        conversation_id: binding.conversation_id,
        external_session_id: binding.external_session_id.clone(),
        started_at: binding.started_at,
    };
    assert!(co
        .resolve_best_effort(&claim, &forged, paused.task.revision)
        .await
        .is_err());
    co.resolve_best_effort(&claim, &binding, paused.task.revision)
        .await
        .unwrap();
    assert!(co
        .resolve_best_effort(&claim, &binding, paused.task.revision)
        .await
        .is_err());
    let after = get_task(&db, id).await.unwrap();
    assert_eq!(after.task.status, "interrupted");
    assert_eq!(after.results[0].outcome, "succeeded");
    assert!(!after.exit_checks[0].complete);
    assert!(after.runtimes[0].exit_confirmed_at.is_none());
    assert!(after.attempts[0].finished_at.is_none());
    assert_eq!(
        after
            .history
            .iter()
            .filter(|e| e.action == "human_best_effort_resolution")
            .count(),
        1
    );
    assert_eq!(
        co.host.claim_next().await.unwrap().unwrap().task_id,
        next.task_id
    );
}
