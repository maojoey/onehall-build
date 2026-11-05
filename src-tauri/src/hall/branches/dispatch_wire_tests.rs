use super::{dispatch_wire, trust};
use crate::db::test_helpers::fresh_in_memory_db;
use serde_json::{json, Value};

fn envelope(id: &str) -> Value {
    json!({"dispatchId":id,"memberRef":"local-worker","title":"Test dispatch","description":"Prepare the isolated test report."})
}

#[tokio::test]
async fn dedicated_intake_requires_issuer_credential_and_cannot_confirm_or_launch() {
    let db = fresh_in_memory_db().await;
    let issuer = trust::create_local(&db.conn, "HQ").await.unwrap();
    let credential = format!("{}.{}", issuer.issuer_id, issuer.secret);
    let server = axum_test::TestServer::new(dispatch_wire::routes(db.conn.clone(), None)).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    for token in ["", "ordinary-web-token", &issuer.secret] {
        server
            .post("/hall-dispatch/v1/inbox")
            .authorization_bearer(token)
            .json(&envelope(&id))
            .await
            .assert_status_unauthorized();
    }
    let first = server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&credential)
        .json(&envelope(&id))
        .await;
    first.assert_status_ok();
    let receipt = first.json::<Value>();
    assert_eq!(receipt["status"], "awaiting_confirmation");
    assert_eq!(receipt["launchAllowed"], false);
    assert!(!first.text().contains(&issuer.secret));
    let repeat = server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&credential)
        .json(&envelope(&id))
        .await;
    assert_eq!(repeat.json::<Value>(), receipt);
    let detail = server
        .get(&format!("/hall-dispatch/v1/inbox/{id}"))
        .authorization_bearer(&credential)
        .await;
    detail.assert_status_ok();
    assert_eq!(
        detail.json::<Value>()["description"],
        envelope(&id)["description"]
    );
    for suffix in ["accept", "reject", "launch", "trust"] {
        server
            .post(&format!("/hall-dispatch/v1/{suffix}"))
            .authorization_bearer(&credential)
            .json(&json!({}))
            .await
            .assert_status_not_found();
    }
}

#[tokio::test]
async fn wire_intake_rejects_forged_identity_conflicts_and_revoked_access() {
    let db = fresh_in_memory_db().await;
    let a = trust::create_local(&db.conn, "A").await.unwrap();
    let b = trust::create_local(&db.conn, "B").await.unwrap();
    let token = format!("{}.{}", a.issuer_id, a.secret);
    let server = axum_test::TestServer::new(dispatch_wire::routes(db.conn.clone(), None)).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&token)
        .json(&envelope(&id))
        .await
        .assert_status_ok();
    let mut forged = envelope(&id);
    forged["issuerId"] = b.issuer_id.clone().into();
    server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&token)
        .json(&forged)
        .await
        .assert_status_unprocessable_entity();
    let mut changed = envelope(&id);
    changed["description"] = "Different request".into();
    server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&token)
        .json(&changed)
        .await
        .assert_status_conflict();
    let other = format!("{}.{}", b.issuer_id, b.secret);
    server
        .get(&format!("/hall-dispatch/v1/inbox/{id}"))
        .authorization_bearer(&other)
        .await
        .assert_status_not_found();
    trust::revoke_local(&db.conn, &a.issuer_id, 1)
        .await
        .unwrap();
    server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(&token)
        .json(&envelope(&id))
        .await
        .assert_status_unauthorized();
    server
        .get(&format!("/hall-dispatch/v1/inbox/{id}"))
        .authorization_bearer(&token)
        .await
        .assert_status_unauthorized();
}

#[tokio::test]
async fn wire_intake_bounds_the_request_before_deserializing() {
    let db = fresh_in_memory_db().await;
    let a = trust::create_local(&db.conn, "HQ").await.unwrap();
    let server = axum_test::TestServer::new(dispatch_wire::routes(db.conn, None)).unwrap();
    let mut body = envelope(&uuid::Uuid::new_v4().to_string());
    body["description"] = "x".repeat(70 * 1024).into();
    server
        .post("/hall-dispatch/v1/inbox")
        .authorization_bearer(format!("{}.{}", a.issuer_id, a.secret))
        .json(&body)
        .await
        .assert_status_payload_too_large();
}
