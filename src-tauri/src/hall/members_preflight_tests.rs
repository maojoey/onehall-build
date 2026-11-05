//! Saved-data checks through the authenticated router. Disposable DB/files only.
use super::members_tests::{draft, list, member_dir, preset, register, server, TEST_AUTH};
use crate::db::entities::{assistant, folder};
use crate::db::service::folder_service;
use crate::db::test_helpers::fresh_in_memory_db;
use axum_test::{TestResponse, TestServer};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{json, Value};

fn value(response: TestResponse) -> Value {
    assert_eq!(response.status_code(), 200, "{}", response.text());
    response.json()
}
async fn read(server: &TestServer, member: &Value) -> Value {
    value(
        server
            .post("/api/hall_preview_member_config")
            .add_header("authorization", TEST_AUTH)
            .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"]}))
            .await,
    )
}
async fn check(server: &TestServer, member: &Value, observed: &Value) -> TestResponse {
    server
        .post("/api/hall_check_member")
        .add_header("authorization", TEST_AUTH)
        .json(
            &json!({"memberId":member["memberId"],"expectedRevision":member["revision"],
            "expectedCheckVersion":observed["checkVersion"]}),
        )
        .await
}
fn row<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap()
}
async fn template(db: &DatabaseConnection) -> assistant::Model {
    let row = preset(db, "codex", true).await;
    assistant::Entity::update_many()
        .col_expr(
            assistant::Column::LabelSnapshot,
            Expr::value(r#"{"agent":"Codex","options":[]}"#),
        )
        .filter(assistant::Column::Id.eq(row.id))
        .exec(db)
        .await
        .unwrap();
    assistant::Entity::find_by_id(row.id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn default_member_is_only_data_checked_and_stays_disabled_without_any_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let observed = read(&server, &member).await;
    assert_eq!(observed["checkSupported"], true);
    assert_eq!(observed["checkVersion"].as_str().unwrap().len(), 64);
    let report = value(check(&server, &member, &observed).await);
    assert_eq!(report["configurationValid"], true);
    assert_eq!(report["activationAvailable"], false);
    assert_eq!(row(&report, "declaration")["code"], "defaults_only");
    assert_eq!(row(&report, "template")["code"], "template_not_bound");
    assert_eq!(row(&report, "project")["code"], "project_not_bound");
    assert_eq!(row(&report, "runtime")["status"], "pending");
    assert_eq!(row(&report, "execution")["status"], "pending");
    assert_eq!(report["checkVersion"], observed["checkVersion"]);
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
    assert_eq!(list(&server).await, vec![member]);
}

#[tokio::test]
async fn invalid_declarations_have_observation_versions_but_never_expose_raw_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let raw = "schema_version: 1\nprivate-key: private-value\n";
    std::fs::write(path.join("agent.yaml"), raw).unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let observed = read(&server, &member).await;
    assert_eq!(observed["valid"], false);
    assert!(observed.get("fileVersion").is_none());
    let report = value(check(&server, &member, &observed).await);
    assert_eq!(report["configurationValid"], false);
    assert_eq!(row(&report, "declaration")["status"], "blocked");
    assert_eq!(report["diagnostics"][0]["code"], "unknown_field");
    assert!(!report.to_string().contains("private"));
    assert_eq!(
        std::fs::read_to_string(path.join("agent.yaml")).unwrap(),
        raw
    );
    assert!(!path.join(".onehall").exists());
    assert_eq!(list(&server).await, vec![member]);
}

#[tokio::test]
async fn a_missing_directory_is_blocked_and_never_recreated() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    std::fs::rename(&path, dir.path().join("moved")).unwrap();
    let observed = read(&server, &member).await;
    let report = value(check(&server, &member, &observed).await);
    assert_eq!(report["configurationValid"], false);
    assert_eq!(row(&report, "directory")["status"], "blocked");
    assert_eq!(row(&report, "declaration")["status"], "pending");
    assert!(!path.exists());
}

#[tokio::test]
async fn template_health_and_revision_are_read_live_without_returning_options_or_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let template = template(&conn).await;
    let server = server(db, dir.path());
    let mut draft = draft(&path);
    draft["assistantUid"] = json!(template.uid);
    let member = value(register(&server, draft).await);
    let observed = read(&server, &member).await;
    let report = value(check(&server, &member, &observed).await);
    assert_eq!(row(&report, "template")["code"], "template_ok");
    assert!(!report.to_string().contains("instructions"));
    assert!(!report.to_string().contains("agentOptions"));
    assistant::Entity::update_many()
        .col_expr(
            assistant::Column::Revision,
            Expr::value(template.revision + 1),
        )
        .filter(assistant::Column::Id.eq(template.id))
        .exec(&conn)
        .await
        .unwrap();
    let report = value(check(&server, &member, &observed).await);
    assert_eq!(report["configurationValid"], false);
    assert_eq!(row(&report, "template")["code"], "template_changed");
    assert_eq!(
        report["templateRecordedRevision"],
        member["assistantRevision"]
    );
    assert_eq!(report["templateCurrentRevision"], template.revision + 1);
    assistant::Entity::update_many()
        .col_expr(assistant::Column::Enabled, Expr::value(false))
        .filter(assistant::Column::Id.eq(template.id))
        .exec(&conn)
        .await
        .unwrap();
    assert_eq!(
        row(&value(check(&server, &member, &observed).await), "template")["code"],
        "template_disabled"
    );
    assistant::Entity::update_many()
        .col_expr(assistant::Column::Enabled, Expr::value(true))
        .col_expr(assistant::Column::AgentType, Expr::value("claude_code"))
        .filter(assistant::Column::Id.eq(template.id))
        .exec(&conn)
        .await
        .unwrap();
    assert_eq!(
        row(&value(check(&server, &member, &observed).await), "template")["code"],
        "template_engine_mismatch"
    );
    assistant::Entity::delete_by_id(template.id)
        .exec(&conn)
        .await
        .unwrap();
    assert_eq!(
        row(&value(check(&server, &member, &observed).await), "template")["code"],
        "template_missing"
    );
    assert_eq!(list(&server).await, vec![member]);
}

#[tokio::test]
async fn incomplete_or_corrupt_saved_template_options_are_not_defaulted_or_echoed() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let template = template(&conn).await;
    let server = server(db, dir.path());
    let mut draft = draft(&path);
    draft["assistantUid"] = json!(template.uid);
    let member = value(register(&server, draft).await);
    let observed = read(&server, &member).await;
    for options in [
        "not-json-private",
        r#"{"config_values":{"private-key":"private-value"}}"#,
    ] {
        assistant::Entity::update_many()
            .col_expr(assistant::Column::AgentOptions, Expr::value(options))
            .filter(assistant::Column::Id.eq(template.id))
            .exec(&conn)
            .await
            .unwrap();
        let report = value(check(&server, &member, &observed).await);
        assert_eq!(row(&report, "template")["code"], "template_invalid");
        assert!(!report.to_string().contains("private"));
    }
}

#[tokio::test]
async fn project_binding_is_metadata_only_but_must_still_reference_a_live_regular_root() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let project = folder_service::add_folder(&conn, &dir.path().join("project").to_string_lossy())
        .await
        .unwrap();
    folder::Entity::update_many()
        .col_expr(folder::Column::IsOpen, Expr::value(false))
        .filter(folder::Column::Id.eq(project.id))
        .exec(&conn)
        .await
        .unwrap();
    let server = server(db, dir.path());
    let mut input = draft(&path);
    input["projectId"] = json!(project.id);
    let member = value(register(&server, input).await);
    let observed = read(&server, &member).await;
    assert_eq!(
        row(&value(check(&server, &member, &observed).await), "project")["code"],
        "project_ok"
    );
    folder::Entity::update_many()
        .col_expr(folder::Column::ParentId, Expr::value(project.id))
        .filter(folder::Column::Id.eq(project.id))
        .exec(&conn)
        .await
        .unwrap();
    assert_eq!(
        row(&value(check(&server, &member, &observed).await), "project")["code"],
        "project_unavailable"
    );
    folder::Entity::update_many()
        .col_expr(folder::Column::ParentId, Expr::value(None::<i32>))
        .col_expr(folder::Column::Kind, Expr::value("chat"))
        .filter(folder::Column::Id.eq(project.id))
        .exec(&conn)
        .await
        .unwrap();
    assert_eq!(
        row(&value(check(&server, &member, &observed).await), "project")["code"],
        "project_unavailable"
    );
    folder_service::soft_delete_folder(&conn, project.id)
        .await
        .unwrap();
    assert_eq!(
        row(&value(check(&server, &member, &observed).await), "project")["code"],
        "project_unavailable"
    );
    assert_eq!(list(&server).await, vec![member]);
    assert!(!path.join(".onehall").exists());
}

#[tokio::test]
async fn check_requires_authentication_current_observation_and_member_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let observed = read(&server, &member).await;
    server
        .post("/api/hall_check_member")
        .json(&json!({}))
        .await
        .assert_status_unauthorized();
    let invalid = json!({"checkVersion":"bad"});
    check(&server, &member, &invalid)
        .await
        .assert_status_bad_request();
    let stale = json!({"memberId":member["memberId"],"revision":2});
    check(&server, &stale, &observed)
        .await
        .assert_status_conflict();
    let missing = json!({"memberId":uuid::Uuid::new_v4().to_string(),"revision":1});
    check(&server, &missing, &observed)
        .await
        .assert_status_not_found();
    let forged = server.post("/api/hall_check_member").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":1,"expectedCheckVersion":observed["checkVersion"],"path":"/tmp/outside"})).await;
    assert!(forged.status_code().is_client_error());
    std::fs::write(path.join("agent.yaml"), "# external\nschema_version: 1\n").unwrap();
    check(&server, &member, &observed)
        .await
        .assert_status_conflict();
    assert_ne!(
        read(&server, &member).await["checkVersion"],
        observed["checkVersion"]
    );
}
