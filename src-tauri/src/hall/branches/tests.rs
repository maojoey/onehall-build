use super::{model::*, service};
use crate::db::test_helpers::fresh_in_memory_db;
fn input(url: &str) -> RegisterBranchInput {
    RegisterBranchInput {
        name: "Branch A".into(),
        base_url: url.into(),
        token: "branch-secret".into(),
        headers: vec![],
    }
}
#[test]
fn endpoint_allows_explicit_private_overlay_and_prefix() {
    assert_eq!(
        service::validate_endpoint(" http://100.101.102.103:3080/prefix/ ").unwrap(),
        "http://100.101.102.103:3080/prefix"
    );
}
#[test]
fn endpoint_refuses_credentials_ambiguous_paths_and_recursive_relays() {
    for url in [
        "file:///tmp/a",
        "http://user:secret@host",
        "https://host?token=bad",
        "http://host/#frag",
        "http://host/branches/abc",
        "http://host/%62ranches/abc",
        "http://host/a/../x",
        "http://host/a%2fb",
        "http://host/\\path",
        "http://host/a//b",
    ] {
        assert!(service::validate_endpoint(url).is_err(), "accepted {url}");
    }
}
#[tokio::test]
async fn registration_returns_public_summary_without_credentials_or_url() {
    let db = fresh_in_memory_db().await;
    let summary = service::register(&db.conn, input("http://127.0.0.1:3080"))
        .await
        .unwrap();
    assert!(uuid::Uuid::parse_str(&summary.id).is_ok());
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"id":summary.id,"name":"Branch A","enabled":true,"revision":1,"status":"unknown","checkedAt":null})
    );
    assert!(!json.to_string().contains("secret"));
    assert!(!json.to_string().contains("127.0.0.1"));
}
#[tokio::test]
async fn enabled_revision_is_atomic_and_list_never_needs_branch_network() {
    let db = fresh_in_memory_db().await;
    let branch = service::register(&db.conn, input("http://127.0.0.1:1"))
        .await
        .unwrap();
    let updated = service::set_enabled(&db.conn, &branch.id, false, 1)
        .await
        .unwrap();
    assert!(!updated.enabled);
    assert_eq!(updated.revision, 2);
    assert!(service::set_enabled(&db.conn, &branch.id, true, 1)
        .await
        .is_err());
    let listed = service::list(&db.conn).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].enabled);
    assert_eq!(listed[0].status, BranchStatus::Unknown);
    assert!(service::set_enabled(&db.conn, "../not-a-uuid", true, 2)
        .await
        .is_err());
}
#[tokio::test]
async fn registry_caps_branches_and_rejects_reserved_identity_headers() {
    let db = fresh_in_memory_db().await;
    let mut bad = input("http://127.0.0.1:3080");
    bad.headers.push(crate::models::RemoteWorkspaceHeader {
        name: "Cookie".into(),
        value: "secret".into(),
    });
    assert!(service::register(&db.conn, bad).await.is_err());
    for i in 0..32 {
        service::register(&db.conn, input(&format!("http://127.0.0.1:{}", 3080 + i)))
            .await
            .unwrap();
    }
    assert!(service::register(&db.conn, input("http://127.0.0.1:3080"))
        .await
        .is_err());
    assert_eq!(service::list(&db.conn).await.unwrap().len(), 32);
}
#[tokio::test]
async fn probe_records_only_redacted_status_for_authenticated_health() {
    use axum::{
        http::{HeaderMap, StatusCode},
        routing::post,
        Router,
    };
    let upstream = axum_test::TestServer::builder()
        .http_transport()
        .build(
            Router::new()
                .route(
                    "/api/health",
                    post(|headers: HeaderMap| async move {
                        if headers["authorization"] == "Bearer branch-secret" {
                            (StatusCode::OK, "{}")
                        } else {
                            (StatusCode::UNAUTHORIZED, "secret error")
                        }
                    }),
                )
                .route(
                    "/bad/api/health",
                    post(|| async { (StatusCode::UNAUTHORIZED, "secret error") }),
                ),
        )
        .unwrap();
    let db = fresh_in_memory_db().await;
    let good = service::register(&db.conn, input(upstream.server_url("/").unwrap().as_str()))
        .await
        .unwrap();
    let checked = service::probe(&db.conn, &good.id).await.unwrap();
    assert_eq!(checked.status, BranchStatus::Online);
    assert!(checked.checked_at.is_some());
    let mut bad = input(upstream.server_url("/bad").unwrap().as_str());
    bad.token = "wrong".into();
    let bad = service::register(&db.conn, bad).await.unwrap();
    assert_eq!(
        service::probe(&db.conn, &bad.id).await.unwrap().status,
        BranchStatus::AuthFailed
    );
    let offline = service::register(&db.conn, input("http://127.0.0.1:1"))
        .await
        .unwrap();
    assert_eq!(
        service::probe(&db.conn, &offline.id).await.unwrap().status,
        BranchStatus::Unreachable
    );
    assert!(service::probe(&db.conn, "../../health").await.is_err());
}
