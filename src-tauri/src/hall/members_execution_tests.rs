//! Real authenticated API; every file/database here belongs to a fixture.
use super::members_tests::{draft, list, member_dir, preset, register, server, TEST_AUTH};
use crate::db::entities::assistant;
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
async fn prepare(server: &TestServer, member: &Value, observed: &Value) -> TestResponse {
    server.post("/api/hall_preview_member_execution").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"],"expectedCheckVersion":observed["checkVersion"]})).await
}
async fn template(
    db: &DatabaseConnection,
    mode: Option<&str>,
    config_mode: Option<&str>,
) -> assistant::Model {
    let row = preset(db, "codex", true).await;
    let mut values = json!({"model":"private-preset-model"});
    if let Some(value) = config_mode {
        values["mode"] = json!(value);
    }
    let labels = values
        .as_object()
        .unwrap()
        .iter()
        .map(|(id, _)| json!({"id":id,"kind":"select","name":id,"value_label":"Private label"}))
        .collect::<Vec<_>>();
    assistant::Entity::update_many()
        .col_expr(
            assistant::Column::AgentOptions,
            Expr::value(json!({"mode_id":mode,"config_values":values}).to_string()),
        )
        .col_expr(
            assistant::Column::LabelSnapshot,
            Expr::value(json!({"agent":"Codex","mode":mode,"options":labels}).to_string()),
        )
        .col_expr(
            assistant::Column::Instructions,
            Expr::value("private fixture instructions"),
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
async fn a_default_candidate_is_stable_private_and_never_changes_member_or_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let observed = read(&server, &member).await;
    let first = value(prepare(&server, &member, &observed).await);
    assert_eq!(first["valid"], true);
    assert_eq!(first["launchAllowed"], false);
    assert_eq!(first["summary"]["requestedMode"], "read-only");
    assert_eq!(first["summary"]["modelSource"], "engine_default");
    assert_eq!(first["summary"]["permissionPolicy"], "wait");
    assert_eq!(first["summary"]["runtimeVerified"], false);
    assert_eq!(first["preflight"]["executionPreviewSupported"], true);
    let second = value(prepare(&server, &member, &observed).await);
    assert_eq!(
        first["summary"]["snapshotVersion"],
        second["summary"]["snapshotVersion"]
    );
    assert_eq!(
        first["summary"]["snapshotVersion"].as_str().unwrap().len(),
        64
    );
    assert_eq!(list(&server).await, vec![member]);
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
}

#[tokio::test]
async fn a_bound_template_is_frozen_without_exposing_content_and_changed_versions_require_reconfirmation(
) {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let template = template(&conn, None, None).await;
    let server = server(db, dir.path());
    let mut input = draft(&path);
    input["assistantUid"] = json!(template.uid);
    let member = value(register(&server, input).await);
    let observed = read(&server, &member).await;
    let first = value(prepare(&server, &member, &observed).await);
    assert_eq!(first["valid"], true);
    assert_eq!(first["summary"]["modelSource"], "template");
    assert_eq!(first["summary"]["instructionsIncluded"], true);
    assert!(!first.to_string().contains("private"));
    assert_eq!(list(&server).await, vec![member.clone()]);
    assistant::Entity::update_many()
        .col_expr(
            assistant::Column::Instructions,
            Expr::value("changed private instructions"),
        )
        .col_expr(
            assistant::Column::Revision,
            Expr::value(template.revision + 1),
        )
        .filter(assistant::Column::Id.eq(template.id))
        .exec(&conn)
        .await
        .unwrap();
    let stale = value(prepare(&server, &member, &observed).await);
    assert_eq!(stale["valid"], false);
    assert!(stale["summary"].is_null());
    let updated = value(
        server
            .post("/api/hall_update_member")
            .add_header("authorization", TEST_AUTH)
            .json(
                &json!({"memberId":member["memberId"],"expectedRevision":member["revision"],
            "draft":{"name":member["name"],"role":member["role"],"assistantUid":template.uid}}),
            )
            .await,
    );
    let current = read(&server, &updated).await;
    let later = value(prepare(&server, &updated, &current).await);
    assert_eq!(later["valid"], true);
    assert_ne!(
        later["summary"]["snapshotVersion"],
        first["summary"]["snapshotVersion"]
    );
    assert!(!later.to_string().contains("private"));
    assert!(!path.join(".onehall").exists());
}

#[tokio::test]
async fn unsupported_path_syntax_and_mode_conflicts_fail_closed_with_fixed_issues() {
    for (mode, config_mode, rule, code) in [
        (Some("agent-full-access"), None, "**", "unsafe_mode"),
        (Some("workspace-write"), None, "**", "mode_exceeds_scope"),
        (
            Some("read-only"),
            Some("workspace-write"),
            "**",
            "conflicting_mode",
        ),
        (None, None, "bad[", "invalid_path_rule"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = member_dir(dir.path(), "member");
        std::fs::write(
            path.join("agent.yaml"),
            format!("schema_version: 1\nscope:\n  read: ['{rule}']\n"),
        )
        .unwrap();
        let db = fresh_in_memory_db().await;
        let row = template(&db.conn, mode, config_mode).await;
        let server = server(db, dir.path());
        let mut input = draft(&path);
        input["assistantUid"] = json!(row.uid);
        let member = value(register(&server, input).await);
        let observed = read(&server, &member).await;
        let report = value(prepare(&server, &member, &observed).await);
        assert_eq!(report["valid"], false);
        assert!(report["summary"].is_null());
        assert!(report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == code));
        assert!(!report.to_string().contains("private"));
        assert!(!path.join(".onehall").exists());
    }
}

#[tokio::test]
async fn candidate_requests_are_authenticated_versioned_and_never_accept_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let observed = read(&server, &member).await;
    server
        .post("/api/hall_preview_member_execution")
        .json(&json!({}))
        .await
        .assert_status_unauthorized();
    let bad = json!({"checkVersion":"bad"});
    prepare(&server, &member, &bad)
        .await
        .assert_status_bad_request();
    let stale = json!({"memberId":member["memberId"],"revision":2});
    prepare(&server, &stale, &observed)
        .await
        .assert_status_conflict();
    let forged = server.post("/api/hall_preview_member_execution").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":1,"expectedCheckVersion":observed["checkVersion"],"path":"/tmp/outside"})).await;
    assert!(forged.status_code().is_client_error());
    std::fs::write(
        path.join("agent.yaml"),
        "schema_version: 1\nmodel: changed\n",
    )
    .unwrap();
    prepare(&server, &member, &observed)
        .await
        .assert_status_conflict();
    assert_eq!(list(&server).await, vec![member]);
}
