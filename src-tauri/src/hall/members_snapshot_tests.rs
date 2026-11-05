//! All storage and paths here are disposable fixtures; no running service.
use super::members_tests::{draft, list, member_dir, preset, register, server, TEST_AUTH};
use crate::db::migration::Migrator;
use crate::db::test_helpers::{fresh_disk_db, fresh_in_memory_db};
use axum_test::{TestResponse, TestServer};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;
use serde_json::{json, Value};

fn value(response: TestResponse) -> Value {
    assert_eq!(response.status_code(), 200, "{}", response.text());
    response.json()
}
pub(super) async fn capture(server: &TestServer, member: &Value) -> Value {
    let observed = value(
        server
            .post("/api/hall_preview_member_config")
            .add_header("authorization", TEST_AUTH)
            .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"]}))
            .await,
    );
    let candidate = value(server.post("/api/hall_preview_member_execution").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"],"expectedCheckVersion":observed["checkVersion"]})).await);
    assert_eq!(candidate["valid"], true, "{candidate}");
    candidate
}
pub(super) fn params(member: &Value, candidate: &Value) -> Value {
    json!({"memberId":member["memberId"],"expectedRevision":member["revision"],
        "expectedCheckVersion":candidate["checkVersion"],"expectedSnapshotVersion":candidate["summary"]["snapshotVersion"]})
}
pub(super) async fn save(server: &TestServer, input: &Value) -> TestResponse {
    server
        .post("/api/hall_save_member_snapshot")
        .add_header("authorization", TEST_AUTH)
        .json(input)
        .await
}
async fn history(server: &TestServer, member: &Value) -> TestResponse {
    server
        .post("/api/hall_list_member_snapshots")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"]}))
        .await
}
async fn count(db: &DatabaseConnection) -> i64 {
    db.query_one(Statement::from_string(
        DbBackend::Sqlite,
        "SELECT COUNT(*) AS n FROM hall_execution_snapshot",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

async fn valid_preset(db: &DatabaseConnection) -> crate::db::entities::assistant::Model {
    let row = preset(db, "codex", true).await;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE assistant SET label_snapshot = ? WHERE uid = ?",
        vec![
            r#"{"agent":"Codex","options":[]}"#.into(),
            row.uid.clone().into(),
        ],
    ))
    .await
    .unwrap();
    row
}

#[tokio::test]
async fn snapshots_are_explicit_idempotent_redacted_and_never_modify_a_member_or_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let candidate = capture(&server, &member).await;
    assert_eq!(candidate["preflight"]["snapshotStorageSupported"], true);
    assert_eq!(count(&conn).await, 0);
    let input = params(&member, &candidate);
    let (first_a, first_b) = tokio::join!(save(&server, &input), save(&server, &input));
    let first = value(first_a);
    assert_eq!(value(first_b), first);
    assert_eq!(value(save(&server, &input).await), first);
    assert_eq!(count(&conn).await, 1);
    assert!(uuid::Uuid::parse_str(first["snapshotId"].as_str().unwrap()).is_ok());
    assert_eq!(first["summary"], candidate["summary"]);
    assert_eq!(first["launchAllowed"], false);
    assert_eq!(first["memberRevision"], member["revision"]);
    assert!(first.get("payload").is_none());
    assert_eq!(value(history(&server, &member).await), json!([first]));
    assert_eq!(list(&server).await, vec![member]);
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
}

#[tokio::test]
async fn changed_files_and_template_contents_reject_a_previously_reviewed_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let template = valid_preset(&conn).await;
    let server = server(db, dir.path());
    let mut input = draft(&path);
    input["assistantUid"] = json!(template.uid);
    let member = value(register(&server, input).await);
    let original = capture(&server, &member).await;
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE assistant SET instructions = ? WHERE uid = ?",
        vec!["changed private fixture".into(), template.uid.into()],
    ))
    .await
    .unwrap();
    assert_eq!(
        save(&server, &params(&member, &original))
            .await
            .status_code(),
        409
    );
    assert_eq!(count(&conn).await, 0);
    let candidate = capture(&server, &member).await;
    let first = value(save(&server, &params(&member, &candidate)).await);
    let file = path.join("agent.yaml");
    let content = "schema_version: 1\nmodel: fixture-model\n";
    std::fs::write(&file, content).unwrap();
    assert_eq!(
        save(&server, &params(&member, &candidate))
            .await
            .status_code(),
        409
    );
    assert_eq!(count(&conn).await, 1);
    let updated = capture(&server, &member).await;
    let second = value(save(&server, &params(&member, &updated)).await);
    assert_ne!(first["snapshotId"], second["snapshotId"]);
    assert_ne!(
        first["summary"]["snapshotVersion"],
        second["summary"]["snapshotVersion"]
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), content);
}

#[tokio::test]
async fn disk_reopen_restores_the_original_content_without_live_template_or_directory_reads() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let template = valid_preset(&conn).await;
    let server = server(db, dir.path());
    let mut input = draft(&path);
    input["assistantUid"] = json!(template.uid);
    let member = value(register(&server, input).await);
    let candidate = capture(&server, &member).await;
    let saved = value(save(&server, &params(&member, &candidate)).await);
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE assistant SET instructions = ?, revision = revision + 1 WHERE uid = ?",
        vec!["different private fixture".into(), template.uid.into()],
    ))
    .await
    .unwrap();
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_member SET name = ?, revision = revision + 1 WHERE member_id = ?",
        vec![
            "renamed fixture".into(),
            member["memberId"].as_str().unwrap().into(),
        ],
    ))
    .await
    .unwrap();
    std::fs::rename(&path, path.with_extension("moved")).unwrap();
    assert_eq!(
        value(history(&server, &member).await),
        json!([saved.clone()])
    );
    drop(server);
    conn.close().await.unwrap();
    let reopened = Database::connect(format!(
        "sqlite:{}?mode=rw",
        dir.path().join("source.db").display()
    ))
    .await
    .unwrap();
    Migrator::up(&reopened, None).await.unwrap();
    let restored = super::members::hall_list_member_snapshots_core(
        &reopened,
        member["memberId"].as_str().unwrap().into(),
    )
    .await
    .unwrap();
    let public = serde_json::to_value(restored).unwrap();
    assert_eq!(public, json!([saved]));
    assert!(!public
        .to_string()
        .contains("template instructions stay here"));
    assert!(!public.to_string().contains("different private fixture"));
    let stored = reopened
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT payload FROM hall_execution_snapshot",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "payload")
        .unwrap();
    assert!(stored.contains("template instructions stay here"));
    assert!(!stored.contains("different private fixture"));
    assert!(!stored.contains("renamed fixture"));
}

#[tokio::test]
async fn authentication_versions_and_strict_parameters_prevent_unreviewed_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let candidate = capture(&server, &member).await;
    for route in [
        "/api/hall_save_member_snapshot",
        "/api/hall_list_member_snapshots",
    ] {
        assert_eq!(server.post(route).json(&json!({})).await.status_code(), 401);
    }
    let mut input = params(&member, &candidate);
    input["expectedSnapshotVersion"] = json!("wrong");
    assert_eq!(save(&server, &input).await.status_code(), 400);
    input["expectedSnapshotVersion"] = json!("a".repeat(64));
    assert_eq!(save(&server, &input).await.status_code(), 409);
    input = params(&member, &candidate);
    input["expectedRevision"] = json!(2);
    assert_eq!(save(&server, &input).await.status_code(), 409);
    input = params(&member, &candidate);
    input["payload"] = json!("private supplied fixture");
    assert!(save(&server, &input).await.status_code().is_client_error());
    let unknown = json!({"memberId":uuid::Uuid::new_v4().to_string()});
    assert_eq!(history(&server, &unknown).await.status_code(), 404);
    assert_eq!(count(&conn).await, 0);
}

#[tokio::test]
async fn corrupt_payloads_or_metadata_are_rejected_without_returning_private_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let candidate = capture(&server, &member).await;
    value(save(&server, &params(&member, &candidate)).await);
    for mutation in [
        "UPDATE hall_execution_snapshot SET snapshot_version = 'invalid'",
        "UPDATE hall_execution_snapshot SET payload = 'private broken \
        fixture'",
    ] {
        conn.execute(Statement::from_string(DbBackend::Sqlite, mutation))
            .await
            .unwrap();
        let response = history(&server, &member).await;
        assert_eq!(response.status_code(), 500);
        assert!(!response.text().contains("private broken fixture"));
        assert!(!response.text().contains("SELECT"));
    }
}

#[tokio::test]
async fn history_is_member_scoped_bounded_and_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let mut saved = Vec::new();
    for n in 0..22 {
        std::fs::write(
            path.join("agent.yaml"),
            format!("schema_version: 1\nmodel: fixture-{n}\n"),
        )
        .unwrap();
        let candidate = capture(&server, &member).await;
        saved.push(value(save(&server, &params(&member, &candidate)).await));
    }
    let rows = value(history(&server, &member).await);
    let expected = saved.iter().rev().take(20).cloned().collect::<Vec<_>>();
    assert_eq!(rows, json!(expected));
    assert_eq!(count(&conn).await, 22);
    let other = value(register(&server, draft(&member_dir(dir.path(), "other"))).await);
    assert_eq!(value(history(&server, &other).await), json!([]));
}
