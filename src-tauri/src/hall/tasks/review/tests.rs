use super::*;
use crate::{
    db::{migration::Migrator, test_helpers::fresh_disk_db, AppDatabase},
    hall::{
        members_snapshot_tests::{capture, params, save},
        members_tests::{draft, member_dir, preset, register, server},
        tasks_tests::{fixture, input, post, value},
    },
};
use sea_orm_migration::MigratorTrait;
use serde_json::{json, Value};

async fn template_fixture(
    db: &DatabaseConnection,
    server: &axum_test::TestServer,
    root: &std::path::Path,
) -> (Value, Value, String) {
    let template = preset(db, "codex", true).await;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE assistant SET label_snapshot=? WHERE uid=?",
        vec![
            r#"{"agent":"Codex","options":[]}"#.into(),
            template.uid.clone().into(),
        ],
    ))
    .await
    .unwrap();
    let mut member = draft(&member_dir(root, "template-member"));
    member["assistantUid"] = json!(template.uid);
    let member = value(register(server, member).await);
    let candidate = capture(server, &member).await;
    let snapshot = value(save(server, &params(&member, &candidate)).await);
    (member, snapshot, template.uid)
}

#[tokio::test]
async fn template_review_approval_survives_repeated_reads_reopen_and_runtime_start_checks() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let s = server(db, dir.path());
    let (member, snapshot, template) = template_fixture(&conn, &s, dir.path()).await;
    let task = value(post(&s, "hall_create_task", input(&member, &snapshot)).await);
    let id = task["taskId"].as_str().unwrap();
    let plan = value(post(&s, "hall_review_task", json!({"taskId":id})).await);
    assert!(plan["brief"]
        .as_str()
        .unwrap()
        .contains("template instructions stay here"));
    for _ in 0..3 {
        assert_eq!(
            value(post(&s, "hall_review_task", json!({"taskId":id})).await),
            plan
        );
    }
    let approval = json!({"approval":{"taskId":id,"expectedRevision":1,
        "planHash":plan["planHash"],"submissionId":uuid::Uuid::new_v4().to_string()}});
    let receipt = value(post(&s, "hall_approve_task", approval.clone()).await);
    assert_eq!(
        value(post(&s, "hall_approve_task", approval).await),
        receipt
    );
    // A live template edit cannot rewrite the instructions already reviewed.
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE assistant SET instructions=? WHERE uid=?",
        vec!["different live instructions".into(), template.into()],
    ))
    .await
    .unwrap();
    drop(s);
    conn.close().await.unwrap();
    let reopened = sea_orm::Database::connect(format!(
        "sqlite:{}?mode=rw",
        dir.path().join("source.db").display()
    ))
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(approved_plan(&reopened, id).await.unwrap()).unwrap(),
        plan
    );
    let host = QueueHost::new(reopened.clone(), 1).unwrap();
    let claim = host.claim_runtime(true, "fixture").await.unwrap().unwrap();
    assert_eq!(claim.attempt_id(), receipt["attemptId"].as_str().unwrap());
    // These are the checks used by coordinator plan(), admit() and CliPort::start().
    validate_dispatch(&reopened, claim.attempt_id(), id)
        .await
        .unwrap();
    let approved = approved_plan(&reopened, id).await.unwrap();
    let profile = snapshot_launch_profile(
        &reopened,
        member["memberId"].as_str().unwrap(),
        snapshot["snapshotId"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(approved.brief, profile.brief);
    assert_eq!(serde_json::to_value(approved).unwrap(), plan);
}

#[tokio::test]
async fn template_instruction_changes_and_snapshot_substitution_invalidate_review() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let s = server(db, dir.path());
    let (member, snapshot, template) = template_fixture(&conn, &s, dir.path()).await;
    let task = value(post(&s, "hall_create_task", input(&member, &snapshot)).await);
    let id = task["taskId"].as_str().unwrap();
    let original = preview(&conn, id).await.unwrap();
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE assistant SET instructions=? WHERE uid=?",
        vec!["changed frozen instructions".into(), template.into()],
    ))
    .await
    .unwrap();
    let candidate = capture(&s, &member).await;
    let changed = value(save(&s, &params(&member, &candidate)).await);
    assert_ne!(changed["snapshotId"], snapshot["snapshotId"]);
    assert_ne!(
        changed["summary"]["snapshotVersion"],
        snapshot["summary"]["snapshotVersion"]
    );
    // Simulate a stale or corrupted task reference without changing its revision.
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_task SET snapshot_id=?,snapshot_version=? WHERE task_id=?",
        vec![
            changed["snapshotId"].as_str().unwrap().into(),
            changed["summary"]["snapshotVersion"]
                .as_str()
                .unwrap()
                .into(),
            id.into(),
        ],
    ))
    .await
    .unwrap();
    assert_eq!(
        post(
            &s,
            "hall_approve_task",
            json!({"approval":{"taskId":id,
        "expectedRevision":1,"planHash":original.plan_hash,
        "submissionId":uuid::Uuid::new_v4().to_string()}})
        )
        .await
        .status_code(),
        409
    );
    let changed_plan = preview(&conn, id).await.unwrap();
    assert_ne!(changed_plan.plan_hash, original.plan_hash);
    assert!(changed_plan.brief.contains("changed frozen instructions"));
    let receipt = approve(
        &conn,
        Approval {
            task_id: id.into(),
            expected_revision: 1,
            plan_hash: changed_plan.plan_hash,
            submission_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .await
    .unwrap();
    validate_dispatch(&conn, &receipt.attempt_id, id)
        .await
        .unwrap();
    // Reusing the snapshot UUID cannot conceal changes to approved instructions.
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_task_review SET plan_json=replace(plan_json,?,?) WHERE attempt_id=?",
        vec![
            "changed frozen instructions".into(),
            "unreviewed instructions".into(),
            receipt.attempt_id.into(),
        ],
    ))
    .await
    .unwrap();
    assert!(approved_plan(&conn, id).await.is_err());
}
pub(crate) struct ReleasedExit064;
#[async_trait::async_trait]
impl MigratorTrait for ReleasedExit064 {
    fn migrations() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
        Migrator::migrations()
            .into_iter()
            .filter(|m| {
                let n = m.name();
                (n.starts_with('m') && n <= "m20261022_000001_tool_approval")
                    || (n.starts_with("onehall_")
                        && n <= "onehall_m20261025_000006_team_exit_evidence")
            })
            .collect()
    }
}
#[tokio::test]
async fn reviewed_enqueue_is_atomic_idempotent_and_not_admission() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let s = server(db, dir.path());
    let (m, snap) = fixture(&s, dir.path(), "review").await;
    let id = value(post(&s, "hall_create_task", input(&m, &snap)).await)["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    let p = preview(&conn, &id).await.unwrap();
    assert!(!p.real_execution_allowed);
    assert_eq!(p.cwd, m["cwd"].as_str().unwrap());
    let submission = uuid::Uuid::new_v4().to_string();
    let body = json!({"approval":{"taskId":id,"expectedRevision":1,"planHash":p.plan_hash,"submissionId":submission}});
    let a = value(post(&s, "hall_approve_task", body.clone()).await);
    let b = value(post(&s, "hall_approve_task", body.clone()).await);
    assert_eq!(a, b);
    let detail = super::super::get_task(&conn, id.clone()).await.unwrap();
    assert_eq!(detail.attempts.len(), 1);
    assert!(detail.attempts[0].started_at.is_none());
    validate_dispatch(&conn, a["attemptId"].as_str().unwrap(), &id)
        .await
        .unwrap();
    let mut bad = body;
    bad["approval"]["planHash"] = json!("0".repeat(64));
    assert_eq!(post(&s, "hall_approve_task", bad).await.status_code(), 409);
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "UPDATE hall_member SET revision=revision+1",
    ))
    .await
    .unwrap();
    assert!(
        validate_dispatch(&conn, a["attemptId"].as_str().unwrap(), &id)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn changed_task_or_scope_cannot_reuse_human_review_and_no_partial_queue_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let s = server(db, dir.path());
    let (m, snap) = fixture(&s, dir.path(), "review").await;
    let id = value(post(&s, "hall_create_task", input(&m, &snap)).await)["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    let p = preview(&conn, &id).await.unwrap();
    value(post(&s,"hall_edit_task",json!({"taskId":id,"expectedRevision":1,"draft":{"title":"changed","description":"changed"}})).await);
    assert!(approve(
        &conn,
        Approval {
            task_id: id.clone(),
            expected_revision: 2,
            plan_hash: p.plan_hash,
            submission_id: uuid::Uuid::new_v4().to_string()
        }
    )
    .await
    .is_err());
    assert!(super::super::get_task(&conn, id)
        .await
        .unwrap()
        .attempts
        .is_empty());
}
#[tokio::test]
async fn pm_proposal_is_not_human_approval_but_user_can_review_existing_queue() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let s = server(db, dir.path());
    let (m, snap) = fixture(&s, dir.path(), "review").await;
    let id = value(post(&s, "hall_create_task", input(&m, &snap)).await)["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    let host = QueueHost::new(conn.clone(), 2).unwrap();
    let old = host
        .enqueue(&id, 1, &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert!(validate_dispatch(&conn, &old.attempt_id, &id)
        .await
        .is_err());
    assert!(host.claim_runtime(true, "fixture").await.unwrap().is_none());
    let p = preview(&conn, &id).await.unwrap();
    let submission = uuid::Uuid::new_v4().to_string();
    let a = Approval {
        task_id: id.clone(),
        expected_revision: 2,
        plan_hash: p.plan_hash.clone(),
        submission_id: submission.clone(),
    };
    let receipt = approve(&conn, a).await.unwrap();
    assert_eq!(receipt.attempt_id, old.attempt_id);
    let replay = approve(
        &conn,
        Approval {
            task_id: id.clone(),
            expected_revision: 2,
            plan_hash: p.plan_hash,
            submission_id: submission,
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.task_revision, replay.task_revision);
    validate_dispatch(&conn, &receipt.attempt_id, &id)
        .await
        .unwrap();
    assert!(host.claim_runtime(true, "fixture").await.unwrap().is_some());
}
#[tokio::test]
async fn review_routes_require_authentication_and_reject_model_identity_fields() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let s = server(db, dir.path());
    for command in [
        "hall_review_task",
        "hall_approve_task",
        "hall_cancel_queued_task",
    ] {
        assert_eq!(
            s.post(&format!("/api/{command}"))
                .json(&json!({}))
                .await
                .status_code(),
            401
        );
    }
    assert_eq!(post(&s,"hall_approve_task",json!({"approval":{"taskId":"x","expectedRevision":1,"planHash":"x","submissionId":"x","actor":"pm"}})).await.status_code(),422);
}
#[tokio::test]
async fn exact_64_upgrade_and_review_backup_refuse_old_writer_without_discarding_history() {
    use crate::commands::backup::{core, sections::LiveRoots};
    use crate::web::event_bridge::EventEmitter;
    let dir = tempfile::tempdir().unwrap();
    let url = format!(
        "sqlite:{}?mode=rwc",
        dir.path().join("upgrade.db").display()
    );
    let db = sea_orm::Database::connect(&url).await.unwrap();
    ReleasedExit064::up(&db, None).await.unwrap();
    assert_eq!(
        ReleasedExit064::get_applied_migrations(&db)
            .await
            .unwrap()
            .len(),
        64
    );
    let s = server(AppDatabase { conn: db.clone() }, dir.path());
    let (m, snap) = fixture(&s, dir.path(), "review").await;
    let id = value(post(&s, "hall_create_task", input(&m, &snap)).await)["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    Migrator::up(&db, None).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        Migrator::get_applied_migrations(&db).await.unwrap().len(),
        Migrator::migrations().len()
    );
    let p = preview(&db, &id).await.unwrap();
    let a = approve(
        &db,
        Approval {
            task_id: id.clone(),
            expected_revision: 1,
            plan_hash: p.plan_hash,
            submission_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .await
    .unwrap();
    let backup = core::create_backup_core(
        core::BackupInputs {
            conn: &db,
            data_dir: dir.path(),
            live_roots: LiveRoots::rooted_at(&dir.path().join("empty")),
            app_version: "0.39.0",
            runtime_label: "server",
        },
        core::BackupOptions::default(),
        &dir.path().join("review.zip"),
        &EventEmitter::Noop,
        "review-fixture",
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    // Backups record the latest schema so older writers cannot restore it.
    assert_eq!(
        backup.latest_migration,
        Migrator::migrations().last().unwrap().name()
    );
    assert!(core::evaluate_compat(&backup).0);
    assert!(ReleasedExit064::get_pending_migrations(&db).await.is_err());
    let rollback_to_review = Migrator::migrations()
        .iter()
        .rev()
        .position(|m| m.name() == "onehall_m20261025_000007_team_launch_review")
        .unwrap() as u32
        + 1;
    assert!(Migrator::down(&db, Some(rollback_to_review)).await.is_err());
    assert!(db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA foreign_key_check"
        ))
        .await
        .unwrap()
        .is_empty());
    drop(s);
    db.close().await.unwrap();
    let again = sea_orm::Database::connect(&url).await.unwrap();
    validate_dispatch(&again, &a.attempt_id, &id).await.unwrap();
    assert_eq!(
        super::super::get_task(&again, id)
            .await
            .unwrap()
            .attempts
            .len(),
        1
    );
}
