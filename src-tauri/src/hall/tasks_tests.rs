//! Authenticated in-process API and disposable databases only. No real engines.
use super::members_snapshot_tests::{capture, params, save};
use super::members_tests::{draft, member_dir, register, server, TEST_AUTH};
use super::upstream_040_tests::Released040;
use crate::db::{
    migration::Migrator,
    test_helpers::{fresh_disk_db, fresh_in_memory_db},
    AppDatabase,
};
use axum_test::{TestResponse, TestServer};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::{MigrationTrait, MigratorTrait};
use serde_json::{json, Value};

pub(super) fn value(r: TestResponse) -> Value {
    assert_eq!(r.status_code(), 200, "{}", r.text());
    r.json()
}
pub(super) async fn post(server: &TestServer, command: &str, input: Value) -> TestResponse {
    server
        .post(&format!("/api/{command}"))
        .add_header("authorization", TEST_AUTH)
        .json(&input)
        .await
}
pub(super) async fn fixture(
    server: &TestServer,
    root: &std::path::Path,
    name: &str,
) -> (Value, Value) {
    let path = member_dir(root, name);
    let member = value(register(server, draft(&path)).await);
    let candidate = capture(server, &member).await;
    let saved = value(save(server, &params(&member, &candidate)).await);
    (member, saved)
}
pub(super) fn input(member: &Value, snapshot: &Value) -> Value {
    json!({"draft":{"submissionId":uuid::Uuid::new_v4().to_string(),"memberId":member["memberId"],
        "snapshotId":snapshot["snapshotId"],"projectId":null,"purpose":"work","title":"Review fixture","description":"private task fixture body"}})
}
async fn count(db: &DatabaseConnection, table: &str) -> i64 {
    assert!(["hall_task", "hall_task_attempt", "hall_task_history"].contains(&table));
    db.query_one(Statement::from_string(
        DbBackend::Sqlite,
        format!("SELECT COUNT(*) AS n FROM {table}"),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

pub(super) struct ReleasedTeam060;
#[async_trait::async_trait]
impl MigratorTrait for ReleasedTeam060 {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Migrator::migrations()
            .into_iter()
            .filter(|m| {
                let name = m.name();
                (name.starts_with('m') && name <= "m20261022_000001_tool_approval")
                    || (name.starts_with("onehall_")
                        && name <= "onehall_m20261025_000002_team_ledger")
            })
            .collect()
    }
}

#[tokio::test]
async fn draft_creation_is_concurrently_idempotent_and_never_creates_an_execution() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let input = input(&m, &s);
    let (a, b) = tokio::join!(
        post(&server, "hall_create_task", input.clone()),
        post(&server, "hall_create_task", input.clone())
    );
    let a = value(a);
    assert_eq!(a, value(b));
    assert_eq!(a["status"], "draft");
    assert_eq!(a["launchAllowed"], false);
    assert_eq!(a["runtimeVerified"], false);
    assert!(a.get("description").is_none());
    assert!(!a.to_string().contains("payload"));
    assert_eq!(count(&conn, "hall_task").await, 1);
    assert_eq!(count(&conn, "hall_task_history").await, 1);
    assert_eq!(count(&conn, "hall_task_attempt").await, 0);
    let mut changed = input;
    changed["draft"]["title"] = json!("different");
    assert_eq!(
        post(&server, "hall_create_task", changed)
            .await
            .status_code(),
        409
    );
    assert_eq!(
        super::members_tests::list(&server).await[0]["enabled"],
        false
    );
}

#[tokio::test]
async fn edits_are_cas_drafts_only_and_replayed_creation_keeps_the_original_identity() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let input = input(&m, &s);
    let a = value(post(&server, "hall_create_task", input.clone()).await);
    let id = a["taskId"].clone();
    let edit = json!({"taskId":id,"expectedRevision":1,"draft":{"title":"Changed","description":"new explicit task text"}});
    let (x, y) = tokio::join!(
        post(&server, "hall_edit_task", edit.clone()),
        post(&server, "hall_edit_task", edit)
    );
    let statuses = [x.status_code().as_u16(), y.status_code().as_u16()];
    assert!(statuses.contains(&200) && statuses.contains(&409));
    assert_eq!(
        post(
            &server,
            "hall_cancel_task",
            json!({"taskId":id,"expectedRevision":1})
        )
        .await
        .status_code(),
        409
    );
    value(
        post(
            &server,
            "hall_cancel_task",
            json!({"taskId":id,"expectedRevision":2}),
        )
        .await,
    );
    assert_eq!(
        post(
            &server,
            "hall_edit_task",
            json!({"taskId":id,"expectedRevision":3,"draft":{"title":"No","description":""}})
        )
        .await
        .status_code(),
        409
    );
    let repeated = value(post(&server, "hall_create_task", input).await);
    assert_eq!(repeated["taskId"], id);
    assert_eq!(repeated["status"], "cancelled");
    let detail = value(post(&server, "hall_get_task", json!({"taskId":id})).await);
    assert_eq!(detail["revision"], 3);
    assert_eq!(detail["history"].as_array().unwrap().len(), 3);
    assert_eq!(detail["attempts"], json!([]));
    assert_eq!(detail["history"][0]["action"], "draft_cancelled");
    assert_eq!(detail["description"], "new explicit task text");
}

#[tokio::test]
async fn member_project_and_snapshot_bindings_cannot_be_forged_or_crossed() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let (other, _) = fixture(&server, dir.path(), "other").await;
    let mut cross = input(&m, &s);
    cross["draft"]["memberId"] = other["memberId"].clone();
    assert_eq!(
        post(&server, "hall_create_task", cross).await.status_code(),
        400
    );
    let mut wrong = input(&m, &s);
    wrong["draft"]["projectId"] = json!(99);
    assert_eq!(
        post(&server, "hall_create_task", wrong).await.status_code(),
        400
    );
    for field in ["status", "enabled", "launchAllowed", "attemptId"] {
        let mut forged = input(&m, &s);
        forged["draft"][field] = json!("running");
        assert!(post(&server, "hall_create_task", forged)
            .await
            .status_code()
            .is_client_error());
    }
    let mut omitted = input(&m, &s);
    omitted["draft"]
        .as_object_mut()
        .unwrap()
        .remove("projectId");
    assert!(post(&server, "hall_create_task", omitted)
        .await
        .status_code()
        .is_client_error());
}

#[tokio::test]
async fn history_keeps_saved_snapshot_after_member_and_configuration_change() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let created = value(post(&server, "hall_create_task", input(&m, &s)).await);
    value(post(&server,"hall_update_member",json!({"memberId":m["memberId"],"expectedRevision":1,"draft":{"name":"Renamed","role":"Other","assistantUid":null}})).await);
    std::fs::write(
        dir.path().join("member/agent.yaml"),
        "invalid current declaration: [",
    )
    .unwrap();
    let d = value(
        post(
            &server,
            "hall_get_task",
            json!({"taskId":created["taskId"]}),
        )
        .await,
    );
    assert_eq!(d["snapshot"]["snapshotId"], s["snapshotId"]);
    assert_eq!(d["snapshot"]["memberRevision"], 1);
    assert_eq!(
        d["snapshot"]["snapshotVersion"],
        s["summary"]["snapshotVersion"]
    );
    let rows = value(
        post(
            &server,
            "hall_list_tasks",
            json!({"filter":{"memberId":m["memberId"],"unassignedProject":true}}),
        )
        .await,
    );
    assert_eq!(rows, json!([created]));
    assert!(!rows.to_string().contains("private task fixture body"));
}

#[tokio::test]
async fn all_task_routes_require_authentication_and_bad_records_hide_storage_details() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    for command in [
        "hall_create_task",
        "hall_list_tasks",
        "hall_get_task",
        "hall_edit_task",
        "hall_cancel_task",
    ] {
        assert_eq!(
            server
                .post(&format!("/api/{command}"))
                .json(&json!({}))
                .await
                .status_code(),
            401
        );
    }
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let created = value(post(&server, "hall_create_task", input(&m, &s)).await);
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "UPDATE hall_execution_snapshot SET payload='private invalid \
        fixture'",
    ))
    .await
    .unwrap();
    let r = post(
        &server,
        "hall_get_task",
        json!({"taskId":created["taskId"]}),
    )
    .await;
    assert_eq!(r.status_code(), 500);
    assert!(!r.text().contains("private invalid fixture"));
    assert!(!r.text().contains("SELECT"));
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "UPDATE hall_task SET request_hash='corrupt'",
    ))
    .await
    .unwrap();
    assert_eq!(
        post(&server, "hall_list_tasks", json!({}))
            .await
            .status_code(),
        500
    );
}

#[tokio::test]
async fn team_upgrade_59_to_60_preserves_history_snapshots_and_backup_compatibility() {
    use crate::commands::backup::{core, sections::LiveRoots};
    use crate::web::event_bridge::EventEmitter;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("team.db").display());
    let conn = Database::connect(&url).await.unwrap();
    Released040::up(&conn, None).await.unwrap();
    assert_eq!(
        Released040::get_applied_migrations(&conn)
            .await
            .unwrap()
            .len(),
        59
    );
    let server = server(AppDatabase { conn: conn.clone() }, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let manual = dir.path().join("member/agent.yaml");
    std::fs::write(&manual, "# preserved fixture\nschema_version: 1\n").unwrap();
    ReleasedTeam060::up(&conn, None).await.unwrap();
    ReleasedTeam060::up(&conn, None).await.unwrap();
    assert_eq!(
        ReleasedTeam060::get_applied_migrations(&conn)
            .await
            .unwrap()
            .len(),
        60
    );
    let saved = value(post(&server, "hall_create_task", input(&m, &s)).await);
    let before = value(post(&server, "hall_get_task", json!({"taskId":saved["taskId"]})).await);
    let manifest = core::create_backup_core(
        core::BackupInputs {
            conn: &conn,
            data_dir: dir.path(),
            live_roots: LiveRoots::rooted_at(&dir.path().join("empty")),
            app_version: "0.39.0",
            runtime_label: "server",
        },
        core::BackupOptions::default(),
        &dir.path().join("team.zip"),
        &EventEmitter::Noop,
        "team-fixture",
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        manifest.latest_migration,
        Migrator::migrations().last().unwrap().name()
    );
    assert!(!Released040::migrations()
        .iter()
        .any(|m| m.name() == manifest.latest_migration));
    assert!(core::evaluate_compat(&manifest).0);
    let old = crate::commands::backup::manifest::BackupManifest {
        latest_migration: "onehall_m20261025_000001_upstream040".into(),
        ..manifest
    };
    assert!(core::evaluate_compat(&old).0);
    assert!(Released040::get_pending_migrations(&conn).await.is_err());
    drop(server);
    conn.close().await.unwrap();
    let reopened = Database::connect(&url).await.unwrap();
    ReleasedTeam060::up(&reopened, None).await.unwrap();
    let after = super::tasks::get_task(&reopened, saved["taskId"].as_str().unwrap().into())
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(after).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(manual).unwrap(),
        "# preserved fixture\nschema_version: 1\n"
    );
    assert_eq!(count(&reopened, "hall_task").await, 1);
    assert_eq!(count(&reopened, "hall_task_attempt").await, 0);
}

#[tokio::test]
async fn explicit_project_filters_and_replays_preserve_historical_project_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let project = crate::db::service::folder_service::add_folder(&conn, "/fixture/project")
        .await
        .unwrap();
    let server = server(db, dir.path());
    let path = member_dir(dir.path(), "project-member");
    let mut m_draft = draft(&path);
    m_draft["projectId"] = json!(project.id);
    let m = value(register(&server, m_draft).await);
    let c = capture(&server, &m).await;
    let saved = value(save(&server, &params(&m, &c)).await);
    assert_eq!(saved["projectId"], project.id);
    let mut request = input(&m, &saved);
    request["draft"]["projectId"] = json!(project.id);
    let t = value(post(&server, "hall_create_task", request.clone()).await);
    assert_eq!(
        value(
            post(
                &server,
                "hall_list_tasks",
                json!({"filter":{"projectId":project.id}})
            )
            .await
        ),
        json!([t.clone()])
    );
    assert_eq!(
        value(
            post(
                &server,
                "hall_list_tasks",
                json!({"filter":{"unassignedProject":true}})
            )
            .await
        ),
        json!([])
    );
    assert_eq!(
        post(
            &server,
            "hall_list_tasks",
            json!({"filter":{"unassignedProject":true,"projectId":project.id}})
        )
        .await
        .status_code(),
        400
    );
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE folder SET deleted_at=? WHERE id=?",
        vec![chrono::Utc::now().into(), project.id.into()],
    ))
    .await
    .unwrap();
    assert_eq!(
        value(post(&server, "hall_create_task", request.clone()).await),
        t
    );
    request["draft"]["submissionId"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        post(&server, "hall_create_task", request)
            .await
            .status_code(),
        400
    );
    let detail = value(post(&server, "hall_get_task", json!({"taskId":t["taskId"]})).await);
    assert_eq!(detail["projectId"], project.id);
}

#[tokio::test]
async fn draft_history_corruption_cannot_be_presented_as_valid_execution_history() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let t = value(post(&server, "hall_create_task", input(&m, &s)).await);
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "UPDATE hall_task SET status='running'",
    ))
    .await
    .unwrap();
    assert_eq!(
        post(&server, "hall_list_tasks", json!({}))
            .await
            .status_code(),
        500
    );
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "UPDATE hall_task SET status='draft'",
    ))
    .await
    .unwrap();
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM hall_task_history",
    ))
    .await
    .unwrap();
    assert_eq!(
        post(&server, "hall_get_task", json!({"taskId":t["taskId"]}))
            .await
            .status_code(),
        500
    );
}
