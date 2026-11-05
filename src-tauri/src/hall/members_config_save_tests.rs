//! All writes are restricted to these disposable member directories.
use super::members_tests::{draft, list, member_dir, register, server, TEST_AUTH};
use crate::db::test_helpers::fresh_in_memory_db;
use axum_test::{TestResponse, TestServer};
use serde_json::{json, Value};

fn value(response: TestResponse) -> Value {
    assert_eq!(response.status_code(), 200, "{}", response.text());
    response.json()
}
async fn read(server: &TestServer, member: &Value) -> Value {
    value(
        server
            .post("/api/hall_preview_member_config")
            .add_header("authorization", TEST_AUTH)
            .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"]}))
            .await,
    )
}
async fn save(server: &TestServer, member: &Value, version: &Value, draft: Value) -> TestResponse {
    server
        .post("/api/hall_save_member_config")
        .add_header("authorization", TEST_AUTH)
        .json(
            &json!({"memberId":member["memberId"],"expectedRevision":member["revision"],
            "expectedFileVersion":version,"draft":draft}),
        )
        .await
}

#[tokio::test]
async fn save_creates_then_replaces_only_the_overlay_and_returns_a_fresh_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let manual = "# keep my comment\nschema_version: 1\nmodel: base\nscope:\n  write: ['src/**']\n";
    std::fs::write(path.join("agent.yaml"), manual).unwrap();
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let baseline = read(&server, &member).await;
    assert_eq!(baseline["saveSupported"], true);
    let saved = value(
        save(
            &server,
            &member,
            &baseline["fileVersion"],
            json!({"schema_version":1,"model":"  new-model  ","scope":{"write":[]}}),
        )
        .await,
    );
    assert_eq!(saved["effective"]["model"], "new-model");
    assert_eq!(saved["effective"]["scope"]["write"], json!([]));
    assert_eq!(
        saved["overlay"],
        json!({"schema_version":1,"model":"new-model","scope":{"write":[]}})
    );
    assert_ne!(saved["fileVersion"], baseline["fileVersion"]);
    assert_eq!(read(&server, &member).await, saved);
    let final_result = value(
        save(
            &server,
            &member,
            &saved["fileVersion"],
            json!({"schema_version":1}),
        )
        .await,
    );
    assert_eq!(final_result["effective"]["model"], "base");
    assert_eq!(
        final_result["effective"]["scope"]["write"],
        json!(["src/**"])
    );
    assert_eq!(
        std::fs::read_to_string(path.join("agent.yaml")).unwrap(),
        manual
    );
    assert!(!path.join(".onehall/agent.yaml").exists());
    assert_eq!(list(&server).await, vec![member]);
    let files = std::fs::read_dir(path.join(".onehall"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2); // overlay and its persistent cooperative lock
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path.join(".onehall/agent.ui.yaml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn invalid_stale_and_unauthorized_saves_do_not_create_an_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let baseline = read(&server, &member).await;
    for invalid in [
        json!({"schema_version":2}),
        json!({"schema_version":1,"model":null}),
        json!({"schema_version":1,"unknown-private":"private-value"}),
        json!({"schema_version":1,"scope":{"write":["../outside"]}}),
        json!({"schema_version":1,"model":"x".repeat(65537)}),
    ] {
        let response = save(&server, &member, &baseline["fileVersion"], invalid).await;
        response.assert_status_bad_request();
        assert!(!response.text().contains("private"));
    }
    server
        .post("/api/hall_save_member_config")
        .json(&json!({}))
        .await
        .assert_status_unauthorized();
    let stale = json!({"memberId":member["memberId"],"revision":2});
    save(
        &server,
        &stale,
        &baseline["fileVersion"],
        json!({"schema_version":1}),
    )
    .await
    .assert_status_conflict();
    save(&server, &member, &json!("bad"), json!({"schema_version":1}))
        .await
        .assert_status_bad_request();
    let unknown = json!({"memberId":uuid::Uuid::new_v4().to_string(),"revision":1});
    save(
        &server,
        &unknown,
        &baseline["fileVersion"],
        json!({"schema_version":1}),
    )
    .await
    .assert_status_not_found();
    let forged = server.post("/api/hall_save_member_config").add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":1,"expectedFileVersion":baseline["fileVersion"],
            "draft":{"schema_version":1},"path":"/tmp/outside"})).await;
    assert!(forged.status_code().is_client_error());
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
    std::fs::write(
        path.join("agent.yaml"),
        "# outside edit\nschema_version: 1\n",
    )
    .unwrap();
    save(
        &server,
        &member,
        &baseline["fileVersion"],
        json!({"schema_version":1}),
    )
    .await
    .assert_status_conflict();
    assert!(!path.join(".onehall").exists());
    assert_eq!(list(&server).await, vec![member]);
}

#[tokio::test]
async fn two_saves_of_the_same_observed_version_have_only_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let baseline = read(&server, &member).await;
    let (a, b) = tokio::join!(
        save(
            &server,
            &member,
            &baseline["fileVersion"],
            json!({"schema_version":1,"model":"a"})
        ),
        save(
            &server,
            &member,
            &baseline["fileVersion"],
            json!({"schema_version":1,"model":"b"})
        ),
    );
    let statuses = [a.status_code().as_u16(), b.status_code().as_u16()];
    assert!(
        statuses == [200, 409] || statuses == [409, 200],
        "{statuses:?}"
    );
    let current = read(&server, &member).await;
    assert_eq!(current["valid"], true);
    assert_eq!(list(&server).await, vec![member]);
}

#[tokio::test]
async fn a_key_only_draft_reports_its_source_and_can_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = member_dir(dir.path(), "member");
    let server = server(fresh_in_memory_db().await, dir.path());
    let member = value(register(&server, draft(&path)).await);
    let baseline = read(&server, &member).await;
    let overlay = json!({"schema_version":1,"onedesk_key":"read"});
    let proposed = value(server.post("/api/hall_preview_member_config_draft")
        .add_header("authorization", TEST_AUTH)
        .json(&json!({"memberId":member["memberId"],"expectedRevision":member["revision"],"expectedFileVersion":baseline["fileVersion"],"draft":overlay}))
        .await);
    assert_eq!(proposed["valid"], true);
    let changes = proposed["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["field"], "onedesk_key");
    assert_eq!(changes[0]["before"], "none");
    assert_eq!(changes[0]["after"], "read");
    assert_eq!(changes[0]["afterSource"], ".onehall/agent.ui.yaml");
    let saved = value(save(&server, &member, &baseline["fileVersion"], overlay).await);
    assert_eq!(saved["effective"]["onedesk_key"], "read");
    assert_eq!(saved["overlay"]["onedesk_key"], "read");
    let row = saved["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["field"] == "onedesk_key")
        .unwrap();
    assert_eq!(row["source"], ".onehall/agent.ui.yaml");
    assert_eq!(read(&server, &member).await, saved);
    assert!(!path.join(".onedesk").exists());
}
