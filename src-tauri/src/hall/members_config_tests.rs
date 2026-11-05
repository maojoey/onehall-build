//! Read-only preview through the authenticated router, using disposable data.
use super::members_tests::{draft, list, member_dir, register, server, TEST_AUTH};
use crate::db::test_helpers::fresh_in_memory_db;
use axum_test::{TestResponse, TestServer};
use serde_json::{json, Value};

async fn preview(server: &TestServer, id: &str, revision: i64) -> TestResponse {
    server
        .post("/api/hall_preview_member_config")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId": id, "expectedRevision": revision}))
        .await
}

fn value(response: TestResponse) -> Value {
    assert_eq!(response.status_code(), 200, "{}", response.text());
    response.json()
}

async fn preview_draft(
    server: &TestServer,
    member: &Value,
    version: &Value,
    draft: Value,
) -> TestResponse {
    server
        .post("/api/hall_preview_member_config_draft")
        .add_header("authorization", TEST_AUTH)
        .json(
            &json!({"memberId":member["memberId"], "expectedRevision":member["revision"],
            "expectedFileVersion":version, "draft":draft}),
        )
        .await
}

#[tokio::test]
async fn draft_inherits_clears_arrays_and_reports_value_and_source_changes_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    std::fs::create_dir(path.join(".onehall")).unwrap();
    let manual = "# handwritten\nschema_version: 1\nmodel: base\nscope:\n  write: ['src/**']\n";
    let overlay = "# keep overlay too\nschema_version: 1\nmodel: ''\nskills:\n  own: [review]\n";
    std::fs::write(path.join("agent.yaml"), manual).unwrap();
    std::fs::write(path.join(".onehall/agent.ui.yaml"), overlay).unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let baseline = value(preview(&server, member["memberId"].as_str().unwrap(), 1).await);
    assert_eq!(
        baseline["overlay"],
        json!({"schema_version":1,"model":"","skills":{"own":["review"]}})
    );
    assert_eq!(baseline["fileVersion"].as_str().unwrap().len(), 64);
    let result = value(
        preview_draft(
            &server,
            &member,
            &baseline["fileVersion"],
            json!({"schema_version":1,"scope":{"write":[],"shell":[]}}),
        )
        .await,
    );
    assert_eq!(result["valid"], true);
    assert_eq!(result["fileVersion"], baseline["fileVersion"]);
    assert_eq!(result["effective"]["model"], "base");
    assert_eq!(result["effective"]["scope"]["write"], json!([]));
    assert_eq!(result["effective"]["skills"]["own"], json!([]));
    let changes = result["changes"].as_array().unwrap();
    let model = changes.iter().find(|c| c["field"] == "model").unwrap();
    assert_eq!(model["before"], Value::Null);
    assert_eq!(model["after"], "base");
    assert_eq!(model["afterSource"], "agent.yaml");
    let shell = changes
        .iter()
        .find(|c| c["field"] == "scope.shell")
        .unwrap();
    assert_eq!(shell["before"], shell["after"]);
    assert_eq!(shell["beforeSource"], "defaults");
    assert_eq!(shell["afterSource"], ".onehall/agent.ui.yaml");
    assert_eq!(
        std::fs::read_to_string(path.join("agent.yaml")).unwrap(),
        manual
    );
    assert_eq!(
        std::fs::read_to_string(path.join(".onehall/agent.ui.yaml")).unwrap(),
        overlay
    );
    assert_eq!(list(&server).await, vec![member.clone()]);
    assert_eq!(
        value(preview(&server, member["memberId"].as_str().unwrap(), 1).await)["fileVersion"],
        baseline["fileVersion"]
    );
    std::fs::write(
        path.join("agent.yaml"),
        format!("{manual}# external edit\n"),
    )
    .unwrap();
    preview_draft(
        &server,
        &member,
        &baseline["fileVersion"],
        json!({"schema_version":1}),
    )
    .await
    .assert_status_conflict();
    let fresh = value(preview(&server, member["memberId"].as_str().unwrap(), 1).await);
    assert_ne!(fresh["fileVersion"], baseline["fileVersion"]);
    let stale = json!({"revision":2,"memberId":member["memberId"]});
    preview_draft(
        &server,
        &stale,
        &fresh["fileVersion"],
        json!({"schema_version":1}),
    )
    .await
    .assert_status_conflict();
}

#[tokio::test]
async fn draft_validation_is_bounded_private_authenticated_and_does_not_create_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let baseline = value(preview(&server, member["memberId"].as_str().unwrap(), 1).await);
    assert_eq!(baseline["overlay"], json!({"schema_version":1}));
    let no_change = value(
        preview_draft(
            &server,
            &member,
            &baseline["fileVersion"],
            json!({"schema_version":1}),
        )
        .await,
    );
    assert_eq!(no_change["valid"], true);
    assert_eq!(no_change["changes"], json!([]));
    for invalid in [
        json!({"schema_version":2}),
        json!({"schema_version":1,"model":null}),
        json!({"schema_version":1,"unknown-private-key":"private-value"}),
        json!({"schema_version":1,"scope":{"read":["../private"]}}),
        json!({"schema_version":1,"model":"x".repeat(65537)}),
    ] {
        let result =
            value(preview_draft(&server, &member, &baseline["fileVersion"], invalid).await);
        assert_eq!(result["valid"], false);
        assert_eq!(result["effective"], Value::Null);
        assert_eq!(result["changes"], json!([]));
        assert_eq!(result["fields"], json!([]));
        assert_eq!(result["diagnostics"][0]["source"], "draft");
        assert!(!result.to_string().contains("private"));
    }
    preview_draft(&server, &member, &json!("bad"), json!({}))
        .await
        .assert_status_bad_request();
    server
        .post("/api/hall_preview_member_config_draft")
        .json(&json!({}))
        .await
        .assert_status_unauthorized();
    let forged = server.post("/api/hall_preview_member_config_draft").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":1,
            "expectedFileVersion":baseline["fileVersion"],"draft":{"schema_version":1},"path":"/tmp/other"})).await;
    assert!(forged.status_code().is_client_error());
    let unknown = json!({"memberId":uuid::Uuid::new_v4().to_string(),"revision":1});
    preview_draft(
        &server,
        &unknown,
        &baseline["fileVersion"],
        json!({"schema_version":1}),
    )
    .await
    .assert_status_not_found();
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
    assert_eq!(list(&server).await, vec![member]);
}

#[tokio::test]
async fn missing_files_preview_defaults_without_creating_anything() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let result = value(preview(&server, member["memberId"].as_str().unwrap(), 1).await);
    assert_eq!(result["valid"], true);
    assert_eq!(result["effective"]["scope"]["read"], json!(["**"]));
    assert_eq!(result["effective"]["scope"]["write"], json!([]));
    assert_eq!(result["effective"]["scope"]["network"], "none");
    assert_eq!(result["files"].as_array().unwrap().len(), 3);
    assert!(result["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["status"] == "missing"));
    assert!(result["fields"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["source"] == "defaults"));
    assert_eq!(list(&server).await, vec![member]);
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
}

#[tokio::test]
async fn overlays_merge_leaves_replace_arrays_and_preserve_files_and_registry() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    std::fs::create_dir(path.join(".onehall")).unwrap();
    let manual = "# keep this comment\nschema_version: 1\nengine: codex\nmodel: \
        base\nscope:\n  write: ['reports/**']\nskills:\n  own: \
        [review]\n";
    let overlay = "schema_version: 1\nmodel: ''\nscope:\n  write: []\n";
    std::fs::write(path.join("agent.yaml"), manual).unwrap();
    std::fs::write(path.join(".onehall/agent.ui.yaml"), overlay).unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let result = value(preview(&server, member["memberId"].as_str().unwrap(), 1).await);
    assert_eq!(result["valid"], true);
    assert_eq!(result["effective"]["model"], Value::Null);
    assert_eq!(result["effective"]["scope"]["read"], json!(["**"]));
    assert_eq!(result["effective"]["scope"]["write"], json!([]));
    assert_eq!(result["effective"]["skills"]["own"], json!(["review"]));
    let fields = result["fields"].as_array().unwrap();
    let write = fields.iter().find(|f| f["field"] == "scope.write").unwrap();
    assert_eq!(write["source"], ".onehall/agent.ui.yaml");
    assert_eq!(write["overridden"], true);
    let skill = fields.iter().find(|f| f["field"] == "skills.own").unwrap();
    assert_eq!(skill["source"], "agent.yaml");
    assert_eq!(
        std::fs::read_to_string(path.join("agent.yaml")).unwrap(),
        manual
    );
    assert_eq!(
        std::fs::read_to_string(path.join(".onehall/agent.ui.yaml")).unwrap(),
        overlay
    );
    assert_eq!(list(&server).await, vec![member]);
}

#[tokio::test]
async fn nested_manual_is_supported_but_two_manuals_are_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    std::fs::create_dir(path.join(".onehall")).unwrap();
    std::fs::write(
        path.join(".onehall/agent.yaml"),
        "schema_version: 1\nmodel: nested\n",
    )
    .unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let id = member["memberId"].as_str().unwrap();
    let result = value(preview(&server, id, 1).await);
    assert_eq!(result["effective"]["model"], "nested");
    std::fs::write(path.join("agent.yaml"), "schema_version: 1\n").unwrap();
    let result = value(preview(&server, id, 1).await);
    assert_eq!(result["valid"], false);
    assert_eq!(result["effective"], Value::Null);
    assert_eq!(result["diagnostics"][0]["code"], "multiple_manuals");
}

#[tokio::test]
async fn bad_declarations_do_not_return_partial_results_or_leak_source_values() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let cases = [
        "schema_version: 1\ntoken: SECRET_VALUE_NEVER_ECHO\n",
        "schema_version: 2\nmodel: SECRET_VALUE_NEVER_ECHO\n",
        "schema_version: 1\nmodel: [SECRET_VALUE_NEVER_ECHO]\n",
        "schema_version: 1\nmodel: null\n",
        "schema_version: 1\nmodel: a\nmodel: b\n",
        "schema_version: 1\n---\nschema_version: 1\n",
        "schema_version: 1\nmodel: !tag SECRET_VALUE_NEVER_ECHO\n",
        "schema_version: 1\nmodel: [SECRET_VALUE_NEVER_ECHO\n",
        "schema_version: 1\ncwd: /SECRET_VALUE_NEVER_ECHO\n",
        "schema_version: 1\nengine: claude\n",
        "schema_version: 1\nscope:\n  read: ['../secret']\n",
        "schema_version: 1\nscope:\n  write: ['/outside']\n",
        "schema_version: 1\nscope:\n  read: ['C:/outside']\n",
        "schema_version: 1\nscope:\n  read: ['a/../../secret']\n",
        "schema_version: 1\nskills:\n  own: [a, a]\n",
        "schema_version: 1\nscope: {read: null}\n",
        "schema_version: 1\nscope: {env: SECRET_VALUE_NEVER_ECHO}\n",
        "",
    ];
    for (n, raw) in cases.iter().enumerate() {
        let path = member_dir(dir.path(), &format!("case-{n}"));
        std::fs::write(path.join("agent.yaml"), raw).unwrap();
        let member = value(register(&server, draft(&path)).await);
        let result = value(preview(&server, member["memberId"].as_str().unwrap(), 1).await);
        assert_eq!(result["valid"], false, "case {n}");
        assert_eq!(result["effective"], Value::Null, "case {n}");
        assert!(!result["diagnostics"].as_array().unwrap().is_empty());
        assert!(!result.to_string().contains("SECRET_VALUE_NEVER_ECHO"));
        assert_eq!(
            std::fs::read_to_string(path.join("agent.yaml")).unwrap(),
            *raw
        );
    }
}

#[tokio::test]
async fn preview_rejects_stale_unknown_or_forged_identity_and_bounds_file_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let id = member["memberId"].as_str().unwrap();
    assert_eq!(preview(&server, id, 2).await.status_code(), 409);
    assert_eq!(preview(&server, "invalid", 1).await.status_code(), 400);
    assert_eq!(
        preview(&server, &uuid::Uuid::new_v4().to_string(), 1)
            .await
            .status_code(),
        404
    );
    let forged = server
        .post("/api/hall_preview_member_config")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":id,"expectedRevision":1,"cwd":"/forged"}))
        .await;
    assert_eq!(forged.status_code(), 422);
    std::fs::write(path.join("agent.yaml"), "x".repeat(65537)).unwrap();
    let result = value(preview(&server, id, 1).await);
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics"][0]["code"], "too_large");
}

#[cfg(unix)]
#[tokio::test]
async fn preview_refuses_links_and_special_files_without_following_or_hanging() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let outside = member_dir(dir.path(), "outside");
    std::fs::write(
        outside.join("agent.yaml"),
        "schema_version: 1\nmodel: SECRET_VALUE_NEVER_ECHO\n",
    )
    .unwrap();
    for kind in ["file-link", "parent-link", "fifo", "directory", "root-link"] {
        let path = member_dir(dir.path(), kind);
        let member = value(register(&server, draft(&path)).await);
        match kind {
            "file-link" => symlink(outside.join("agent.yaml"), path.join("agent.yaml")).unwrap(),
            "parent-link" => symlink(&outside, path.join(".onehall")).unwrap(),
            "fifo" => {
                use std::os::unix::ffi::OsStrExt;
                let fifo =
                    std::ffi::CString::new(path.join("agent.yaml").as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            }
            "directory" => std::fs::create_dir(path.join("agent.yaml")).unwrap(),
            "root-link" => {
                std::fs::rename(&path, path.with_extension("moved")).unwrap();
                symlink(&outside, &path).unwrap();
            }
            _ => unreachable!(),
        }
        let result = value(
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                preview(&server, member["memberId"].as_str().unwrap(), 1),
            )
            .await
            .expect("must not block opening a FIFO"),
        );
        assert_eq!(result["valid"], false, "{kind}");
        assert!(!result.to_string().contains("SECRET_VALUE_NEVER_ECHO"));
    }
}
