//! Stock Codeg 0.40 contract fixture, deliberately WITHOUT any OneHall tables.
//! Uses the 55 byte-identical migrations of official 95fd81d, never relabels
//! a current-schema backup as an old one. No real DB, home or CLI directories.
use crate::{
    commands::backup::{
        archive, core,
        manifest::{BackupManifest, BACKUP_FORMAT_VERSION, BACKUP_KIND},
        restore,
        sections::LiveRoots,
    },
    db::migration::Migrator,
    models::agent::AgentType,
    parsers::{AgentParser, ExternalSource},
    web::event_bridge::EventEmitter,
};
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use sea_orm_migration::{MigrationTrait, MigratorTrait};
use serde_json::json;
use std::path::Path;
struct Original040;
#[async_trait::async_trait]
impl MigratorTrait for Original040 {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Migrator::migrations()
            .into_iter()
            .filter(|m| m.name().starts_with('m'))
            .collect()
    }
}
async fn seed(
    db: &sea_orm::DatabaseConnection,
    dir: &Path,
    name: &str,
    agent: AgentType,
    ext: &str,
) -> i32 {
    let folder =
        crate::db::service::folder_service::ensure_folder_for_path(db, dir.to_str().unwrap())
            .await
            .unwrap();
    let id = crate::commands::conversations::create_conversation_core(
        db,
        folder.id,
        agent,
        Some(name.into()),
    )
    .await
    .unwrap();
    crate::db::service::conversation_service::bind_external_id(db, id, ext, &[])
        .await
        .unwrap();
    id
}
#[tokio::test]
async fn stock_040_full_restore_replaces_index_and_preserves_native_cli_and_acp_text_on_copies() {
    use crate::acp_transcript::{EntryKind, TranscriptEntry, TranscriptHeader};
    use tokio_util::sync::CancellationToken;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let source = Database::connect(format!(
        "sqlite:{}?mode=rwc",
        root.join("stock.db").display()
    ))
    .await
    .unwrap();
    Original040::up(&source, None).await.unwrap();
    assert_eq!(
        Original040::get_applied_migrations(&source)
            .await
            .unwrap()
            .len(),
        55
    );
    assert!(source
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE name='hall_member'"
        ))
        .await
        .unwrap()
        .is_none());
    let native_id = uuid::Uuid::new_v4().to_string();
    let native = seed(
        &source,
        &project,
        "stock native",
        AgentType::Codex,
        &native_id,
    )
    .await;
    let custom = AgentType::custom("stock040-acp").unwrap();
    let acp = seed(&source, &project, "stock ACP", custom, "stock-acp-session").await;
    let native_file = root.join(format!("rollout-2026-10-26T10-00-00-{native_id}.jsonl"));
    let native_body=[json!({"timestamp":"2026-10-26T10:00:00Z","type":"session_meta","payload":{"id":native_id,"cwd":project}}),json!({"timestamp":"2026-10-26T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"stock-native-body"} ]}}),json!({"timestamp":"2026-10-26T10:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"stock-native-result"}]}})].iter().map(|x|format!("{x}\n")).collect::<String>();
    std::fs::write(&native_file, &native_body).unwrap();
    let managed = root.join("stock-managed");
    let acp_root = managed.join("acp-transcripts");
    crate::acp_transcript::append_line_in(
        &acp_root,
        "stock040-acp",
        "stock-acp-session",
        &serde_json::to_string(&TranscriptHeader::new(
            "custom:stock040-acp",
            "stock-acp-session",
            project.to_str().unwrap(),
            1_790_000_000_000,
        ))
        .unwrap(),
    );
    for (k, p) in [
        (
            EntryKind::Prompt,
            json!([{"type":"text","text":"stock-acp-body"}]),
        ),
        (
            EntryKind::Update,
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"stock-acp-result"}}),
        ),
    ] {
        crate::acp_transcript::append_line_in(
            &acp_root,
            "stock040-acp",
            "stock-acp-session",
            &serde_json::to_string(&TranscriptEntry {
                t: 1_790_000_000_001,
                k,
                p,
                steer: false,
            })
            .unwrap(),
        );
    }
    let snapshot = root.join("snapshot.db");
    core::snapshot_db_to(&source, &snapshot).await.unwrap();
    let saved_index = root.join("session_index.jsonl");
    std::fs::write(
        &saved_index,
        format!(
            "{}\n",
            json!({"id":native_id,"thread_name":"stock native","updated_at":"2026-10-26T10:00:00Z"})
        ),
    )
    .unwrap();
    let zip = root.join("original040-contract.zip");
    let mut writer = archive::ArchiveBuilder::create(&zip).unwrap();
    let cancel = CancellationToken::new();
    let mut progress = archive::null_progress();
    writer
        .add_file("db/codeg.db", &snapshot, &cancel, &mut progress)
        .unwrap();
    writer
        .add_dir(
            "acp-transcripts",
            &acp_root,
            &|_| false,
            &cancel,
            &mut progress,
        )
        .unwrap();
    writer
        .add_file(
            &format!(
                "external/codex/{}",
                native_file.file_name().unwrap().to_str().unwrap()
            ),
            &native_file,
            &cancel,
            &mut progress,
        )
        .unwrap();
    writer
        .add_file(
            "external/codex-session-index/session_index.jsonl",
            &saved_index,
            &cancel,
            &mut progress,
        )
        .unwrap();
    let manifest = writer
        .finish(BackupManifest {
            format_version: BACKUP_FORMAT_VERSION,
            kind: BACKUP_KIND.into(),
            created_at: "2026-10-26T10:00:00Z".into(),
            app_version: "0.40.0".into(),
            latest_migration: Original040::migrations().last().unwrap().name().into(),
            runtime: "desktop".into(),
            includes_external_transcripts: true,
            includes_secrets: true,
            managed_sections: Some(vec!["acp-transcripts".into(), "uploads".into()]),
            degraded_sqlite: vec![],
            entries: vec![],
        })
        .unwrap();
    assert_eq!(manifest.latest_migration, "m20261022_000001_tool_approval");
    assert!(core::evaluate_compat(&manifest).0);
    assert!(manifest
        .entries
        .iter()
        .any(|e| e.path.starts_with("acp-transcripts/")));
    assert!(manifest
        .entries
        .iter()
        .any(|e| e.path.starts_with("external/codex/")));
    // Existing destination is NEW disposable data. Demonstrate replacement, not
    // merge, and that the safety snapshot retains the prior destination state.
    let data = root.join("destination-data");
    let live = root.join("destination-managed");
    std::fs::create_dir_all(&data).unwrap();
    let target_path = data.join(crate::db::database_file_name());
    let target = Database::connect(format!("sqlite:{}?mode=rwc", target_path.display()))
        .await
        .unwrap();
    Migrator::up(&target, None).await.unwrap();
    seed(
        &target,
        &project,
        "destination-only",
        AgentType::Codex,
        "old-only",
    )
    .await;
    target.close().await.unwrap();
    std::fs::create_dir_all(live.join("acp-transcripts")).unwrap();
    std::fs::write(live.join("acp-transcripts/destination-only.jsonl"), "old").unwrap();
    restore::stage_restore_core(
        &zip,
        &data,
        &EventEmitter::Noop,
        "original040-copy",
        &cancel,
    )
    .await
    .unwrap();
    let staged = data.join(restore::STAGING_DIR).join("original040-copy");
    let native_home = root.join("destination-codex");
    let sources = vec![
        ExternalSource {
            agent: "codex",
            root: native_home.join("sessions"),
            is_file: false,
            sqlite: false,
            include_top: None,
            indexes: &[],
        },
        ExternalSource {
            agent: "codex-session-index",
            root: native_home.join("session_index.jsonl"),
            is_file: true,
            sqlite: false,
            include_top: None,
            indexes: &[],
        },
    ];
    crate::commands::backup::external::restore_external_with_sources(
        &staged.join("external"),
        &sources,
        restore::ConflictPolicy::SkipExisting,
        &cancel,
    )
    .unwrap();
    let outcome =
        restore::apply_pending_restore_with_paths(&data, &LiveRoots::rooted_at(&live)).unwrap();
    let backup = match outcome {
        restore::RestoreApplied::Applied {
            safety_snapshot: Some(path),
        } => path,
        _ => panic!("expected safety snapshot"),
    };
    assert!(backup.join("db/codeg.db").exists());
    assert!(!live.join("acp-transcripts/destination-only.jsonl").exists());
    let restored = Database::connect(format!("sqlite:{}?mode=rwc", target_path.display()))
        .await
        .unwrap();
    Migrator::up(&restored, None).await.unwrap();
    assert_eq!(
        Migrator::get_applied_migrations(&restored)
            .await
            .unwrap()
            .len(),
        Migrator::migrations().len()
    );
    let rows = restored
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id,title,external_id FROM conversation ORDER BY id",
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(
        |r| r.try_get::<Option<String>>("", "title").unwrap().as_deref()
            != Some("destination-only")
    ));
    assert!(rows
        .iter()
        .any(|r| r.try_get::<i32>("", "id").unwrap() == native
            && r.try_get::<Option<String>>("", "external_id")
                .unwrap()
                .as_deref()
                == Some(native_id.as_str())));
    assert!(rows
        .iter()
        .any(|r| r.try_get::<i32>("", "id").unwrap() == acp
            && r.try_get::<Option<String>>("", "external_id")
                .unwrap()
                .as_deref()
                == Some("stock-acp-session")));
    let codex = crate::parsers::codex::CodexParser::with_base_dir(native_home.join("sessions"));
    let native_detail = codex.get_conversation(&native_id).unwrap();
    assert_eq!(native_detail.summary.title.as_deref(), Some("stock native"));
    assert!(serde_json::to_string(&native_detail.turns)
        .unwrap()
        .contains("stock-native-body"));
    assert!(serde_json::to_string(&native_detail.turns)
        .unwrap()
        .contains("stock-native-result"));
    assert_eq!(
        std::fs::read(native_home.join("session_index.jsonl")).unwrap(),
        std::fs::read(saved_index).unwrap()
    );
    let acp_detail =
        crate::parsers::acp_native::AcpNativeParser::new_in(custom, live.join("acp-transcripts"))
            .get_conversation("stock-acp-session")
            .unwrap();
    let text = serde_json::to_string(&acp_detail.turns).unwrap();
    assert!(text.contains("stock-acp-body"));
    assert!(text.contains("stock-acp-result"));
    assert!(restored
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA foreign_key_check"
        ))
        .await
        .unwrap()
        .is_empty());
    let check = restored
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
    let reopened_url = format!("sqlite:{}?mode=rwc", target_path.display());
    restored.close().await.unwrap();
    let again = Database::connect(reopened_url).await.unwrap();
    assert_eq!(
        again
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT id FROM conversation"
            ))
            .await
            .unwrap()
            .len(),
        2
    );
}
