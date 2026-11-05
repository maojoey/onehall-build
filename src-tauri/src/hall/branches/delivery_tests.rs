//! Focused real HTTP/SQLite tests; no production runtime or service is started.
use super::{delivery, dispatch, dispatch_wire, local, trust};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::RwLock;
// Test fixtures meet the same private-directory contract as production.
// Generic Windows TEMP inherits broader ACLs and may have an admin owner.
fn private_temp_dir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("Private Unix fixture directory must be protected");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = r#"$ErrorActionPreference='Stop'; $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=New-Object Security.AccessControl.DirectorySecurity; $acl.SetOwner($sid); $acl.SetAccessRuleProtection($true,$false); foreach($identity in @($sid,(New-Object Security.Principal.SecurityIdentifier('S-1-5-18')),(New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))) { $rule=New-Object Security.AccessControl.FileSystemAccessRule($identity,'FullControl','ContainerInherit,ObjectInherit','None','Allow'); $acl.AddAccessRule($rule) }; Set-Acl -LiteralPath $env:ONEHALL_PRIVATE_FIXTURE -AclObject $acl"#;
        let status = std::process::Command::new("powershell.exe")
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW; match process::configure_std_command.
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("ONEHALL_PRIVATE_FIXTURE", directory.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("Private Windows fixture setup must run");
        assert!(
            status.success(),
            "Private Windows fixture ACL initialization failed"
        );
    }
    directory
}
fn sql(q: &str, v: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, q, v)
}
async fn database() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    // Only unrelated task/connection table fixtures; branch schemas are real migrations.
    db.execute_unprepared("CREATE TABLE hall_role_binding(role TEXT PRIMARY KEY,member_id TEXT,revision INTEGER); INSERT INTO hall_role_binding VALUES('pm',NULL,1); CREATE TABLE hall_member(member_id TEXT PRIMARY KEY); CREATE TABLE remote_workspace_connection(id INTEGER PRIMARY KEY,base_url TEXT); CREATE TABLE hall_task(task_id TEXT PRIMARY KEY,revision INTEGER,status TEXT); CREATE TABLE hall_task_attempt(attempt_id TEXT PRIMARY KEY,task_id TEXT,attempt_number INTEGER); CREATE TABLE hall_task_result(attempt_id TEXT,outcome TEXT,body TEXT); CREATE TABLE hall_task_runtime(attempt_id TEXT,run_id TEXT,exit_confirmed_at TEXT)").await.unwrap();
    let manager = SchemaManager::new(&db);
    for migration in [
        Box::new(super::migration::Migration) as Box<dyn MigrationTrait>,
        Box::new(super::dispatch_migration::Migration),
        Box::new(super::inbox_migration::Migration),
        Box::new(super::trust_migration::Migration),
        Box::new(super::binding_migration::Migration),
        Box::new(super::link_migration::Migration),
        Box::new(crate::hall::exchange::migration::Migration),
        Box::new(crate::hall::exchange::protocol_migration::Migration),
    ] {
        migration.up(&manager).await.unwrap();
    }
    db
}
async fn register(db: &DatabaseConnection, endpoint: &str, issuer: &str, secret: &str) -> String {
    let branch = uuid::Uuid::new_v4().to_string();
    db.execute(sql(
        "INSERT INTO remote_workspace_connection(id,base_url) VALUES(1,?)",
        vec![endpoint.into()],
    ))
    .await
    .unwrap();
    db.execute(sql(
        "INSERT INTO hall_branch(id,name,connection_id) VALUES(?,'Fixture',1)",
        vec![branch.clone().into()],
    ))
    .await
    .unwrap();
    db.execute(sql("INSERT INTO hall_branch_dispatch_credential(branch_id,issuer_id,secret,endpoint) VALUES(?,?,?,?)",vec![branch.clone().into(),issuer.into(),secret.into(),endpoint.into()])).await.unwrap();
    branch
}
async fn draft(db: &DatabaseConnection, branch: &str) -> dispatch::Receipt {
    dispatch::create(
        db,
        dispatch::Draft {
            submission_id: uuid::Uuid::new_v4().to_string(),
            branch_id: branch.into(),
            member_ref: "worker".into(),
            title: "Dispatch".into(),
            description: "Body".into(),
            exchange_manifest_id: None,
        },
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn trusted_missing_item_is_404_and_revoked_issuer_cannot_probe() {
    let db = database().await;
    let registration = trust::create_local(&db, "HQ").await.unwrap();
    let token = format!("{}.{}", registration.issuer_id, registration.secret);
    let server = axum_test::TestServer::new(dispatch_wire::routes(db.clone(), None)).unwrap();
    let path = format!("/hall-dispatch/v1/inbox/{}", uuid::Uuid::new_v4());
    server
        .get(&path)
        .authorization_bearer(&token)
        .await
        .assert_status_not_found();
    trust::revoke_local(&db, &registration.issuer_id, 1)
        .await
        .unwrap();
    server
        .get(&path)
        .authorization_bearer(&token)
        .await
        .assert_status_unauthorized();
}
#[tokio::test]
async fn local_export_is_private_once_only_and_never_returns_the_credential() {
    let db = database().await;
    let dir = private_temp_dir();
    let path = dir.path().join("credential.json");
    let input = serde_json::to_vec(&json!({"action":"create_trust","name":"HQ","exportPath":path}))
        .unwrap();
    let public = local::execute_json(&db, &input).await.unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let file: Value = serde_json::from_slice(&bytes).unwrap();
    let secret = file["secret"].as_str().unwrap();
    assert_eq!(secret.len(), 43);
    assert!(!public.to_string().contains(secret));
    assert!(!public.to_string().contains("secret"));
    assert!(
        trust::authenticate(&db, file["issuerId"].as_str().unwrap(), secret)
            .await
            .is_ok()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(local::execute_json(&db, &input).await.is_err());
    assert!(
        bytes == std::fs::read(&path).unwrap(),
        "Export must preserve the original protected file"
    );
    assert_eq!(trust::list_local(&db).await.unwrap().len(), 1);
    for invalid in [
        br#"{"action":"list_trust","secret":"sensitive-marker"}"#.to_vec(),
        vec![b' '; 16385],
    ] {
        let error = local::execute_json(&db, &invalid).await.unwrap_err();
        assert!(!format!("{error:?}").contains("sensitive-marker"));
    }
}
#[tokio::test]
async fn lost_post_response_is_queried_before_retry_and_concurrent_sync_is_idempotent() {
    let db = database().await;
    let registration = trust::create_local(&db, "HQ").await.unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let counter = posts.clone();
    let router = dispatch_wire::routes(db.clone(), None).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let counter = counter.clone();
            async move {
                let post = request.method() == axum::http::Method::POST;
                let response = next.run(request).await;
                if post {
                    counter.fetch_add(1, Ordering::SeqCst);
                    axum::response::IntoResponse::into_response(axum::http::StatusCode::BAD_GATEWAY)
                } else {
                    response
                }
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let branch = register(
        &db,
        &endpoint,
        &registration.issuer_id,
        &registration.secret,
    )
    .await;
    let item = draft(&db, &branch).await;
    let unknown = delivery::sync(&db, &item.dispatch_id).await.unwrap();
    assert_eq!(unknown.status, "pending_delivery");
    assert!(unknown.sync_error.is_some());
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    let (a, b) = tokio::join!(
        delivery::sync(&db, &item.dispatch_id),
        delivery::sync(&db, &item.dispatch_id)
    );
    for result in [a, b] {
        let result = result.unwrap();
        assert_eq!(result.status, "awaiting_acceptance");
        assert!(result.sync_error.is_none());
    }
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    db.execute_unprepared("UPDATE hall_branch SET enabled=0")
        .await
        .unwrap();
    let disabled = delivery::sync(&db, &item.dispatch_id).await.unwrap();
    assert_eq!(disabled.status, "awaiting_acceptance");
    assert!(disabled.sync_error.is_some());
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    let count: i64 = db
        .query_one(sql("SELECT COUNT(*) AS n FROM hall_branch_inbox", vec![]))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap();
    assert_eq!(count, 1);
    server.abort();
}
async fn changing_report() -> (
    DatabaseConnection,
    String,
    String,
    Arc<RwLock<Value>>,
    tokio::task::JoinHandle<()>,
) {
    use axum::{extract::State, routing::get, Json, Router};
    let db = database().await;
    let report = Arc::new(RwLock::new(json!({})));
    let state = report.clone();
    let router = Router::new()
        .route(
            "/hall-dispatch/v1/inbox/{id}",
            get(|State(state): State<Arc<RwLock<Value>>>| async move {
                Json(state.read().await.clone())
            }),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let issuer = uuid::Uuid::new_v4().to_string();
    let branch = register(&db, &endpoint, &issuer, &"A".repeat(43)).await;
    let item = draft(&db, &branch).await;
    *report.write().await = json!({"dispatchId":item.dispatch_id,"issuerId":issuer,"memberRef":"worker","title":"Dispatch","description":"Body","status":"accepted","revision":2,"launchAllowed":false,"task":{"taskId":uuid::Uuid::new_v4().to_string(),"revision":5,"status":"running","resultOutcome":null,"resultText":null}});
    (db, item.dispatch_id, branch, report, server)
}
#[tokio::test]
async fn replies_validate_identity_and_both_revision_cursors_preserve_terminal_evidence() {
    let (db, id, _, report, server) = changing_report().await;
    assert_eq!(delivery::sync(&db, &id).await.unwrap().status, "running");
    {
        let mut r = report.write().await;
        r["task"]["revision"] = 4.into();
        r["task"]["status"] = "draft".into();
    }
    let stale = delivery::sync(&db, &id).await.unwrap();
    assert_eq!(stale.status, "running");
    assert!(stale.sync_error.is_some());
    {
        let mut r = report.write().await;
        r["task"]["revision"] = 6.into();
        r["task"]["status"] = "succeeded".into();
        r["task"]["resultOutcome"] = "succeeded".into();
        r["task"]["resultText"] = "Verified worker output".into();
    }
    let success = delivery::sync(&db, &id).await.unwrap();
    assert_eq!(success.status, "succeeded");
    assert_eq!(
        success.result_text.as_deref(),
        Some("Verified worker output")
    );
    {
        let mut r = report.write().await;
        r["task"]["revision"] = 7.into();
        r["task"]["status"] = "running".into();
        r["task"]["resultText"] = Value::Null;
    }
    let regressed = delivery::sync(&db, &id).await.unwrap();
    assert_eq!(regressed.status, "succeeded");
    assert_eq!(regressed.result_text, success.result_text);
    assert!(regressed.sync_error.is_some());
    {
        let mut r = report.write().await;
        r["issuerId"] = uuid::Uuid::new_v4().to_string().into();
    }
    assert!(delivery::sync(&db, &id).await.unwrap().sync_error.is_some());
    server.abort();
}
#[tokio::test]
async fn mere_success_status_without_result_evidence_is_outcome_unknown() {
    let (db, id, _, report, server) = changing_report().await;
    report.write().await["task"]["status"] = "succeeded".into();
    assert_eq!(
        delivery::sync(&db, &id).await.unwrap().status,
        "outcome_unknown"
    );
    server.abort();
}
#[tokio::test]
async fn local_import_binds_registered_endpoint_and_requires_protected_file_and_transport_acknowledgement(
) {
    let db = database().await;
    let dir = private_temp_dir();
    let path = dir.path().join("credential.json");
    local::execute_json(
        &db,
        &serde_json::to_vec(&json!({"action":"create_trust","name":"HQ","exportPath":path}))
            .unwrap(),
    )
    .await
    .unwrap();
    let branch = uuid::Uuid::new_v4().to_string();
    db.execute_unprepared("INSERT INTO remote_workspace_connection VALUES(1,'http://127.0.0.1:1')")
        .await
        .unwrap();
    db.execute(sql(
        "INSERT INTO hall_branch(id,name,connection_id) VALUES(?,'Fixture',1)",
        vec![branch.clone().into()],
    ))
    .await
    .unwrap();
    let mut input = json!({"action":"import_credential","branchId":branch,"path":path});
    assert!(
        local::execute_json(&db, &serde_json::to_vec(&input).unwrap())
            .await
            .is_err()
    );
    input["acknowledgeTrustedTransport"] = true.into();
    let public = local::execute_json(&db, &serde_json::to_vec(&input).unwrap())
        .await
        .unwrap();
    assert_eq!(public["imported"], true);
    assert!(!public.to_string().contains("secret"));
    assert!(super::link_migration::Migration
        .down(&SchemaManager::new(&db))
        .await
        .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            local::execute_json(&db, &serde_json::to_vec(&input).unwrap())
                .await
                .is_err()
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = dir.path().join("alias");
        symlink(&path, &alias).unwrap();
        input["path"] = json!(alias);
        assert!(
            local::execute_json(&db, &serde_json::to_vec(&input).unwrap())
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn a_different_issuer_or_endpoint_cannot_redeliver_an_unknown_attempt() {
    let (db, id, branch, report, server) = changing_report().await;
    report.write().await["dispatchId"] = uuid::Uuid::new_v4().to_string().into();
    assert!(delivery::sync(&db, &id).await.unwrap().sync_error.is_some());
    let replacement = uuid::Uuid::new_v4().to_string();
    db.execute(sql(
        "UPDATE hall_branch_dispatch_credential SET issuer_id=? WHERE branch_id=?",
        vec![replacement.into(), branch.into()],
    ))
    .await
    .unwrap();
    report.write().await["dispatchId"] = id.clone().into();
    let result = delivery::sync(&db, &id).await.unwrap();
    assert_eq!(result.status, "pending_delivery");
    assert!(result.sync_error.is_some());
    server.abort();
}
#[cfg(unix)]
#[test]
fn cli_database_preflight_refuses_symlinks_and_public_storage() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap().join("private");
    local::prepare_database(&directory, "codeg.db").unwrap();
    assert_eq!(
        std::fs::metadata(directory.join("codeg.db"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(local::prepare_database(&directory, "codeg.db").is_err());
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let alias = root.path().join("alias");
    symlink(&directory, &alias).unwrap();
    assert!(local::prepare_database(&alias, "codeg.db").is_err());
}

#[tokio::test]
async fn creation_identity_is_atomic_and_the_same_submission_cannot_change_owner() {
    let db = database().await;
    let issuer = uuid::Uuid::new_v4().to_string();
    let branch = register(&db, "http://127.0.0.1:1", &issuer, &"A".repeat(43)).await;
    let submission = uuid::Uuid::new_v4().to_string();
    let member = uuid::Uuid::new_v4().to_string();
    db.execute(sql(
        "INSERT INTO hall_member(member_id) VALUES(?)",
        vec![member.clone().into()],
    ))
    .await
    .unwrap();
    let input = || dispatch::Draft {
        submission_id: submission.clone(),
        branch_id: branch.clone(),
        member_ref: "worker".into(),
        title: "Dispatch".into(),
        description: "Body".into(),
        exchange_manifest_id: None,
    };
    let host = dispatch::create(&db, input()).await.unwrap();
    assert_eq!(
        host.dispatch_id,
        dispatch::create(&db, input()).await.unwrap().dispatch_id
    );
    assert!(
        dispatch::create_for_pm(&db, input(), &member)
            .await
            .is_err(),
        "host submission cannot be adopted by a PM"
    );
    let second = uuid::Uuid::new_v4().to_string();
    let pm_input = || dispatch::Draft {
        submission_id: second.clone(),
        branch_id: branch.clone(),
        member_ref: "worker".into(),
        title: "Dispatch".into(),
        description: "Body".into(),
        exchange_manifest_id: None,
    };
    let pm = dispatch::create_for_pm(&db, pm_input(), &member)
        .await
        .unwrap();
    assert_eq!(
        pm.dispatch_id,
        dispatch::create_for_pm(&db, pm_input(), &member)
            .await
            .unwrap()
            .dispatch_id
    );
    assert!(dispatch::create(&db, pm_input()).await.is_err());
    let other = uuid::Uuid::new_v4().to_string();
    assert!(dispatch::create_for_pm(&db, pm_input(), &other)
        .await
        .is_err());
}

#[tokio::test]
async fn equal_revision_cannot_acquire_new_result_outcome_evidence() {
    let (db, id, _, report, server) = changing_report().await;
    report.write().await["task"]["status"] = "succeeded".into();
    let first = delivery::sync(&db, &id).await.unwrap();
    assert_eq!(first.status, "outcome_unknown");
    report.write().await["task"]["resultOutcome"] = "succeeded".into();
    let inconsistent = delivery::sync(&db, &id).await.unwrap();
    assert_eq!(inconsistent.status, "outcome_unknown");
    assert!(inconsistent.sync_error.is_some());
    // Even outcomes that currently map to the same public status are immutable.
    report.write().await["task"]["resultOutcome"] = "failed".into();
    assert!(delivery::sync(&db, &id).await.unwrap().sync_error.is_some());
    server.abort();
}
#[tokio::test]
async fn result_view_never_substitutes_an_older_attempt_result() {
    let db = database().await;
    let registration = trust::create_local(&db, "HQ").await.unwrap();
    let issuer = trust::authenticate(&db, &registration.issuer_id, &registration.secret)
        .await
        .unwrap();
    let dispatch = uuid::Uuid::new_v4().to_string();
    super::inbox::receive_verified(
        &db,
        &issuer,
        super::inbox::Envelope {
            dispatch_id: dispatch.clone(),
            member_ref: "worker".into(),
            title: "Bound task".into(),
            description: "Body".into(),
        },
    )
    .await
    .unwrap();
    super::inbox::decide(
        &db,
        &registration.issuer_id,
        &dispatch,
        1,
        super::inbox::Decision::Accept,
    )
    .await
    .unwrap();
    let task = uuid::Uuid::new_v4().to_string();
    db.execute(sql(
        "INSERT INTO hall_task(task_id,revision,status) VALUES(?,8,'running')",
        vec![task.clone().into()],
    ))
    .await
    .unwrap();
    db.execute(sql("INSERT INTO hall_branch_task_binding(issuer_id,dispatch_id,task_id,request_hash) VALUES(?,?,?,'fixture')",vec![registration.issuer_id.clone().into(),dispatch.clone().into(),task.clone().into()])).await.unwrap();
    db.execute(sql("INSERT INTO hall_task_attempt(attempt_id,task_id,attempt_number) VALUES('old',?,1),('current',?,2)",vec![task.clone().into(),task.into()])).await.unwrap();
    db.execute_unprepared("INSERT INTO hall_task_result(attempt_id,outcome,body) VALUES('old','failed','Previous attempt output')").await.unwrap();
    let current = super::dispatch_result::detail(&db, &issuer, &dispatch)
        .await
        .unwrap()
        .task
        .unwrap();
    assert_eq!(current.status, "running");
    assert!(current.result_outcome.is_none());
    assert!(current.result_text.is_none());
    db.execute_unprepared("UPDATE hall_task SET status='interrupted',revision=9")
        .await
        .unwrap();
    let terminal = super::dispatch_result::detail(&db, &issuer, &dispatch)
        .await
        .unwrap()
        .task
        .unwrap();
    assert_eq!(terminal.status, "interrupted");
    assert!(terminal.result_outcome.is_none());
    assert!(terminal.result_text.is_none());
    db.execute_unprepared("INSERT INTO hall_task_result(attempt_id,outcome,body) VALUES('current','succeeded','Current attempt output')").await.unwrap();
    let latest = super::dispatch_result::detail(&db, &issuer, &dispatch)
        .await
        .unwrap()
        .task
        .unwrap();
    assert_eq!(
        latest.result_text.as_deref(),
        Some("Current attempt output")
    );
}

#[tokio::test]
async fn explicit_interrupted_recovery_advances_only_with_a_newer_terminal_task_revision() {
    for outcome in ["succeeded", "failed"] {
        let (db, id, _, report, server) = changing_report().await;
        report.write().await["task"]["status"] = "interrupted".into();
        assert_eq!(
            delivery::sync(&db, &id).await.unwrap().status,
            "interrupted"
        );
        {
            let mut r = report.write().await;
            r["task"]["status"] = outcome.into();
            r["task"]["resultOutcome"] = outcome.into();
            r["task"]["resultText"] = "Recovered result".into();
        }
        let equal = delivery::sync(&db, &id).await.unwrap();
        assert_eq!(equal.status, "interrupted");
        assert!(equal.sync_error.is_some());
        report.write().await["task"]["revision"] = 6.into();
        let recovered = delivery::sync(&db, &id).await.unwrap();
        assert_eq!(recovered.status, outcome);
        assert_eq!(recovered.result_text.as_deref(), Some("Recovered result"));
        assert!(recovered.sync_error.is_none());
        server.abort();
    }
    for previous in ["interrupted", "cancelled"] {
        let (db, id, _, report, server) = changing_report().await;
        report.write().await["task"]["status"] = previous.into();
        assert_eq!(
            delivery::sync(&db, &id).await.unwrap().status,
            "interrupted"
        );
        {
            let mut r = report.write().await;
            r["task"]["revision"] = 6.into();
            r["task"]["status"] = "running".into();
        }
        let regressed = delivery::sync(&db, &id).await.unwrap();
        assert_eq!(regressed.status, "interrupted");
        assert!(regressed.sync_error.is_some());
        if previous == "cancelled" {
            let mut r = report.write().await;
            r["task"]["revision"] = 7.into();
            r["task"]["status"] = "succeeded".into();
            r["task"]["resultOutcome"] = "succeeded".into();
            drop(r);
            let cancelled = delivery::sync(&db, &id).await.unwrap();
            assert_eq!(cancelled.status, "interrupted");
            assert!(cancelled.sync_error.is_some());
        }
        server.abort();
    }
}

// A local CLI must remain usable while the normal server owns a SQLite handle.
// An exclusive credential-style database preflight fails here on Windows.
#[tokio::test]
async fn local_cli_can_create_and_list_trust_while_server_database_is_open() {
    let directory = private_temp_dir();
    #[cfg(unix)]
    let data_dir = directory.path().canonicalize().unwrap();
    #[cfg(not(unix))]
    let data_dir = directory.path().to_path_buf();
    local::prepare_database(&data_dir, "codeg.db").unwrap();
    let database_url = format!("sqlite://{}?mode=rw", data_dir.join("codeg.db").display());
    let server = Database::connect(&database_url).await.unwrap();
    super::trust_migration::Migration
        .up(&SchemaManager::new(&server))
        .await
        .unwrap();

    // Keep the server connection alive throughout the CLI's preflight and write.
    local::prepare_database(&data_dir, "codeg.db")
        .expect("Local CLI preflight must permit an already-open SQLite database");
    let cli = Database::connect(&database_url).await.unwrap();
    let export_path = data_dir.join("credential.json");
    let created = local::execute_json(
        &cli,
        &serde_json::to_vec(&json!({
            "action": "create_trust", "name": "Live server", "exportPath": export_path
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let listed = local::execute_json(&server, br#"{"action":"list_trust"}"#)
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["issuerId"], created["issuerId"]);
    let credential: Value = serde_json::from_slice(&std::fs::read(export_path).unwrap()).unwrap();
    assert_eq!(credential["issuerId"], created["issuerId"]);
    assert!(trust::authenticate(
        &server,
        credential["issuerId"].as_str().unwrap(),
        credential["secret"].as_str().unwrap(),
    )
    .await
    .is_ok());
    assert!(!listed.to_string().contains("secret"));
    cli.close().await.unwrap();
    server.close().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn credential_import_refuses_a_file_open_in_another_handle() {
    let db = database().await;
    let directory = private_temp_dir();
    let path = directory.path().join("credential.json");
    local::execute_json(
        &db,
        &serde_json::to_vec(&json!({
            "action": "create_trust", "name": "HQ", "exportPath": path
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let branch = uuid::Uuid::new_v4().to_string();
    db.execute_unprepared(
        "INSERT INTO remote_workspace_connection VALUES(1,'https://example.test')",
    )
    .await
    .unwrap();
    db.execute(sql(
        "INSERT INTO hall_branch(id,name,connection_id) VALUES(?,'Fixture',1)",
        vec![branch.clone().into()],
    ))
    .await
    .unwrap();
    let request = serde_json::to_vec(&json!({
        "action": "import_credential", "branchId": branch, "path": path
    }))
    .unwrap();
    let held = std::fs::File::open(&path).unwrap();
    assert!(
        local::execute_json(&db, &request).await.is_err(),
        "Credential reads must retain exclusive access despite database sharing"
    );
    drop(held);
    assert_eq!(
        local::execute_json(&db, &request).await.unwrap()["imported"],
        true
    );
}
