//! Desktop transfer adapters against a real loopback HQ: the real
//! `/api/hall-exchange` routes behind the real token middleware, on a
//! private SQLite database and exchange store. No remote host is contacted.
use super::*;
use crate::{
    app_error::AppErrorCode,
    db::test_helpers::fresh_disk_db,
    hall::exchange::{api, Exchange},
    models::RemoteWorkspaceHeader,
};
use axum::{
    http::{HeaderMap, StatusCode},
    middleware,
    routing::get,
    Router,
};
use serde_json::{json, Value};

const TOKEN: &str = "loopback-test-token";
const BYTES: &[u8] = "{\"rows\":[1,2,3],\"note\":\"桌面上传\"}".as_bytes();

fn digest(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
/// Real path of a fresh temp dir: macOS `/var` is itself a symlink, which the
/// anchored walk rightly refuses.
fn real_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}
fn connection(base_url: &str, token: &str) -> RemoteWorkspaceConnectionInfo {
    RemoteWorkspaceConnectionInfo {
        id: 1,
        name: "loopback".into(),
        base_url: base_url.into(),
        token: token.into(),
        headers: vec![RemoteWorkspaceHeader {
            name: "x-proxy-secret".into(),
            value: "proxy".into(),
        }],
        sort_order: 0,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}
async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

struct Hq {
    exchange: Exchange,
    url: String,
    _dir: tempfile::TempDir,
}
impl Hq {
    async fn start() -> Self {
        let (dir, root) = real_dir();
        let db = fresh_disk_db(&root).await.conn;
        let exchange = Exchange::open(db.clone(), &root.join("exchange")).unwrap();
        let api =
            api::routes(db, Some(exchange.clone())).layer(middleware::from_fn(|req, next| {
                crate::web::auth::require_token(req, next, TOKEN.into())
            }));
        let url = serve(Router::new().nest("/api", api)).await;
        Self {
            exchange,
            url,
            _dir: dir,
        }
    }
    async fn prepare(&self, submission: &str, name: &str, bytes: &[u8]) -> (String, String) {
        let ticket = api::execute(
            Some(&self.exchange),
            serde_json::from_value(json!({
                "action": "prepare_input",
                "submissionId": submission,
                "displayName": name,
                "size": bytes.len(),
                "sha256": digest(bytes),
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        (
            ticket["objectId"].as_str().unwrap().into(),
            ticket["transferId"].as_str().unwrap().into(),
        )
    }
    async fn confirm(&self, submission: &str, object: &str) -> Result<Value, AppCommandError> {
        api::execute(
            Some(&self.exchange),
            serde_json::from_value(json!({
                "action": "confirm_input",
                "submissionId": submission,
                "objectId": object,
            }))
            .unwrap(),
        )
        .await
    }
}
fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tokio::test]
async fn inspect_hashes_on_the_host_and_refuses_links_and_relative_paths() {
    let (_dir, root) = real_dir();
    let file = root.join("輸入 data.json");
    std::fs::write(&file, BYTES).unwrap();
    let seen = inspect(file.clone()).await.unwrap();
    assert_eq!(
        seen,
        InspectedFile {
            display_name: "輸入 data.json".into(),
            size: BYTES.len() as u64,
            sha256: digest(BYTES),
        }
    );
    assert!(inspect(PathBuf::from("relative.json")).await.is_err());
    assert!(inspect(root.clone()).await.is_err(), "directory");
    assert!(inspect(root.join("missing.json")).await.is_err());
    #[cfg(unix)]
    {
        let link = root.join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(inspect(link).await.is_err(), "symlink");
        let linked_dir = root.join("linked-dir");
        std::os::unix::fs::symlink(&root, &linked_dir).unwrap();
        assert!(
            inspect(linked_dir.join("輸入 data.json")).await.is_err(),
            "symlinked ancestor"
        );
        let hard = root.join("hard.json");
        std::fs::hard_link(&file, &hard).unwrap();
        assert!(inspect(hard).await.is_err(), "hard link");
    }
}

#[tokio::test]
async fn remote_upload_streams_authenticated_bytes_the_hq_verifies() {
    let hq = Hq::start().await;
    let (_dir, root) = real_dir();
    let file = root.join("in.json");
    std::fs::write(&file, BYTES).unwrap();
    let submission = uid();
    let (object, transfer) = hq.prepare(&submission, "in.json", BYTES).await;

    let wrong = remote_upload(&connection(&hq.url, "wrong"), &transfer, file.clone())
        .await
        .unwrap_err();
    assert!(matches!(wrong.code, AppErrorCode::AuthenticationFailed));
    assert!(
        hq.confirm(&submission, &object).await.is_err(),
        "nothing stored"
    );

    let stored = remote_upload(&connection(&hq.url, TOKEN), &transfer, file.clone())
        .await
        .unwrap();
    assert_eq!(stored.object_id, object);
    assert_eq!(stored.sha256, digest(BYTES));
    let confirmed = hq.confirm(&submission, &object).await.unwrap();
    assert_eq!(confirmed["size"], json!(BYTES.len()));
    // Same bytes again are an idempotent replay, not a second object.
    let replay = remote_upload(&connection(&hq.url, TOKEN), &transfer, file)
        .await
        .unwrap();
    assert_eq!(replay, stored);
}

#[tokio::test]
async fn remote_upload_of_different_bytes_is_refused_by_the_hq() {
    let hq = Hq::start().await;
    let (_dir, root) = real_dir();
    let file = root.join("in.json");
    std::fs::write(&file, b"not what was announced").unwrap();
    let submission = uid();
    let (object, transfer) = hq.prepare(&submission, "in.json", BYTES).await;
    let refused = remote_upload(&connection(&hq.url, TOKEN), &transfer, file)
        .await
        .unwrap_err();
    assert!(!matches!(refused.code, AppErrorCode::AuthenticationFailed));
    assert!(hq.confirm(&submission, &object).await.is_err());
    // An id that is not a UUID never reaches the network.
    let bad = remote_upload(
        &connection(&hq.url, TOKEN),
        "../admin",
        root.join("in.json"),
    )
    .await
    .unwrap_err();
    assert!(matches!(bad.code, AppErrorCode::InvalidInput));
}

#[tokio::test]
async fn remote_save_of_an_object_the_hq_does_not_own_creates_no_file() {
    let hq = Hq::start().await;
    let (_dir, root) = real_dir();
    let target = root.join("result.json");
    let denied = remote_save(
        &connection(&hq.url, TOKEN),
        &uid(),
        BYTES.len() as u64,
        &digest(BYTES),
        target.clone(),
    )
    .await
    .unwrap_err();
    assert!(matches!(denied.code, AppErrorCode::PermissionDenied));
    assert!(!target.exists());
    let expired = remote_save(
        &connection(&hq.url, "wrong"),
        &uid(),
        1,
        &digest(b"x"),
        target.clone(),
    )
    .await
    .unwrap_err();
    assert!(matches!(expired.code, AppErrorCode::AuthenticationFailed));
    assert!(!target.exists());
}

/// Byte-level stub for the download side, which needs a collected artifact
/// from a full branch run (covered end to end by 05's `exchange_wire`).
async fn artifact_stub(served: &'static [u8]) -> String {
    serve(Router::new().route(
        "/api/hall-exchange/artifacts/{object}",
        get(move |headers: HeaderMap| async move {
            let authorized = headers.get("authorization").and_then(|v| v.to_str().ok())
                == Some(&format!("Bearer {TOKEN}"))
                && headers.get("x-proxy-secret").is_some();
            if authorized {
                (StatusCode::OK, served.to_vec())
            } else {
                (StatusCode::UNAUTHORIZED, Vec::new())
            }
        }),
    ))
    .await
}

#[tokio::test]
async fn remote_save_writes_a_new_file_only_when_size_and_hash_match() {
    let url = artifact_stub(BYTES).await;
    let (_dir, root) = real_dir();
    let conn = connection(&url, TOKEN);
    let target = root.join("result.json");
    remote_save(
        &conn,
        &uid(),
        BYTES.len() as u64,
        &digest(BYTES),
        target.clone(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), BYTES);

    // Never overwrite: the operator's existing file stays as it was.
    let again = remote_save(
        &conn,
        &uid(),
        BYTES.len() as u64,
        &digest(BYTES),
        target.clone(),
    )
    .await
    .unwrap_err();
    assert!(matches!(again.code, AppErrorCode::AlreadyExists));
    assert_eq!(std::fs::read(&target).unwrap(), BYTES);

    let tampered = root.join("tampered.json");
    let wrong = remote_save(
        &conn,
        &uid(),
        BYTES.len() as u64,
        &digest(b"other"),
        tampered.clone(),
    )
    .await
    .unwrap_err();
    assert!(matches!(wrong.code, AppErrorCode::IoError));
    assert!(!tampered.exists(), "partial file removed");

    let short = remote_save(&conn, &uid(), 3, &digest(BYTES), tampered.clone())
        .await
        .unwrap_err();
    assert!(matches!(short.code, AppErrorCode::IoError));
    assert!(!tampered.exists());

    let unauthorized = remote_save(
        &connection(&url, "wrong"),
        &uid(),
        BYTES.len() as u64,
        &digest(BYTES),
        tampered.clone(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        unauthorized.code,
        AppErrorCode::AuthenticationFailed
    ));
    assert!(!tampered.exists());
    let relative = remote_save(&conn, &uid(), 1, &digest(b"x"), PathBuf::from("out.json"))
        .await
        .unwrap_err();
    assert!(matches!(relative.code, AppErrorCode::InvalidInput));
}

#[tokio::test]
async fn a_branch_linked_connection_cannot_carry_exchange_bytes() {
    let (_dir, root) = real_dir();
    let db = fresh_disk_db(&root).await.conn;
    let saved =
        remote_workspace_connection_service::create(&db, "hq", "http://127.0.0.1:9", "t", &[])
            .await
            .unwrap();
    let branch =
        remote_workspace_connection_service::create(&db, "branch", "http://127.0.0.1:10", "b", &[])
            .await
            .unwrap();
    db.execute(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Sqlite,
        "INSERT INTO hall_branch(id,name,connection_id) VALUES(?,'Fixture',?)",
        vec![uid().into(), branch.id.into()],
    ))
    .await
    .unwrap();
    assert_eq!(headquarters(&db, saved.id).await.unwrap().id, saved.id);
    assert!(matches!(
        headquarters(&db, branch.id).await.unwrap_err().code,
        AppErrorCode::NotFound
    ));
    assert!(matches!(
        headquarters(&db, saved.id + 100).await.unwrap_err().code,
        AppErrorCode::NotFound
    ));
}

#[test]
fn inspect_reply_matches_the_published_desktop_sample() {
    let sample: Value =
        serde_json::from_str(include_str!("contract_samples/desktop_transfer.json")).unwrap();
    let reply = &sample["inspectReply"];
    let built = InspectedFile {
        display_name: reply["displayName"].as_str().unwrap().into(),
        size: reply["size"].as_u64().unwrap(),
        sha256: reply["sha256"].as_str().unwrap().into(),
    };
    assert_eq!(serde_json::to_value(built).unwrap(), *reply);
}

#[tokio::test]
async fn cancelling_a_remote_download_removes_its_partial_file() {
    use axum::body::{Body, Bytes};
    use futures_util::{stream, StreamExt};
    let url = serve(Router::new().route(
        "/api/hall-exchange/artifacts/{object}",
        get(|| async {
            let body = stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"part")) })
                .chain(stream::pending::<Result<Bytes, std::io::Error>>());
            Body::from_stream(body)
        }),
    ))
    .await;
    let (_dir, root) = real_dir();
    let target = root.join("result.json");
    let destination = target.clone();
    let download = tokio::spawn(async move {
        remote_save(
            &connection(&url, TOKEN),
            &uid(),
            BYTES.len() as u64,
            &digest(BYTES),
            destination,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                !download.is_finished(),
                "download ended before the controlled pause"
            );
            // Windows directory entries can cache size=0 while the writer is
            // open; query the real path without changing the pause threshold.
            let written = std::fs::read_dir(&root)
                .unwrap()
                .any(|entry| std::fs::metadata(entry.unwrap().path()).unwrap().len() >= 4);
            if written {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first body chunk should reach the destination directory");
    download.abort();
    assert!(download.await.unwrap_err().is_cancelled());
    assert!(
        !target.exists(),
        "aborted download exposed a partial result"
    );
    assert_eq!(
        std::fs::read_dir(root).unwrap().count(),
        0,
        "partial file survived cancellation"
    );
}

#[tokio::test]
async fn remote_upload_bounds_success_and_error_receipts_before_parsing() {
    use axum::{body::Bytes, routing::put};
    let (_dir, root) = real_dir();
    let file = root.join("input.json");
    std::fs::write(&file, BYTES).unwrap();
    for status in [StatusCode::OK, StatusCode::FORBIDDEN] {
        let url = serve(Router::new().route(
            "/api/hall-exchange/transfers/{transfer}",
            put(move |_body: Bytes| async move { (status, vec![b'x'; 65537]) }),
        ))
        .await;
        let error = remote_upload(&connection(&url, TOKEN), &uid(), file.clone())
            .await
            .unwrap_err();
        assert!(matches!(error.code, AppErrorCode::NetworkError));
        assert!(
            format!("{error:?}").contains("Remote reply exceeds 64 KiB"),
            "{error:?}"
        );
    }
}

#[test]
fn file_dialog_authority_accepts_the_matching_remote_workspace_only() {
    let local = url::Url::parse("tauri://localhost/workspace").unwrap();
    let remote = url::Url::parse("tauri://localhost/workspace?remoteConnectionId=7&remoteWindowId=11111111-1111-4111-8111-111111111111").unwrap();
    assert!(validate_file_window("main", &local, FileScope::Inspect).is_ok());
    assert!(validate_file_window("main", &local, FileScope::Local).is_ok());
    assert!(validate_file_window("main", &local, FileScope::Remote(7)).is_ok());
    assert!(validate_file_window("settings", &local, FileScope::Inspect).is_ok());
    for label in ["main", "remote-workspace-7"] {
        assert!(validate_file_window(label, &remote, FileScope::Inspect).is_ok());
        assert!(validate_file_window(label, &remote, FileScope::Remote(7)).is_ok());
        assert!(validate_file_window(label, &remote, FileScope::Remote(8)).is_err());
        assert!(validate_file_window(label, &remote, FileScope::Local).is_err());
    }
}

#[test]
fn file_dialog_authority_refuses_foreign_origins_and_forged_window_identity() {
    for (label, raw) in [
        ("main", "https://example.invalid/workspace"),
        ("main", "https://tauri.localhost.example.invalid/workspace"),
        ("embedded-browser", "tauri://localhost/workspace"),
        ("remote-workspace-7", "tauri://localhost/workspace"),
        (
            "remote-workspace-7",
            "tauri://localhost/workspace?remoteConnectionId=8",
        ),
        (
            "remote-workspace-7-extra",
            "tauri://localhost/workspace?remoteConnectionId=7",
        ),
        (
            "main",
            "tauri://localhost/workspace?remoteConnectionId=7&remoteConnectionId=8",
        ),
        ("main", "tauri://localhost/workspace?remoteConnectionId=-1"),
        (
            "main",
            "tauri://localhost/workspace?remoteWindowId=11111111-1111-4111-8111-111111111111",
        ),
        (
            "main",
            "tauri://localhost/workspace?remoteConnectionId=7&remoteWindowId=forged",
        ),
    ] {
        let url = url::Url::parse(raw).unwrap();
        for scope in [FileScope::Inspect, FileScope::Local, FileScope::Remote(7)] {
            assert!(
                validate_file_window(label, &url, scope).is_err(),
                "{label}: {raw} / {scope:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_destination_created_during_download_is_preserved_and_reported_as_a_conflict() {
    use axum::body::{Body, Bytes};
    use futures_util::{stream, StreamExt};
    use std::sync::Arc;
    let gate = Arc::new(tokio::sync::Notify::new());
    let server_gate = gate.clone();
    let url = serve(Router::new().route(
        "/api/hall-exchange/artifacts/{object}",
        get(move || {
            let gate = server_gate.clone();
            async move {
                let first = stream::once(async {
                    Ok::<_, std::io::Error>(Bytes::from_static(&BYTES[..4]))
                });
                let rest = stream::once(async move {
                    gate.notified().await;
                    Ok::<_, std::io::Error>(Bytes::from_static(&BYTES[4..]))
                });
                Body::from_stream(first.chain(rest))
            }
        }),
    ))
    .await;
    let (_dir, root) = real_dir();
    let target = root.join("result.json");
    let destination = target.clone();
    let download = tokio::spawn(async move {
        remote_save(
            &connection(&url, TOKEN),
            &uid(),
            BYTES.len() as u64,
            &digest(BYTES),
            destination,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                !download.is_finished(),
                "download ended before the controlled pause"
            );
            if std::fs::read_dir(&root)
                .unwrap()
                .any(|entry| std::fs::metadata(entry.unwrap().path()).unwrap().len() >= 4)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Also catches the former direct-write design: an operator can replace
    // its visible partial result while the request still has that file open.
    if target.exists() {
        std::fs::remove_file(&target).unwrap();
    }
    std::fs::write(&target, b"operator-owned replacement").unwrap();
    gate.notify_one();
    let error = download.await.unwrap().unwrap_err();
    assert!(
        matches!(error.code, AppErrorCode::AlreadyExists),
        "{error:?}"
    );
    assert_eq!(
        std::fs::read(target).unwrap(),
        b"operator-owned replacement"
    );
    assert_eq!(
        std::fs::read_dir(root).unwrap().count(),
        1,
        "private partial file survived conflict"
    );
}
