//! OneHall's registry only: real authenticated router + disposable SQLite and
//! directories. No engines, user config, native sessions or production data.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app_state::AppState;
use crate::db::entities::{assistant, folder};
use crate::db::migration::Migrator;
use crate::db::service::folder_service;
use crate::db::test_helpers::{fresh_disk_db, fresh_in_memory_db};
use crate::db::AppDatabase;
use crate::web::router::build_router;
use crate::web::shutdown::ShutdownSignal;
use axum_test::{TestResponse, TestServer};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use sea_orm_migration::MigratorTrait;
use serde_json::{json, Value};

pub(super) const TEST_AUTH: &str = "Bearer hall-registry-test-only";

pub(super) fn server(db: AppDatabase, dir: &Path) -> TestServer {
    let state = Arc::new(AppState::new_for_test(db, dir.to_path_buf()));
    TestServer::new(build_router(
        state,
        "hall-registry-test-only".to_owned(),
        dir.join("static"),
        Arc::new(ShutdownSignal::new()),
    ))
    .expect("in-process router")
}

pub(super) fn member_dir(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir(&path).unwrap();
    // macOS's /var and /tmp can themselves be symlinks: supply their real
    // paths, exactly as a directory preview must do before registration.
    path.canonicalize().unwrap()
}

pub(super) fn draft(path: &Path) -> Value {
    json!({"name": "审查员", "role": "代码审查", "cwd": path})
}

pub(super) async fn register(server: &TestServer, draft: Value) -> TestResponse {
    server
        .post("/api/hall_register_member")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({"draft": draft}))
        .await
}

pub(super) async fn list(server: &TestServer) -> Vec<Value> {
    let response = server
        .post("/api/hall_list_members")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 200, "{}", response.text());
    response.json()
}

pub(super) async fn preset(
    db: &DatabaseConnection,
    engine: &str,
    enabled: bool,
) -> assistant::Model {
    let now = chrono::Utc::now();
    assistant::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        name: Set(uuid::Uuid::new_v4().to_string()),
        description: Set(String::new()),
        avatar: Set("{}".to_owned()),
        agent_type: Set(engine.to_owned()),
        agent_options: Set("{}".to_owned()),
        label_snapshot: Set("{}".to_owned()),
        instructions: Set("template instructions stay here".to_owned()),
        enabled: Set(enabled),
        sort_order: Set(0),
        revision: Set(7),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

#[tokio::test]
async fn registry_routes_require_the_existing_api_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    for route in [
        "/api/hall_register_member",
        "/api/hall_list_members",
        "/api/hall_update_member",
        "/api/hall_list_member_projects",
        "/api/hall_preview_member_config",
    ] {
        let missing = server.post(route).json(&json!({})).await;
        assert_eq!(missing.status_code(), 401);
        let wrong = server
            .post(route)
            .add_header("authorization", "Bearer incorrect-test-value")
            .json(&json!({}))
            .await;
        assert_eq!(wrong.status_code(), 401);
    }
    assert!(list(&server).await.is_empty());
}

#[tokio::test]
async fn two_members_share_a_template_but_keep_distinct_identities_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_in_memory_db().await;
    let template = preset(&db.conn, "codex", true).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let mut registered = Vec::new();
    for name in ["review-a", "review-b"] {
        let path = member_dir(dir.path(), name);
        let files = ["AGENTS.md", "memory.md", "agent.yaml"];
        for file in files {
            std::fs::write(path.join(file), format!("private {name}/{file}")).unwrap();
        }
        let mut input = draft(&path);
        input["assistantUid"] = json!(template.uid);
        input["name"] = json!(" 审查员 ");
        let response = register(&server, input).await;
        assert_eq!(response.status_code(), 200, "{}", response.text());
        let member: Value = response.json();
        assert!(uuid::Uuid::parse_str(member["memberId"].as_str().unwrap()).is_ok());
        assert_eq!(member["name"], "审查员");
        assert_eq!(member["cwd"], json!(path));
        assert_eq!(member["engine"], "codex");
        assert_eq!(member["enabled"], false);
        assert_eq!(member["revision"], 1);
        assert_eq!(member["assistantUid"], template.uid);
        assert_eq!(member["assistantRevision"], 7);
        for file in files {
            assert_eq!(
                std::fs::read_to_string(path.join(file)).unwrap(),
                format!("private {name}/{file}")
            );
        }
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), files.len());
        registered.push(member);
    }
    assert_ne!(registered[0]["memberId"], registered[1]["memberId"]);
    let members = list(&server).await;
    assert_eq!(members.len(), 2);
    for member in registered {
        assert!(members.contains(&member));
    }
    let unchanged = assistant::Entity::find_by_id(template.id)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged, template);
}

#[tokio::test]
async fn concurrent_registration_of_one_directory_never_creates_two_members() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_disk_db(dir.path()).await, dir.path());
    let (a, b) = tokio::join!(
        register(&server, draft(&path)),
        register(&server, draft(&path))
    );
    let mut codes = [a.status_code().as_u16(), b.status_code().as_u16()];
    codes.sort();
    assert_eq!(codes, [200, 409]);
    let rejected = if a.status_code() == 409 { a } else { b };
    assert_eq!(rejected.json::<Value>()["code"], "already_exists");
    let again = register(&server, draft(&path.join("."))).await;
    assert_eq!(again.status_code(), 409);
    assert_eq!(list(&server).await.len(), 1);
}

#[tokio::test]
async fn invalid_directories_do_not_create_rows_or_files() {
    let dir = tempfile::tempdir().unwrap();
    let real = member_dir(dir.path(), "member");
    let root = real.ancestors().last().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, "preserve me").unwrap();
    let missing = dir.path().join("absent");
    let server = server(fresh_in_memory_db().await, dir.path());
    for path in [
        PathBuf::from("relative-member"),
        real.join(".."),
        missing.clone(),
        file.clone(),
        root.to_path_buf(),
        root.join("."),
    ] {
        let response = register(&server, draft(&path)).await;
        assert_eq!(response.status_code(), 400, "{}", response.text());
        assert_eq!(response.json::<Value>()["code"], "invalid_input");
    }
    assert!(list(&server).await.is_empty());
    assert!(!missing.exists());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "preserve me");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_including_parent_segments_need_a_separate_confirmation_flow() {
    let dir = tempfile::tempdir().unwrap();
    let real = member_dir(dir.path(), "member");
    let child = member_dir(&real, "nested");
    let alias = real.parent().unwrap().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    for path in [&alias, &alias.join("nested")] {
        let response = register(&server, draft(path)).await;
        assert_eq!(response.status_code(), 400, "{}", response.text());
    }
    assert!(list(&server).await.is_empty());
    let accepted = register(&server, draft(&child)).await;
    assert_eq!(accepted.status_code(), 200);
}

#[tokio::test]
async fn missing_disabled_deleted_or_other_engine_templates_cannot_be_registered() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let disabled = preset(&db.conn, "codex", false).await;
    // Claude Code is the second engine (10-26); other engines stay out.
    let other = preset(&db.conn, "gemini", true).await;
    let claude = preset(&db.conn, "claude_code", true).await;
    let deleted = preset(&db.conn, "codex", true).await;
    let mut deleted_row: assistant::ActiveModel = deleted.clone().into();
    deleted_row.deleted_at = Set(Some(chrono::Utc::now()));
    deleted_row.update(&db.conn).await.unwrap();
    let server = server(db, dir.path());
    for uid in ["missing", &disabled.uid, &other.uid, &deleted.uid] {
        let mut input = draft(&path);
        input["assistantUid"] = json!(uid);
        let response = register(&server, input).await;
        assert_eq!(response.status_code(), 400, "{}", response.text());
    }
    assert!(list(&server).await.is_empty());
    // A preset decides the engine; asking for another engine is refused.
    let mut mismatch = draft(&path);
    mismatch["assistantUid"] = json!(claude.uid);
    mismatch["engine"] = json!("codex");
    assert_eq!(register(&server, mismatch).await.status_code(), 400);
    let claude_dir = member_dir(dir.path(), "claude");
    let mut with_claude = draft(&claude_dir);
    with_claude["assistantUid"] = json!(claude.uid);
    let bound = register(&server, with_claude).await;
    assert_eq!(bound.status_code(), 200, "{}", bound.text());
    assert_eq!(bound.json::<Value>()["engine"], "claude_code");
    // No template is required; there is no implicit engine probe or fallback.
    let accepted = register(&server, draft(&path)).await;
    assert_eq!(accepted.status_code(), 200);
    let member: Value = accepted.json();
    assert_eq!(member["assistantUid"], Value::Null);
    assert_eq!(member["assistantRevision"], Value::Null);
    assert_eq!(member["engine"], "codex");
}

#[tokio::test]
async fn invalid_text_or_caller_supplied_identity_and_engine_do_not_create_members() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    for (field, value) in [
        ("name", json!("   ")),
        ("role", json!("")),
        ("name", json!("name\nforged")),
        ("role", json!("review\u{1e}frame")),
        ("name", json!("名".repeat(81))),
        ("role", json!("人".repeat(201))),
    ] {
        let mut input = draft(&path);
        input[field] = value;
        let response = register(&server, input).await;
        assert_eq!(response.status_code(), 400, "{}", response.text());
    }
    for engine in ["gemini", "", "Codex"] {
        let mut input = draft(&path);
        input["engine"] = json!(engine);
        let response = register(&server, input).await;
        assert_eq!(response.status_code(), 400, "{}", response.text());
    }
    for (field, value) in [
        ("memberId", json!("forged")),
        ("enabled", json!(true)),
        ("assistantRevision", json!(999)),
    ] {
        let mut input = draft(&path);
        input[field] = value;
        let response = register(&server, input).await;
        assert_eq!(response.status_code(), 422);
    }
    assert!(list(&server).await.is_empty());
}

#[tokio::test]
async fn closing_and_reopening_the_database_preserves_member_identity_and_template_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_disk_db(dir.path()).await;
    let template = preset(&db.conn, "codex", true).await;
    let project = folder_service::add_folder(&db.conn, "/persistent-project")
        .await
        .unwrap();
    // A second handle lets the test close the whole pool after the router is
    // dropped, rather than accidentally proving only an in-memory cache.
    let conn = db.conn.clone();
    let first_server = server(db, dir.path());
    let mut input = draft(&path);
    input["assistantUid"] = json!(template.uid);
    input["projectId"] = json!(project.id);
    let response = register(&first_server, input).await;
    assert_eq!(response.status_code(), 200, "{}", response.text());
    let original: Value = response.json();
    let mut changed: assistant::ActiveModel = template.into();
    changed.revision = Set(8);
    changed.update(&conn).await.unwrap();
    drop(response);
    drop(first_server);
    conn.close().await.unwrap();

    let reopened = server(fresh_disk_db(dir.path()).await, dir.path());
    assert_eq!(list(&reopened).await, vec![original.clone()]);
    assert_eq!(original["assistantRevision"], 7);
    assert_eq!(original["enabled"], false);
    assert_eq!(original["projectId"], project.id);
}

#[tokio::test]
async fn registry_migration_can_upgrade_and_rollback_without_altering_upstream_presets() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_in_memory_db().await;
    let template = preset(&db.conn, "codex", true).await;
    Migrator::down(&db.conn, Some(2)).await.unwrap();
    Migrator::up(&db.conn, None).await.unwrap();
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    assert!(list(&server).await.is_empty());
    let path = member_dir(dir.path(), "member");
    let registered = register(&server, draft(&path)).await;
    assert_eq!(registered.status_code(), 200);
    Migrator::up(&conn, None).await.unwrap();
    assert_eq!(list(&server).await.len(), 1);
    Migrator::down(&conn, Some(1)).await.unwrap();
    assert_eq!(
        assistant::Entity::find_by_id(template.id)
            .one(&conn)
            .await
            .unwrap(),
        Some(template)
    );
}

async fn edit(server: &TestServer, member_id: &str, revision: i64, draft: Value) -> TestResponse {
    server
        .post("/api/hall_update_member")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({
            "memberId": member_id,
            "expectedRevision": revision,
            "draft": draft,
        }))
        .await
}

fn edit_draft(name: &str) -> Value {
    json!({"name": name, "role": "代码审查", "assistantUid": null})
}

#[tokio::test]
async fn editing_metadata_preserves_member_identity_directory_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    std::fs::write(path.join("AGENTS.md"), "private rules").unwrap();
    let db = fresh_in_memory_db().await;
    let template = preset(&db.conn, "codex", true).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let mut input = draft(&path);
    input["assistantUid"] = json!(template.uid);
    let created = register(&server, input).await;
    assert_eq!(created.status_code(), 200);
    let original: Value = created.json();
    let mut changed: assistant::ActiveModel = template.clone().into();
    changed.revision = Set(8);
    changed.update(&conn).await.unwrap();
    let mut input = edit_draft(" 新名字 ");
    input["assistantUid"] = json!(template.uid);
    input["role"] = json!(" 新角色 ");
    let edited = edit(&server, original["memberId"].as_str().unwrap(), 1, input).await;
    assert_eq!(edited.status_code(), 200, "{}", edited.text());
    let member: Value = edited.json();
    for field in ["memberId", "cwd", "engine", "enabled", "createdAt"] {
        assert_eq!(member[field], original[field], "{field} changed");
    }
    assert_eq!(member["name"], "新名字");
    assert_eq!(member["role"], "新角色");
    assert_eq!(member["revision"], 2);
    assert_eq!(member["assistantUid"], template.uid);
    assert_eq!(member["assistantRevision"], 8);
    assert_eq!(
        std::fs::read_to_string(path.join("AGENTS.md")).unwrap(),
        "private rules"
    );
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1);
    assert_eq!(list(&server).await, vec![member]);

    // Removing the template reference is explicit; it does not delete the
    // template, the member directory or the member's stable identity.
    let cleared = edit(
        &server,
        original["memberId"].as_str().unwrap(),
        2,
        edit_draft("新名字"),
    )
    .await;
    assert_eq!(cleared.status_code(), 200);
    let cleared: Value = cleared.json();
    assert_eq!(cleared["assistantUid"], Value::Null);
    assert_eq!(cleared["assistantRevision"], Value::Null);
    assert_eq!(cleared["memberId"], original["memberId"]);
    assert_eq!(cleared["revision"], 3);
    assert!(assistant::Entity::find_by_id(template.id)
        .one(&conn)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn concurrent_edits_cannot_overwrite_the_winner_with_a_stale_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_disk_db(dir.path()).await, dir.path());
    let response = register(&server, draft(&path)).await;
    let original: Value = response.json();
    let id = original["memberId"].as_str().unwrap();
    let (a, b) = tokio::join!(
        edit(&server, id, 1, edit_draft("first")),
        edit(&server, id, 1, edit_draft("second")),
    );
    let mut codes = [a.status_code().as_u16(), b.status_code().as_u16()];
    codes.sort();
    assert_eq!(codes, [200, 409]);
    let winner: Value = if a.status_code() == 200 {
        a.json()
    } else {
        b.json()
    };
    assert_eq!(winner["revision"], 2);
    assert_eq!(list(&server).await, vec![winner.clone()]);
    let stale = edit(&server, id, 1, edit_draft("stale update")).await;
    assert_eq!(stale.status_code(), 409);
    assert_eq!(stale.json::<Value>()["code"], "already_exists");
    assert_eq!(list(&server).await, vec![winner]);
}

#[tokio::test]
async fn rejected_edits_leave_the_original_member_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let disabled = preset(&db.conn, "codex", false).await;
    let server = server(db, dir.path());
    let response = register(&server, draft(&path)).await;
    let original: Value = response.json();
    let id = original["memberId"].as_str().unwrap();
    for (field, value) in [
        ("name", json!(" ")),
        ("role", json!("bad\u{1e}role")),
        ("assistantUid", json!("missing")),
        ("assistantUid", json!(disabled.uid)),
    ] {
        let mut input = edit_draft("changed");
        input[field] = value;
        let response = edit(&server, id, 1, input).await;
        assert_eq!(response.status_code(), 400, "{}", response.text());
        assert_eq!(list(&server).await, vec![original.clone()]);
    }
    let mut unknown_engine = edit_draft("changed");
    unknown_engine["engine"] = json!("gemini");
    assert_eq!(
        edit(&server, id, 1, unknown_engine).await.status_code(),
        400
    );
    assert_eq!(list(&server).await, vec![original.clone()]);
    for (field, value) in [
        ("cwd", json!(dir.path())),
        ("enabled", json!(true)),
        ("assistantRevision", json!(99)),
        ("memberId", json!("forged")),
    ] {
        let mut input = edit_draft("changed");
        input[field] = value;
        let response = edit(&server, id, 1, input).await;
        assert_eq!(response.status_code(), 422);
    }
    let invalid_revision = edit(&server, id, 0, edit_draft("changed")).await;
    assert_eq!(invalid_revision.status_code(), 400);
    let unknown = edit(
        &server,
        &uuid::Uuid::new_v4().to_string(),
        1,
        edit_draft("changed"),
    )
    .await;
    assert_eq!(unknown.status_code(), 404);
    assert_eq!(list(&server).await, vec![original]);
}

async fn projects(server: &TestServer) -> Vec<Value> {
    let response = server
        .post("/api/hall_list_member_projects")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 200, "{}", response.text());
    response.json()
}

#[tokio::test]
async fn projects_offer_live_roots_including_closed_ones_but_not_chat_or_worktrees() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_in_memory_db().await;
    let mut ids = Vec::new();
    for name in ["root", "closed", "chat", "worktree", "deleted"] {
        let entry = folder_service::add_folder(&db.conn, &dir.path().join(name).to_string_lossy())
            .await
            .unwrap();
        ids.push(entry.id);
        let mut row: folder::ActiveModel = folder::Entity::find_by_id(entry.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
            .into();
        match name {
            "root" => row.alias = Set(Some("Alias".to_owned())),
            "closed" => row.is_open = Set(false),
            "chat" => row.kind = Set(folder::FolderKind::Chat),
            "worktree" => row.parent_id = Set(Some(ids[0])),
            "deleted" => row.deleted_at = Set(Some(chrono::Utc::now())),
            _ => unreachable!(),
        }
        row.update(&db.conn).await.unwrap();
    }
    let server = server(db, dir.path());
    let choices = projects(&server).await;
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[0]["id"], ids[0]);
    assert_eq!(choices[0]["alias"], "Alias");
    assert_eq!(choices[1]["id"], ids[1]);
    // Project listing/binding is DB metadata: it never creates project dirs.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn binding_rebinding_and_clearing_keep_identity_home_and_old_client_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    std::fs::write(
        path.join("agent.yaml"),
        "# manual comments\nengine: codex\n",
    )
    .unwrap();
    let db = fresh_in_memory_db().await;
    let a = folder_service::add_folder(&db.conn, "/project-a")
        .await
        .unwrap();
    let b = folder_service::add_folder(&db.conn, "/project-b")
        .await
        .unwrap();
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let mut input = draft(&path);
    input["projectId"] = json!(a.id);
    let created = register(&server, input).await;
    assert_eq!(created.status_code(), 200, "{}", created.text());
    let original: Value = created.json();
    assert_eq!(original["projectId"], a.id);
    let id = original["memberId"].as_str().unwrap();
    let old_edit = edit(&server, id, 1, edit_draft("Rename only")).await;
    assert_eq!(old_edit.status_code(), 200, "{}", old_edit.text());
    assert_eq!(old_edit.json::<Value>()["projectId"], a.id);
    let mut change = edit_draft("Rename only");
    change["projectId"] = json!(b.id);
    let changed = edit(&server, id, 2, change.clone()).await;
    assert_eq!(changed.status_code(), 200, "{}", changed.text());
    let changed: Value = changed.json();
    assert_eq!(changed["projectId"], b.id);
    for field in ["memberId", "cwd", "engine", "enabled", "createdAt"] {
        assert_eq!(changed[field], original[field]);
    }
    // Deleting a project preserves the stable binding; it must not silently
    // turn an unavailable project into the unbound state on read or old edit.
    folder_service::soft_delete_folder(&conn, b.id)
        .await
        .unwrap();
    assert_eq!(list(&server).await[0]["projectId"], b.id);
    assert_eq!(projects(&server).await.len(), 1);
    let preserved = edit(&server, id, 3, edit_draft("Still linked")).await;
    assert_eq!(preserved.status_code(), 200);
    assert_eq!(preserved.json::<Value>()["projectId"], b.id);
    let stale = edit(&server, id, 2, change).await;
    assert_eq!(stale.status_code(), 400); // deleted project is invalid too
    let mut clear = edit_draft("Still linked");
    clear["projectId"] = Value::Null;
    let cleared = edit(&server, id, 4, clear).await;
    assert_eq!(cleared.status_code(), 200, "{}", cleared.text());
    assert_eq!(cleared.json::<Value>()["projectId"], Value::Null);
    assert_eq!(cleared.json::<Value>()["revision"], 5);
    assert_eq!(
        std::fs::read_to_string(path.join("agent.yaml")).unwrap(),
        "# manual comments\nengine: codex\n"
    );
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1);
}

#[tokio::test]
async fn invalid_project_bindings_and_stale_changes_leave_members_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_in_memory_db().await;
    let root = folder_service::add_folder(&db.conn, "/root-project")
        .await
        .unwrap();
    let child = folder_service::add_folder_with_parent(&db.conn, "/worktree", Some(root.id))
        .await
        .unwrap();
    let chat = folder_service::add_folder(&db.conn, "/chat").await.unwrap();
    let mut row: folder::ActiveModel = folder::Entity::find_by_id(chat.id)
        .one(&db.conn)
        .await
        .unwrap()
        .unwrap()
        .into();
    row.kind = Set(folder::FolderKind::Chat);
    row.update(&db.conn).await.unwrap();
    let gone = folder_service::add_folder(&db.conn, "/gone").await.unwrap();
    folder_service::soft_delete_folder(&db.conn, gone.id)
        .await
        .unwrap();
    let server = server(db, dir.path());
    for project_id in [0, -1, 99999, child.id, chat.id, gone.id] {
        let mut input = draft(&path);
        input["projectId"] = json!(project_id);
        let rejected = register(&server, input).await;
        assert_eq!(rejected.status_code(), 400, "{}", rejected.text());
    }
    assert!(list(&server).await.is_empty());
    let response = register(&server, draft(&path)).await;
    let original: Value = response.json();
    assert_eq!(original["projectId"], Value::Null);
    let id = original["memberId"].as_str().unwrap();
    for project_id in [
        json!(child.id),
        json!(chat.id),
        json!(gone.id),
        json!(99999),
    ] {
        let mut input = edit_draft("changed");
        input["projectId"] = project_id;
        assert_eq!(edit(&server, id, 1, input).await.status_code(), 400);
    }
    let mut input = edit_draft("changed");
    input["projectId"] = json!(root.id);
    let accepted = edit(&server, id, 1, input.clone()).await;
    assert_eq!(accepted.status_code(), 200);
    assert_eq!(accepted.json::<Value>()["projectId"], root.id);
    input["projectId"] = Value::Null;
    assert_eq!(edit(&server, id, 1, input).await.status_code(), 409);
    assert_eq!(list(&server).await, vec![accepted.json::<Value>()]);
}

#[tokio::test]
async fn additive_project_migration_preserves_preexisting_members() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let server = server(db, dir.path());
    let created = register(&server, draft(&path)).await;
    assert_eq!(created.status_code(), 200);
    let original: Value = created.json();
    Migrator::down(&conn, Some(1)).await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    assert_eq!(list(&server).await, vec![original]);
}

#[tokio::test]
async fn upstream_039_upgrade_preserves_hall_members_and_project_bindings() {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    // The released OneHall 0.38 schema, including both hall migrations but
    // excluding the upstream migration that was inserted before them in 0.39.
    struct Legacy038;
    #[async_trait::async_trait]
    impl MigratorTrait for Legacy038 {
        fn migrations() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
            Migrator::migrations()
                .into_iter()
                .filter(|m| m.name() != "m20261020_000001_folder_command_run_local")
                .collect()
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Legacy038::up(&conn, None).await.unwrap();
    let project = folder_service::add_folder(&conn, "/existing-project")
        .await
        .unwrap();
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO folder_command (folder_id,name,command,sort_order,\
            created_at,updated_at) VALUES (?, 'dev', 'pnpm dev', 0, \
            '2026-10-21 00:00:00', '2026-10-21 00:00:00')",
        [project.id.into()],
    ))
    .await
    .unwrap();
    let template = preset(&conn, "codex", true).await;
    let server = server(AppDatabase { conn: conn.clone() }, dir.path());
    let path = member_dir(dir.path(), "member");
    let mut input = draft(&path);
    input["projectId"] = json!(project.id);
    input["assistantUid"] = json!(template.uid);
    let response = register(&server, input).await;
    assert_eq!(response.status_code(), 200, "{}", response.text());
    let original: Value = response.json();

    Migrator::up(&conn, None).await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    assert_eq!(list(&server).await, vec![original.clone()]);
    assert_eq!(original["projectId"], project.id);
    assert_eq!(
        assistant::Entity::find_by_id(template.id)
            .one(&conn)
            .await
            .unwrap(),
        Some(template)
    );
    let command = conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT command, run_local FROM folder_command".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        command.try_get::<String>("", "command").unwrap(),
        "pnpm dev"
    );
    assert!(!command.try_get::<bool>("", "run_local").unwrap());
}
