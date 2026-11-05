//! Upgrade fixtures only. No real database, engine, token or user directory.
use super::members_tests::{draft, list, member_dir, register, server, TEST_AUTH};
use crate::commands::backup::{core, sections::LiveRoots};
use crate::db::test_helpers::seed_conversation;
use crate::db::{
    migration::Migrator,
    service::{folder_service, tool_approval_service},
    AppDatabase,
};
use crate::models::agent::AgentType;
use crate::web::event_bridge::EventEmitter;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use sea_orm_migration::{MigrationTrait, MigratorTrait};
use serde_json::{json, Value};

/// Exact released migration range: 0.40 added one upstream migration, and
/// OneHall adds a new final epoch. Earlier migration bodies were not changed.
struct Released039;
#[async_trait::async_trait]
impl MigratorTrait for Released039 {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Migrator::migrations()
            .into_iter()
            .filter(|m| {
                let name = m.name();
                (name.starts_with('m') && name <= "m20261020_000001_folder_command_run_local")
                    || (name.starts_with("onehall_")
                        && name <= "onehall_m20261024_000001_execution_snapshot")
            })
            .collect()
    }
}

/// Frozen 0.40 compatibility epoch, before the separate team-ledger migration.
pub(super) struct Released040;
#[async_trait::async_trait]
impl MigratorTrait for Released040 {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Migrator::migrations()
            .into_iter()
            .filter(|m| {
                let name = m.name();
                (name.starts_with('m') && name <= "m20261022_000001_tool_approval")
                    || (name.starts_with("onehall_")
                        && name <= "onehall_m20261025_000001_upstream040")
            })
            .collect()
    }
}

#[tokio::test]
async fn upstream_040_upgrade_preserves_members_snapshots_and_approval_history_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fixture.db");
    let url = format!("sqlite:{}?mode=rwc", file.display());
    let conn = Database::connect(&url).await.unwrap();
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "PRAGMA foreign_keys=ON",
    ))
    .await
    .unwrap();
    Released039::up(&conn, None).await.unwrap();
    let old_names = Released039::get_applied_migrations(&conn).await.unwrap();
    assert_eq!(old_names.len(), 57);
    let path = member_dir(dir.path(), "member");
    let handwritten = "# preserve this fixture comment\nschema_version: 1\nmodel: fixture-model\n";
    std::fs::write(path.join("agent.yaml"), handwritten).unwrap();
    let project = folder_service::add_folder(&conn, "/fixture/project")
        .await
        .unwrap();
    let server = server(AppDatabase { conn: conn.clone() }, dir.path());
    let mut input = draft(&path);
    input["projectId"] = json!(project.id);
    let created = register(&server, input).await;
    assert_eq!(created.status_code(), 200);
    let member: Value = created.json();
    let observed: Value = server
        .post("/api/hall_preview_member_config")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"]}))
        .await
        .json();
    let candidate: Value = server.post("/api/hall_preview_member_execution").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"],"expectedCheckVersion":observed["checkVersion"]})).await.json();
    assert_eq!(candidate["valid"], true);
    let saved = server.post("/api/hall_save_member_snapshot").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"],"expectedCheckVersion":observed["checkVersion"],"expectedSnapshotVersion":candidate["summary"]["snapshotVersion"]})).await;
    assert_eq!(saved.status_code(), 200);
    let saved: Value = saved.json();
    let payload = conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT payload FROM hall_execution_snapshot",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "payload")
        .unwrap();

    Released040::up(&conn, None).await.unwrap();
    Released040::up(&conn, None).await.unwrap();
    let applied = Released040::get_applied_migrations(&conn).await.unwrap();
    assert_eq!(applied.len(), 59);
    assert_eq!(list(&server).await, vec![member.clone()]);
    let restored = super::members::hall_list_member_snapshots_core(
        &conn,
        member["memberId"].as_str().unwrap().into(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(restored).unwrap(),
        json!([saved.clone()])
    );
    let after = conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT payload FROM hall_execution_snapshot",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "payload")
        .unwrap();
    assert_eq!(after, payload);
    assert_eq!(
        std::fs::read_to_string(path.join("agent.yaml")).unwrap(),
        handwritten
    );

    let db = AppDatabase { conn: conn.clone() };
    let conversation = seed_conversation(&db, project.id, AgentType::Codex).await;
    let approval = crate::acp::types::ToolApprovalInfo {
        tool_call_id: "fixture-call".into(),
        option_kind: "allow_once".into(),
        option_name: "Approve fixture once".into(),
        origin: "user".into(),
        decided_at: chrono::Utc::now(),
    };
    tool_approval_service::record(&conn, conversation, &approval)
        .await
        .unwrap();
    tool_approval_service::record(&conn, conversation, &approval)
        .await
        .unwrap();
    assert_eq!(
        tool_approval_service::list_by_conversation(&conn, conversation)
            .await
            .unwrap(),
        vec![approval.clone()]
    );
    assert!(Released039::get_pending_migrations(&conn).await.is_err());
    drop(server);
    drop(db);
    conn.close().await.unwrap();
    let reopened = Database::connect(&url).await.unwrap();
    Released040::up(&reopened, None).await.unwrap();
    assert_eq!(
        tool_approval_service::list_by_conversation(&reopened, conversation)
            .await
            .unwrap(),
        vec![approval]
    );
    let restored = super::members::hall_list_member_snapshots_core(
        &reopened,
        member["memberId"].as_str().unwrap().into(),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), json!([saved]));
    let check = reopened
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA quick_check",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert_eq!(check, "ok");
}

#[tokio::test]
async fn upstream_040_backup_manifest_is_refused_by_the_released_039_compatibility_gate() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!(
        "sqlite:{}?mode=rwc",
        dir.path().join("released040.db").display()
    );
    let conn = Database::connect(url).await.unwrap();
    Released040::up(&conn, None).await.unwrap();
    let db = AppDatabase { conn };
    let live = dir.path().join("empty-managed-sections");
    let manifest = core::create_backup_core(
        core::BackupInputs {
            conn: &db.conn,
            data_dir: dir.path(),
            live_roots: LiveRoots::rooted_at(&live),
            app_version: "0.39.0",
            runtime_label: "server",
        },
        core::BackupOptions::default(),
        &dir.path().join("fixture-backup.zip"),
        &EventEmitter::Noop,
        "upstream040-fixture",
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    // The current writer always stamps its compiled latest epoch, even when
    // given a historical DB fixture. Do not pretend it is a released writer.
    assert_eq!(
        manifest.latest_migration,
        Migrator::migrations().last().unwrap().name()
    );
    // The actual 0.39 guard accepts archive format <= 2 and a known final
    // migration. Assert on a real generated manifest, not a fabricated label.
    let old_accepts = manifest.format_version <= 2
        && Released039::migrations()
            .iter()
            .any(|m| m.name() == manifest.latest_migration);
    assert!(
        !old_accepts,
        "a 0.39 reader must reject the new-schema backup before importing it"
    );
    assert!(core::evaluate_compat(&manifest).0);
    // Manifest-only historical writer contract; the exact 57→59 DB upgrade
    // remains independently exercised above, and the team test uses a real
    // newly generated archive to check the 0.40→team boundary.
    let frozen040 = crate::commands::backup::manifest::BackupManifest {
        latest_migration: "onehall_m20261025_000001_upstream040".into(),
        ..manifest
    };
    assert!(!Released039::migrations()
        .iter()
        .any(|m| m.name() == frozen040.latest_migration));
    assert!(core::evaluate_compat(&frozen040).0);
    let old_backup = crate::commands::backup::manifest::BackupManifest {
        latest_migration: "onehall_m20261024_000001_execution_snapshot".into(),
        ..frozen040
    };
    assert!(
        core::evaluate_compat(&old_backup).0,
        "the new reader must still accept 0.39 backups"
    );
}
