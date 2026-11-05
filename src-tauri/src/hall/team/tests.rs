use super::mailbox::*;
use super::*;
use crate::{
    db::{test_helpers::fresh_disk_db, AppDatabase},
    hall::{
        members_tests::server,
        tasks::runtime::tests::start_fixture,
        tasks_tests::{fixture, input, post, value},
    },
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{json, Value};
pub(crate) struct ReleasedMailbox063;
#[async_trait::async_trait]
impl sea_orm_migration::MigratorTrait for ReleasedMailbox063 {
    fn migrations() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
        crate::db::migration::Migrator::migrations()
            .into_iter()
            .filter(|m| {
                let n = m.name();
                (n.starts_with('m') && n <= "m20261022_000001_tool_approval")
                    || (n.starts_with("onehall_") && n <= "onehall_m20261025_000005_team_mailbox")
            })
            .collect()
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    db: DatabaseConnection,
    server: axum_test::TestServer,
    host: QueueHost,
    registry: Arc<Registry>,
    service: TeamService,
    pm: Value,
    member: Value,
    member_snapshot: Value,
    pm_actor: Actor,
    actor: Actor,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await;
        let conn = db.conn.clone();
        let server = server(db, dir.path());
        let (pm, ps) = fixture(&server, dir.path(), "pm").await;
        let (m, ms) = fixture(&server, dir.path(), "worker").await;
        let host = QueueHost::new(conn.clone(), 2).unwrap();
        let registry = Arc::new(Registry::default());
        let mut actors = Vec::new();
        for (member, snapshot) in [(&pm, &ps), (&m, &ms)] {
            let task = value(post(&server, "hall_create_task", input(member, snapshot)).await);
            host.enqueue(
                task["taskId"].as_str().unwrap(),
                1,
                &uuid::Uuid::new_v4().to_string(),
            )
            .await
            .unwrap();
            let (c, b) = start_fixture(host.clone()).await;
            actors.push(registry.fixture_actor(&conn, &c, &b).await);
        }
        let actor = actors.pop().unwrap();
        let pm_actor = actors.pop().unwrap();
        bind_pm(&conn, Some(pm["memberId"].as_str().unwrap().into()), 1)
            .await
            .unwrap();
        let service = TeamService::new(host.clone(), registry.clone());
        Self {
            _dir: dir,
            db: conn,
            server,
            host,
            registry,
            service,
            pm,
            member: m,
            member_snapshot: ms,
            pm_actor,
            actor,
        }
    }
    fn member_id(&self) -> &str {
        self.member["memberId"].as_str().unwrap()
    }
    fn pm_id(&self) -> &str {
        self.pm["memberId"].as_str().unwrap()
    }
}
#[tokio::test]
async fn roles_are_stable_host_bindings_not_names_and_revocation_rejects_existing_actor() {
    let f = Fixture::new().await;
    assert!(f
        .service
        .call(&f.actor, ToolRequest::ListMembers {})
        .await
        .is_err());
    let catalogue = f
        .service
        .call(&f.pm_actor, ToolRequest::ListMembers {})
        .await
        .unwrap();
    assert_eq!(catalogue.as_array().unwrap().len(), 2);
    assert!(catalogue
        .as_array()
        .unwrap()
        .iter()
        .all(|row| !row["savedSnapshots"].as_array().unwrap().is_empty()));
    f.db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_member SET name='总管',role='PM' WHERE member_id=?",
        vec![f.member_id().into()],
    ))
    .await
    .unwrap();
    assert!(f
        .service
        .call(&f.actor, ToolRequest::ListMembers {})
        .await
        .is_err());
    bind_pm(&f.db, None, 2).await.unwrap();
    assert!(f
        .service
        .call(&f.pm_actor, ToolRequest::ListMembers {})
        .await
        .is_err());
    assert!(bind_pm(&f.db, Some(f.pm_id().into()), 2).await.is_err());
    assert_eq!(role(&f.db).await.unwrap().revision, 3);
}
#[tokio::test]
async fn reports_are_durable_idempotent_queued_and_never_advance_task_or_wake_runtime() {
    let f = Fixture::new().await;
    let submission = uuid::Uuid::new_v4().to_string();
    let before = f
        .service
        .call(&f.actor, ToolRequest::OwnTask {})
        .await
        .unwrap();
    let report = || ToolRequest::Report {
        submission_id: submission.clone(),
        body: "work report fixture".into(),
    };
    let (a, b) = tokio::join!(
        f.service.call(&f.actor, report()),
        f.service.call(&f.actor, report())
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a["recipientMemberId"], f.pm_id());
    assert_eq!(a["senderMemberId"], f.member_id());
    // Readers see current names next to the IDs.
    assert_eq!(a["senderName"], member_name(&f.db, f.member_id()).await);
    assert_eq!(a["recipientName"], member_name(&f.db, f.pm_id()).await);
    assert_eq!(a["taskId"], before["taskId"]);
    assert_eq!(a["attemptId"], before["attempts"][0]["attemptId"]);
    assert!(f
        .service
        .call(
            &f.actor,
            ToolRequest::Report {
                submission_id: submission,
                body: "changed report".into()
            }
        )
        .await
        .is_err());
    // The refused rewrite never reached the PM, so the task now shows it.
    let mut after = f
        .service
        .call(&f.actor, ToolRequest::OwnTask {})
        .await
        .unwrap();
    let issues = after["issues"].take();
    assert_eq!(issues.as_array().unwrap().len(), 1);
    assert_eq!(issues[0]["tool"], "hall_report");
    after["issues"] = before["issues"].clone();
    assert_eq!(before, after);
    let pm_inbox = f
        .service
        .call(&f.pm_actor, ToolRequest::ReadMailbox {})
        .await
        .unwrap();
    assert_eq!(pm_inbox.as_array().unwrap().len(), 1);
    assert_eq!(pm_inbox[0]["senderName"], a["senderName"]);
    assert!(pm_inbox[0]["acknowledgedAt"].is_null());
    assert!(f
        .service
        .call(&f.actor, ToolRequest::ReadMailbox {})
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let reopened = Database::connect(format!(
        "sqlite:{}?mode=rwc",
        f._dir.path().join("source.db").display()
    ))
    .await
    .unwrap();
    assert_eq!(
        inbox(&reopened, f.pm_id()).await.unwrap()[0].body,
        "work report fixture"
    );
}
#[tokio::test]
async fn model_chosen_report_keys_are_accepted_and_stay_idempotent() {
    // A real Codex member sent a readable key instead of a UUID; the report
    // must still land once, and retries with the same key must not duplicate.
    let f = Fixture::new().await;
    let report = |key: &str| ToolRequest::Report {
        submission_id: key.into(),
        body: "readable key report".into(),
    };
    let first = f
        .service
        .call(&f.actor, report("member-a-notes-hello-20261026"))
        .await
        .unwrap();
    let retry = f
        .service
        .call(&f.actor, report("member-a-notes-hello-20261026"))
        .await
        .unwrap();
    assert_eq!(first, retry);
    let inbox = f
        .service
        .call(&f.pm_actor, ToolRequest::ReadMailbox {})
        .await
        .unwrap();
    assert_eq!(inbox.as_array().unwrap().len(), 1);
    let blank = f.service.call(&f.actor, report("  ")).await.unwrap_err();
    assert!(matches!(
        blank.code,
        crate::app_error::AppErrorCode::InvalidInput
    ));
}
#[tokio::test]
async fn inbox_read_does_not_consume_and_acknowledgment_is_recipient_scoped_and_idempotent() {
    let f = Fixture::new().await;
    let message = send_as_user(
        &f.db,
        MessageDraft {
            submission_id: uuid::Uuid::new_v4().to_string(),
            recipient_member_id: f.member_id().into(),
            body: "human message".into(),
        },
    )
    .await
    .unwrap();
    let first = f
        .service
        .call(&f.actor, ToolRequest::ReadMailbox {})
        .await
        .unwrap();
    assert_eq!(first[0]["messageId"], message.message_id);
    assert!(first[0]["acknowledgedAt"].is_null());
    assert!(first[0]["senderMemberId"].is_null());
    assert_eq!(
        first,
        f.service
            .call(&f.actor, ToolRequest::ReadMailbox {})
            .await
            .unwrap()
    );
    assert!(f
        .service
        .call(
            &f.pm_actor,
            ToolRequest::AckMessage {
                message_id: message.message_id.clone()
            }
        )
        .await
        .is_err());
    let ack = || ToolRequest::AckMessage {
        message_id: message.message_id.clone(),
    };
    f.service.call(&f.actor, ack()).await.unwrap();
    let saved = inbox(&f.db, f.member_id()).await.unwrap()[0].acknowledged_at;
    f.service.call(&f.actor, ack()).await.unwrap();
    assert_eq!(
        inbox(&f.db, f.member_id()).await.unwrap()[0].acknowledged_at,
        saved
    );
}
#[tokio::test]
async fn pm_assignment_uses_existing_queue_and_replay_does_not_duplicate_attempts_or_start() {
    let f = Fixture::new().await;
    let draft = input(&f.member, &f.member_snapshot)["draft"].clone();
    let req = || {
        serde_json::from_value::<ToolRequest>(json!({"action":"hall_assign","draft":draft}))
            .unwrap()
    };
    assert!(f.service.call(&f.actor, req()).await.is_err());
    let a = f.service.call(&f.pm_actor, req()).await.unwrap();
    let b = f.service.call(&f.pm_actor, req()).await.unwrap();
    assert_eq!(a, b);
    let task = f
        .service
        .call(
            &f.pm_actor,
            ToolRequest::TaskStatus {
                task_id: a["taskId"].as_str().unwrap().into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(task["status"], "queued");
    assert_eq!(
        task["memberName"],
        member_name(&f.db, task["memberId"].as_str().unwrap()).await
    );
    assert_eq!(task["attempts"].as_array().unwrap().len(), 1);
    assert!(task["attempts"][0]["startedAt"].is_null());
    assert_eq!(task["launchAllowed"], false);
    assert_eq!(task["runtimeVerified"], false);
    assert!(f.host.claim_next().await.unwrap().is_none());
}
#[tokio::test]
async fn members_cannot_choose_sender_task_attempt_or_receive_another_members_mail() {
    let f = Fixture::new().await;
    for forged in [
        json!({"action":"hall_own_task","task_id":uuid::Uuid::new_v4().to_string()}),
        json!({"action":"hall_read_mailbox","member_id":f.pm_id()}),
        json!({"action":"hall_report","submission_id":uuid::Uuid::new_v4().to_string(),"body":"x","sender":"pm"}),
        json!({"action":"hall_report","submission_id":uuid::Uuid::new_v4().to_string(),"body":"x","attempt_id":"forged"}),
        json!({"action":"hall_list_members","role":"pm","depth":0}),
    ] {
        assert!(serde_json::from_value::<ToolRequest>(forged).is_err());
    }
    let own = f
        .service
        .call(&f.pm_actor, ToolRequest::OwnTask {})
        .await
        .unwrap();
    assert!(f
        .service
        .call(
            &f.actor,
            ToolRequest::TaskStatus {
                task_id: own["taskId"].as_str().unwrap().into()
            }
        )
        .await
        .is_err());
    assert!(f
        .service
        .call(
            &f.actor,
            ToolRequest::SendMessage {
                draft: MessageDraft {
                    submission_id: uuid::Uuid::new_v4().to_string(),
                    recipient_member_id: f.pm_id().into(),
                    body: "x".into()
                }
            }
        )
        .await
        .is_err());
}
#[tokio::test]
async fn recovery_and_revoked_connection_stop_tool_calls_without_losing_queued_reports() {
    let f = Fixture::new().await;
    f.service
        .call(
            &f.actor,
            ToolRequest::Report {
                submission_id: uuid::Uuid::new_v4().to_string(),
                body: "before recovery".into(),
            },
        )
        .await
        .unwrap();
    let reboot = QueueHost::new(f.db.clone(), 2).unwrap();
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 2);
    assert!(f
        .service
        .call(&f.actor, ToolRequest::ReadMailbox {})
        .await
        .is_err());
    assert!(f
        .service
        .call(&f.pm_actor, ToolRequest::ListMembers {})
        .await
        .is_err());
    assert_eq!(inbox(&f.db, f.pm_id()).await.unwrap().len(), 1);
    assert!(f.registry.own_task(&f.actor).is_ok()); // DB recovery, not identity labels, fenced this actor.
}
#[tokio::test]
async fn mailbox_rejects_invalid_or_conflicting_messages_without_exposing_storage_body() {
    let f = Fixture::new().await;
    for body in ["".into(), "x".repeat(8001), "control\0fixture".into()] {
        assert!(send_as_user(
            &f.db,
            MessageDraft {
                submission_id: uuid::Uuid::new_v4().to_string(),
                recipient_member_id: f.member_id().into(),
                body
            }
        )
        .await
        .is_err());
    }
    let submission = uuid::Uuid::new_v4().to_string();
    send_as_user(
        &f.db,
        MessageDraft {
            submission_id: submission.clone(),
            recipient_member_id: f.member_id().into(),
            body: "x".into(),
        },
    )
    .await
    .unwrap();
    assert!(send_as_user(
        &f.db,
        MessageDraft {
            submission_id: submission,
            recipient_member_id: f.pm_id().into(),
            body: "x".into()
        }
    )
    .await
    .is_err());
    assert_eq!(inbox(&f.db, f.member_id()).await.unwrap().len(), 1);
    assert!(inbox(&f.db, f.pm_id()).await.unwrap().is_empty());
}
#[tokio::test]
async fn exact_runtime_62_upgrades_to_mailbox_63_with_new_backup_gate() {
    use crate::{
        commands::backup::{core, sections::LiveRoots},
        hall::tasks::runtime::tests::ReleasedRuntime062,
        web::event_bridge::EventEmitter,
    };
    use sea_orm_migration::MigratorTrait;
    let dir = tempfile::tempdir().unwrap();
    let url = format!(
        "sqlite:{}?mode=rwc",
        dir.path().join("upgrade.db").display()
    );
    let db = Database::connect(&url).await.unwrap();
    ReleasedRuntime062::up(&db, None).await.unwrap();
    assert_eq!(
        ReleasedRuntime062::get_applied_migrations(&db)
            .await
            .unwrap()
            .len(),
        62
    );
    let s = server(AppDatabase { conn: db.clone() }, dir.path());
    let (m, snapshot) = fixture(&s, dir.path(), "member").await;
    let t = value(post(&s, "hall_create_task", input(&m, &snapshot)).await);
    ReleasedMailbox063::up(&db, None).await.unwrap();
    ReleasedMailbox063::up(&db, None).await.unwrap();
    assert_eq!(
        ReleasedMailbox063::get_applied_migrations(&db)
            .await
            .unwrap()
            .len(),
        63
    );
    assert_eq!(
        value(post(&s, "hall_get_task", json!({"taskId":t["taskId"]})).await)["title"],
        t["title"]
    );
    assert!(role(&db).await.unwrap().member_id.is_none());
    let msg = send_as_user(
        &db,
        MessageDraft {
            submission_id: uuid::Uuid::new_v4().to_string(),
            recipient_member_id: m["memberId"].as_str().unwrap().into(),
            body: "durable migration fixture".into(),
        },
    )
    .await
    .unwrap();
    let manifest = core::create_backup_core(
        core::BackupInputs {
            conn: &db,
            data_dir: dir.path(),
            live_roots: LiveRoots::rooted_at(&dir.path().join("empty")),
            app_version: "0.39.0",
            runtime_label: "server",
        },
        core::BackupOptions::default(),
        &dir.path().join("mailbox.zip"),
        &EventEmitter::Noop,
        "mailbox-fixture",
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        manifest.latest_migration,
        crate::db::migration::Migrator::migrations()
            .last()
            .unwrap()
            .name()
    );
    assert!(core::evaluate_compat(&manifest).0);
    assert!(!ReleasedRuntime062::migrations()
        .iter()
        .any(|m| m.name() == manifest.latest_migration));
    assert!(ReleasedRuntime062::get_pending_migrations(&db)
        .await
        .is_err());
    assert!(ReleasedMailbox063::down(&db, Some(1)).await.is_err());
    drop(s);
    db.close().await.unwrap();
    let reopened = Database::connect(&url).await.unwrap();
    ReleasedMailbox063::up(&reopened, None).await.unwrap();
    assert_eq!(
        inbox(&reopened, m["memberId"].as_str().unwrap())
            .await
            .unwrap()[0]
            .message_id,
        msg.message_id
    );
}
#[tokio::test]
async fn application_mailbox_routes_require_user_auth_and_use_the_same_persistent_services() {
    let f = Fixture::new().await;
    for command in [
        "hall_get_pm_binding",
        "hall_bind_pm",
        "hall_read_mailbox",
        "hall_ack_message",
        "hall_send_message",
    ] {
        assert_eq!(
            f.server
                .post(&format!("/api/{command}"))
                .json(&json!({}))
                .await
                .status_code(),
            401
        );
    }
    assert_eq!(
        value(post(&f.server, "hall_get_pm_binding", json!({})).await)["memberId"],
        f.pm_id()
    );
    assert!(post(
        &f.server,
        "hall_bind_pm",
        json!({"memberId":f.member_id(),"expectedRevision":2,"role":"forged"})
    )
    .await
    .status_code()
    .is_client_error());
    let changed = value(
        post(
            &f.server,
            "hall_bind_pm",
            json!({"memberId":f.member_id(),"expectedRevision":2}),
        )
        .await,
    );
    assert_eq!(changed["memberId"], f.member_id());
    assert_eq!(changed["revision"], 3);
    assert!(f
        .service
        .call(&f.pm_actor, ToolRequest::ListMembers {})
        .await
        .is_err());
    assert!(f
        .service
        .call(&f.actor, ToolRequest::ListMembers {})
        .await
        .is_ok());
    let sent=value(post(&f.server,"hall_send_message",json!({"draft":{"submissionId":uuid::Uuid::new_v4().to_string(),"recipientMemberId":f.member_id(),"body":"user fixture"}})).await);
    assert!(sent["senderMemberId"].is_null());
    let from_tool = f
        .service
        .call(&f.actor, ToolRequest::ReadMailbox {})
        .await
        .unwrap();
    assert_eq!(from_tool[0]["messageId"], sent["messageId"]);
    value(
        post(
            &f.server,
            "hall_ack_message",
            json!({"memberId":f.member_id(),"messageId":sent["messageId"]}),
        )
        .await,
    );
    assert!(!f
        .service
        .call(&f.actor, ToolRequest::ReadMailbox {})
        .await
        .unwrap()[0]["acknowledgedAt"]
        .is_null());
}

pub(crate) async fn broker_fixture() -> (
    TeamService,
    Actor,
    DatabaseConnection,
    String,
    tempfile::TempDir,
) {
    let Fixture {
        service,
        actor,
        db,
        pm,
        _dir,
        ..
    } = Fixture::new().await;
    (
        service,
        actor,
        db,
        pm["memberId"].as_str().unwrap().into(),
        _dir,
    )
}
#[test]
fn only_hall_tools_on_the_injected_codeg_mcp_skip_the_card() {
    use crate::hall::team::mcp::auto_allowed;
    use serde_json::json;
    for tool in [
        "hall_own_task",
        "hall_read_mailbox",
        "hall_ack_message",
        "hall_report",
        "hall_list_members",
        "hall_assign",
        "hall_task_status",
        "hall_send_message",
    ] {
        assert!(auto_allowed(
            &json!({"server":"codeg-mcp","tool":tool,"arguments":{}})
        ));
    }
    // Other codeg-mcp tools (delegation, questions) and other servers still
    // wait for a person; so does anything without a server / tool name.
    assert!(!auto_allowed(
        &json!({"server":"codeg-mcp","tool":"delegate_to_agent"})
    ));
    assert!(!auto_allowed(
        &json!({"server":"other","tool":"hall_report"})
    ));
    assert!(!auto_allowed(&json!({"command":"rm -rf /"})));
    assert!(!auto_allowed(&json!("hall_report")));
}
async fn member_name(db: &DatabaseConnection, member: &str) -> Value {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT name FROM hall_member WHERE member_id=?",
            vec![member.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let name: String = row.try_get("", "name").unwrap();
    assert!(!name.is_empty());
    Value::String(name)
}
#[tokio::test]
async fn a_failed_report_stays_on_the_task_until_a_retry_gets_through() {
    let f = Fixture::new().await;
    let (_, task, attempt) = f.registry.own_task(&f.actor).unwrap();
    let report = |body: &str| ToolRequest::Report {
        submission_id: uuid::Uuid::new_v4().to_string(),
        body: body.into(),
    };
    assert!(f.service.call(&f.actor, report("")).await.is_err());
    let open = crate::hall::tasks::get_task(&f.db, task.clone())
        .await
        .unwrap()
        .issues;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].tool, "hall_report");
    assert_eq!(open[0].attempt_id, attempt);
    assert!(!open[0].detail.is_empty());
    // Reads are not key actions and never leave an issue behind.
    f.service
        .call(&f.actor, ToolRequest::ReadMailbox {})
        .await
        .unwrap();
    f.service
        .call(&f.actor, report("delivered on retry"))
        .await
        .unwrap();
    let detail = crate::hall::tasks::get_task(&f.db, task).await.unwrap();
    assert!(detail.issues.is_empty());
    assert_eq!(serde_json::to_value(&detail).unwrap()["issues"], json!([]));
}
#[test]
fn claude_style_tool_names_are_recognised_only_for_hall_tools() {
    use crate::hall::team::mcp::auto_allowed_tool_name;
    assert!(auto_allowed_tool_name("mcp__codeg-mcp__hall_report"));
    assert!(auto_allowed_tool_name("mcp__codeg-mcp__hall_task_status"));
    assert!(!auto_allowed_tool_name("mcp__codeg-mcp__delegate_to_agent"));
    assert!(!auto_allowed_tool_name("mcp__other__hall_report"));
    assert!(!auto_allowed_tool_name("hall_report"));
    assert!(!auto_allowed_tool_name("Bash"));
}

#[tokio::test]
async fn branch_tools_require_host_pm_role_and_only_create_one_pending_intention() {
    use crate::hall::branches::{dispatch, model::RegisterBranchInput, service};
    let f = Fixture::new().await;
    let branch = service::register(
        &f.db,
        RegisterBranchInput {
            name: "Test branch".into(),
            base_url: "http://127.0.0.1:9".into(),
            token: "workspace-private-token".into(),
            headers: vec![],
        },
    )
    .await
    .unwrap();
    let draft = || dispatch::Draft {
        submission_id: "one-model-submission".into(),
        branch_id: branch.id.clone(),
        member_ref: "worker".into(),
        title: "Add the numbers".into(),
        description: "Await local acceptance".into(),
        exchange_manifest_id: None,
    };
    assert!(f
        .service
        .call(&f.actor, ToolRequest::ListBranches {})
        .await
        .is_err());
    assert!(f
        .service
        .call(&f.actor, ToolRequest::DispatchBranch { draft: draft() })
        .await
        .is_err());
    assert!(dispatch::list(&f.db).await.unwrap().is_empty());
    let listed = f
        .service
        .call(&f.pm_actor, ToolRequest::ListBranches {})
        .await
        .unwrap();
    assert_eq!(listed[0]["id"], branch.id);
    assert!(!listed.to_string().contains("private-token"));
    let first = f
        .service
        .call(&f.pm_actor, ToolRequest::DispatchBranch { draft: draft() })
        .await
        .unwrap();
    let retry = f
        .service
        .call(&f.pm_actor, ToolRequest::DispatchBranch { draft: draft() })
        .await
        .unwrap();
    assert_eq!(first["dispatchId"], retry["dispatchId"]);
    assert_eq!(retry["status"], "pending_delivery");
    assert!(retry["syncError"].is_string());
    assert_eq!(dispatch::list(&f.db).await.unwrap().len(), 1);
    let origin =
        f.db.query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT creator_member_id FROM hall_branch_dispatch WHERE dispatch_id=?",
            vec![retry["dispatchId"].as_str().unwrap().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        origin.try_get::<String>("", "creator_member_id").unwrap(),
        f.pm_id()
    );
    assert!(f
        .service
        .call(
            &f.actor,
            ToolRequest::BranchDispatchStatus {
                dispatch_id: retry["dispatchId"].as_str().unwrap().into()
            }
        )
        .await
        .is_err());
    let before = crate::hall::tasks::list_tasks(&f.db, Default::default())
        .await
        .unwrap();
    assert_eq!(
        before.len(),
        2,
        "remote intention must not create a local execution"
    );
    bind_pm(&f.db, None, 2).await.unwrap();
    assert!(f
        .service
        .call(&f.pm_actor, ToolRequest::DispatchBranch { draft: draft() })
        .await
        .is_err());
}

#[tokio::test]
async fn branch_result_notice_is_idempotent_and_never_starts_a_local_task() {
    use crate::hall::branches::{dispatch, model::RegisterBranchInput, service};
    let f = Fixture::new().await;
    let branch = service::register(
        &f.db,
        RegisterBranchInput {
            name: "Remote worker".into(),
            base_url: "http://127.0.0.1:9".into(),
            token: "workspace-fixture".into(),
            headers: vec![],
        },
    )
    .await
    .unwrap();
    let row = dispatch::create(
        &f.db,
        dispatch::Draft {
            submission_id: uuid::Uuid::new_v4().to_string(),
            branch_id: branch.id.clone(),
            member_ref: "worker".into(),
            title: "Sum".into(),
            description: "Sum input".into(),
            exchange_manifest_id: None,
        },
    )
    .await
    .unwrap();
    notify_branch_result(&f.db, &row.dispatch_id).await.unwrap();
    assert!(inbox(&f.db, f.pm_id()).await.unwrap().is_empty());
    f.db.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_branch_dispatch SET status='succeeded',remote_revision=2,remote_task_revision=7,result_text='29' WHERE dispatch_id=?",vec![row.dispatch_id.clone().into()])).await.unwrap();
    let (a, b) = tokio::join!(
        notify_branch_result(&f.db, &row.dispatch_id),
        notify_branch_result(&f.db, &row.dispatch_id)
    );
    a.unwrap();
    b.unwrap();
    let mail = inbox(&f.db, f.pm_id()).await.unwrap();
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].kind, "report");
    assert!(mail[0].sender_member_id.is_none());
    assert_eq!(mail[0].sender_name.as_deref(), Some("Branch report"));
    let body: Value = serde_json::from_str(&mail[0].body).unwrap();
    assert_eq!(body["dispatchId"], row.dispatch_id);
    assert_eq!(body["branchId"], branch.id);
    assert_eq!(body["result"], "29");
    assert_eq!(body["executionRequested"], false);
    assert_eq!(
        tasks::list_tasks(&f.db, Default::default())
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn attachment_dispatch_notifies_once_and_only_after_artifacts_are_collected() {
    use crate::hall::branches::{delivery, dispatch, model::RegisterBranchInput, service};
    use crate::hall::exchange::{service::administrator, Exchange};
    use sha2::{Digest, Sha256};
    let f = Fixture::new().await;
    #[cfg(unix)]
    let root = f._dir.path().canonicalize().unwrap().join("exchange");
    #[cfg(windows)]
    let root = f._dir.path().join("exchange");
    let exchange = Exchange::open(f.db.clone(), &root).unwrap();
    let admin = administrator();
    let bytes = b"input";
    let submission = uuid::Uuid::new_v4().to_string();
    let ticket = exchange
        .prepare_input(
            &admin,
            &submission,
            "in.txt",
            bytes.len() as u64,
            &format!("{:x}", Sha256::digest(bytes)),
        )
        .await
        .unwrap();
    exchange
        .upload(&admin, &submission, &ticket.transfer_id, &bytes[..])
        .await
        .unwrap();
    exchange
        .confirm_input(&admin, &submission, &ticket.object_id)
        .await
        .unwrap();
    let manifest = exchange
        .prepare_manifest(
            &admin,
            &submission,
            &[ticket.object_id],
            &["out.txt".into()],
        )
        .await
        .unwrap();
    // Unreachable endpoint: every sync fails to reach the branch, so the
    // stored report and the collection marker are exactly what the test sets.
    let branch = service::register(
        &f.db,
        RegisterBranchInput {
            name: "Remote worker".into(),
            base_url: "http://127.0.0.1:9".into(),
            token: "workspace-fixture".into(),
            headers: vec![],
        },
    )
    .await
    .unwrap();
    f.db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_branch_dispatch_credential(branch_id,issuer_id,secret,endpoint) SELECT ?,?,?,r.base_url FROM hall_branch b JOIN remote_workspace_connection r ON r.id=b.connection_id WHERE b.id=?",
        vec![
            branch.id.clone().into(),
            uuid::Uuid::new_v4().to_string().into(),
            "s".repeat(43).into(),
            branch.id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    let row = dispatch::create_with_exchange(
        &f.db,
        Some(&exchange),
        dispatch::Draft {
            submission_id: submission,
            branch_id: branch.id.clone(),
            member_ref: "worker".into(),
            title: "Files".into(),
            description: "Use input".into(),
            exchange_manifest_id: Some(manifest.manifest_id),
        },
    )
    .await
    .unwrap();
    let set = |sql: &'static str| {
        let id = row.dispatch_id.clone();
        let db = f.db.clone();
        async move {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                vec![id.into()],
            ))
            .await
            .unwrap();
        }
    };
    // Terminal result reported, but artifact collection is still owed.
    set("UPDATE hall_branch_dispatch SET status='succeeded',remote_revision=2,remote_task_revision=7,result_text='29',artifact_sync_error='pending' WHERE dispatch_id=?").await;
    for _ in 0..2 {
        delivery::sync_with(&f.db, Some(&exchange), &row.dispatch_id)
            .await
            .unwrap();
    }
    assert!(
        inbox(&f.db, f.pm_id()).await.unwrap().is_empty(),
        "no notice before the files arrived"
    );
    // Collection finished: exactly one notice, however often it is re-synced.
    set("UPDATE hall_branch_dispatch SET artifact_sync_error=NULL WHERE dispatch_id=?").await;
    for _ in 0..3 {
        delivery::sync_with(&f.db, Some(&exchange), &row.dispatch_id)
            .await
            .unwrap();
    }
    let mail = inbox(&f.db, f.pm_id()).await.unwrap();
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].kind, "report");
    let body: Value = serde_json::from_str(&mail[0].body).unwrap();
    assert_eq!(body["status"], "succeeded");
    assert_eq!(body["executionRequested"], false);
}
