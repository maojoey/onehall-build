//! Execution mode: legacy compatibility, frozen capabilities and forgery.
//! Disposable fixtures only; no engine, native sandbox or user configuration.
use super::members_snapshot_tests::{capture, params, save};
use super::members_tests::{draft, member_dir, preset, register, server as router, TEST_AUTH};
use crate::db::entities::assistant;
use crate::db::test_helpers::fresh_in_memory_db;
use crate::hall::members::{ExecutionMode, LaunchProfile};
use axum_test::TestServer;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn value(response: axum_test::TestResponse) -> Value {
    assert_eq!(response.status_code(), 200, "{}", response.text());
    response.json()
}
async fn observe(server: &TestServer, member: &Value) -> Value {
    value(
        server
            .post("/api/hall_preview_member_config")
            .add_header("authorization", TEST_AUTH)
            .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"]}))
            .await,
    )
}
async fn preview(server: &TestServer, member: &Value) -> Value {
    let observed = observe(server, member).await;
    value(server.post("/api/hall_preview_member_execution").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"],"expectedCheckVersion":observed["checkVersion"]})).await)
}
async fn payload(db: &DatabaseConnection) -> String {
    db.query_one(Statement::from_string(
        DbBackend::Sqlite,
        "SELECT payload FROM hall_execution_snapshot",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "payload")
    .unwrap()
}
fn digest(payload: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"onehall-execution-candidate-v1");
    hash.update(payload.as_bytes());
    format!("{:x}", hash.finalize())
}
/// Replace the stored payload and keep its digest column valid, so a failure
/// comes from payload validation and not from the digest comparison.
async fn retarget(db: &DatabaseConnection, payload: &Value) {
    let raw = payload.to_string();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_execution_snapshot SET payload=?, snapshot_version=?",
        vec![raw.clone().into(), digest(&raw).into()],
    ))
    .await
    .unwrap();
}
async fn listed(db: &DatabaseConnection, member: &Value) -> bool {
    crate::hall::members::hall_list_member_snapshots_core(
        db,
        member["memberId"].as_str().unwrap().into(),
    )
    .await
    .is_ok()
}

#[tokio::test]
async fn a_member_that_never_chose_a_mode_keeps_its_exact_snapshot_bytes_and_digest() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = router(db, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let candidate = capture(&server, &member).await;
    // Codex has always been restricted; nothing is claimed as verified.
    let summary = &candidate["summary"];
    assert_eq!(summary["executionMode"], "restricted");
    assert_eq!(summary["capabilities"]["readBoundary"], "unknown");
    assert_eq!(summary["capabilities"]["evidenceId"], Value::Null);
    value(save(&server, &params(&member, &candidate)).await);
    let stored = payload(&conn).await;
    assert!(!stored.contains("execution_mode") && !stored.contains("execution_capabilities"));
    assert_eq!(digest(&stored), summary["snapshotVersion"]);
}

#[tokio::test]
async fn choosing_a_mode_freezes_capabilities_into_a_new_digest_and_launch_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = router(db, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let before = preview(&server, &member).await;
    std::fs::write(
        path.join("agent.yaml"),
        "schema_version: 1\nexecution_mode: local_assistant\n",
    )
    .unwrap();
    let after = preview(&server, &member).await;
    assert_eq!(after["valid"], true, "{after}");
    assert_ne!(
        before["summary"]["snapshotVersion"],
        after["summary"]["snapshotVersion"]
    );
    let caps = &after["summary"]["capabilities"];
    assert_eq!(after["summary"]["executionMode"], "local_assistant");
    for boundary in ["readBoundary", "writeBoundary", "networkBoundary"] {
        assert_eq!(caps[boundary], "not_enforced");
    }
    assert_eq!(caps["hallIdentity"], true);
    let saved = value(save(&server, &params(&member, &after)).await);
    let profile: LaunchProfile = crate::hall::members::snapshot_launch_profile(
        &conn,
        member["memberId"].as_str().unwrap(),
        saved["snapshotId"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(profile.execution_mode, Some(ExecutionMode::LocalAssistant));
    assert_eq!(
        serde_json::to_value(profile.capabilities.unwrap()).unwrap(),
        *caps
    );
}

#[tokio::test]
async fn unknown_conflicting_or_template_supplied_modes_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = router(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    for raw in ["bypass", "auto", "Restricted", "''"] {
        std::fs::write(
            path.join("agent.yaml"),
            format!("schema_version: 1\nexecution_mode: {raw}\n"),
        )
        .unwrap();
        let read = observe(&server, &member).await;
        assert_eq!(read["valid"], false, "{raw}");
        assert!(read["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["field"] == "execution_mode"));
    }
    for (engine, yaml, code) in [
        (
            "claude_code",
            "execution_mode: restricted",
            "unsupported_execution_mode",
        ),
        (
            "codex",
            "execution_mode: local_assistant\nonedesk_key: read",
            "key_not_allowed",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = member_dir(dir.path(), "member");
        std::fs::write(
            path.join("agent.yaml"),
            format!("schema_version: 1\n{yaml}\n"),
        )
        .unwrap();
        let server = router(fresh_in_memory_db().await, dir.path());
        let mut input = draft(&path);
        input["engine"] = json!(engine);
        let member = value(register(&server, input).await);
        let report = preview(&server, &member).await;
        assert_eq!(report["valid"], false);
        assert!(report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == code));
    }
    // A preset can never choose the frozen mode through its option values.
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let row = preset(&db.conn, "codex", true).await;
    let labels = json!([{"id":"execution_mode","kind":"select","name":"m","value_label":"l"}]);
    assistant::Entity::update_many()
        .col_expr(
            assistant::Column::AgentOptions,
            Expr::value(json!({"config_values":{"execution_mode":"local_assistant"}}).to_string()),
        )
        .col_expr(
            assistant::Column::LabelSnapshot,
            Expr::value(json!({"agent":"Codex","options":labels}).to_string()),
        )
        .filter(assistant::Column::Id.eq(row.id))
        .exec(&db.conn)
        .await
        .unwrap();
    let server = router(db, dir.path());
    let mut input = draft(&path);
    input["assistantUid"] = json!(row.uid);
    let member = value(register(&server, input).await);
    let report = preview(&server, &member).await;
    assert_eq!(report["valid"], false, "{report}");
    assert!(report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "template_sets_execution_mode"));
}

#[tokio::test]
async fn forged_stripped_or_swapped_stored_claims_are_not_restored() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let conn = db.conn.clone();
    let server = router(db, dir.path());
    let member = value(register(&server, draft(&path)).await);
    std::fs::write(
        path.join("agent.yaml"),
        "schema_version: 1\nexecution_mode: local_assistant\n",
    )
    .unwrap();
    let candidate = preview(&server, &member).await;
    value(save(&server, &params(&member, &candidate)).await);
    let original: Value = serde_json::from_str(&payload(&conn).await).unwrap();
    // Control: re-encoding the genuine payload through the helper still restores.
    retarget(&conn, &original).await;
    assert!(listed(&conn, &member).await);
    type Tamper = (&'static str, fn(&mut Value));
    let tampered: [Tamper; 6] = [
        ("verified read", |p| {
            p["execution_capabilities"]["readBoundary"] = json!("verified")
        }),
        ("invented evidence", |p| {
            p["execution_capabilities"]["evidenceId"] = json!("invented")
        }),
        ("stripped", |p| {
            p["execution_capabilities"] = Value::Null;
            p.as_object_mut().unwrap().remove("execution_capabilities");
        }),
        ("unknown mode", |p| {
            p["declaration"]["execution_mode"] = json!("bypass")
        }),
        ("swapped to restricted", |p| {
            p["declaration"]["execution_mode"] = json!("restricted")
        }),
        ("unknown capability field", |p| {
            p["execution_capabilities"]["sandbox"] = json!("verified")
        }),
    ];
    for (name, change) in tampered {
        let mut forged = original.clone();
        change(&mut forged);
        retarget(&conn, &forged).await;
        assert!(!listed(&conn, &member).await, "{name}");
    }
}
