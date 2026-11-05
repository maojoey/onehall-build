use super::queue::QueueHost;
use crate::db::{migration::Migrator, test_helpers::fresh_disk_db, AppDatabase};
use crate::hall::members_tests::server;
use crate::hall::tasks_tests::{fixture, input, post, value, ReleasedTeam060};
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use sea_orm_migration::{MigrationTrait, MigratorTrait};
use serde_json::json;

pub(crate) struct ReleasedQueue061;
#[async_trait::async_trait]
impl MigratorTrait for ReleasedQueue061 {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Migrator::migrations()
            .into_iter()
            .filter(|m| {
                let n = m.name();
                (n.starts_with('m') && n <= "m20261022_000001_tool_approval")
                    || (n.starts_with("onehall_") && n <= "onehall_m20261025_000003_team_queue")
            })
            .collect()
    }
}
async fn task(
    server: &axum_test::TestServer,
    member: &serde_json::Value,
    snapshot: &serde_json::Value,
) -> String {
    value(post(server, "hall_create_task", input(member, snapshot)).await)["taskId"]
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn queue_intent_is_idempotent_cas_and_has_no_public_launch_route() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let id = task(&server, &m, &s).await;
    let host = QueueHost::new(conn.clone(), 2).unwrap();
    let submission = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        host.enqueue(&id, 1, &submission),
        host.enqueue(&id, 1, &submission)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.attempt_id, b.attempt_id);
    assert!(host.enqueue(&id, 2, &submission).await.is_err());
    assert!(host
        .enqueue(&id, 1, &uuid::Uuid::new_v4().to_string())
        .await
        .is_err());
    let d = value(post(&server, "hall_get_task", json!({"taskId":id})).await);
    assert_eq!(d["status"], "queued");
    assert_eq!(d["launchAllowed"], false);
    assert_eq!(d["runtimeVerified"], false);
    assert_eq!(d["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(d["attempts"][0]["status"], "queued");
    assert!(d["attempts"][0]["conversationId"].is_null());
    assert_eq!(d["history"][0]["action"], "task_queued");
    assert!(!d.to_string().contains("hostEpoch"));
    assert!(!d.to_string().contains("claimId"));
    assert!(post(&server, "hall_enqueue_task", json!({"taskId":id}))
        .await
        .status_code()
        .is_server_error());
    assert_eq!(
        post(
            &server,
            "hall_cancel_task",
            json!({"taskId":id,"expectedRevision":2})
        )
        .await
        .status_code(),
        409
    );
}

#[tokio::test]
async fn concurrent_claims_serialize_members_and_respect_global_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let (a, sa) = fixture(&server, dir.path(), "a").await;
    let (b, sb) = fixture(&server, dir.path(), "b").await;
    let (c, sc) = fixture(&server, dir.path(), "c").await;
    let host = QueueHost::new(conn, 2).unwrap();
    let mut ids = Vec::new();
    for (m, s) in [(&a, &sa), (&a, &sa), (&b, &sb), (&c, &sc)] {
        let id = task(&server, m, s).await;
        host.enqueue(&id, 1, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        ids.push(id);
    }
    let (x, y, z) = tokio::join!(host.claim_next(), host.claim_next(), host.claim_next());
    let claims = [x.unwrap(), y.unwrap(), z.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 2);
    assert_ne!(claims[0].attempt_id(), claims[1].attempt_id());
    let mut owners = std::collections::HashSet::new();
    for claim in claims {
        assert!(host.is_current(&claim).await.unwrap());
        let d = value(post(&server, "hall_get_task", json!({"taskId":claim.task_id()})).await);
        owners.insert(d["memberId"].as_str().unwrap().to_string());
        assert_eq!(d["attempts"][0]["status"], "starting");
        assert!(d["attempts"][0]["startedAt"].is_null());
    }
    assert_eq!(owners.len(), 2);
    assert!(host.claim_next().await.unwrap().is_none());
    assert!(!ids.is_empty());
}

#[tokio::test]
async fn restart_marks_unknown_claims_for_review_without_replaying_or_freeing_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let (m, s) = fixture(&server, dir.path(), "a").await;
    let first = task(&server, &m, &s).await;
    let later = task(&server, &m, &s).await;
    let host = QueueHost::new(conn.clone(), 1).unwrap();
    for id in [&first, &later] {
        host.enqueue(id, 1, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
    }
    let old = host.claim_next().await.unwrap().unwrap();
    assert_eq!(old.task_id(), first);
    let reboot = QueueHost::new(conn.clone(), 1).unwrap();
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 1);
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 0);
    assert!(!host.is_current(&old).await.unwrap());
    assert!(!reboot.is_current(&old).await.unwrap());
    assert!(reboot.claim_next().await.unwrap().is_none());
    let interrupted = value(post(&server, "hall_get_task", json!({"taskId":first})).await);
    assert_eq!(interrupted["status"], "interrupted");
    assert!(interrupted["attempts"][0]["finishedAt"].is_null());
    let waiting = value(post(&server, "hall_get_task", json!({"taskId":later})).await);
    assert_eq!(waiting["status"], "queued");
    assert_eq!(waiting["attempts"][0]["status"], "queued");
    drop(server);
    drop(host);
    drop(reboot);
    conn.close().await.unwrap();
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("source.db").display());
    let db = Database::connect(url).await.unwrap();
    let again = QueueHost::new(db.clone(), 1).unwrap();
    assert_eq!(again.reconcile_previous_boots().await.unwrap(), 0);
    assert!(again.claim_next().await.unwrap().is_none());
    let count = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM hall_task_attempt",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn queue_upgrade_60_to_61_keeps_drafts_and_marks_new_backup_epoch() {
    use crate::commands::backup::{core, sections::LiveRoots};
    use crate::web::event_bridge::EventEmitter;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("queue.db").display());
    let conn = Database::connect(&url).await.unwrap();
    ReleasedTeam060::up(&conn, None).await.unwrap();
    assert_eq!(
        ReleasedTeam060::get_applied_migrations(&conn)
            .await
            .unwrap()
            .len(),
        60
    );
    let server = server(AppDatabase { conn: conn.clone() }, dir.path());
    let (m, s) = fixture(&server, dir.path(), "member").await;
    let id = task(&server, &m, &s).await;
    let before = value(post(&server, "hall_get_task", json!({"taskId":id})).await);
    ReleasedQueue061::up(&conn, None).await.unwrap();
    ReleasedQueue061::up(&conn, None).await.unwrap();
    assert_eq!(
        ReleasedQueue061::get_applied_migrations(&conn)
            .await
            .unwrap()
            .len(),
        61
    );
    assert_eq!(
        value(post(&server, "hall_get_task", json!({"taskId":id})).await),
        before
    );
    let manifest = core::create_backup_core(
        core::BackupInputs {
            conn: &conn,
            data_dir: dir.path(),
            live_roots: LiveRoots::rooted_at(&dir.path().join("empty")),
            app_version: "0.39.0",
            runtime_label: "server",
        },
        core::BackupOptions::default(),
        &dir.path().join("queue.zip"),
        &EventEmitter::Noop,
        "queue-fixture",
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    // The current backup writer truthfully stamps its compiled epoch.
    assert_eq!(
        manifest.latest_migration,
        Migrator::migrations().last().unwrap().name()
    );
    assert!(!ReleasedTeam060::migrations()
        .iter()
        .any(|m| m.name() == manifest.latest_migration));
    assert!(core::evaluate_compat(&manifest).0);
    assert!(ReleasedTeam060::get_pending_migrations(&conn)
        .await
        .is_err());
}
