use super::{model::RegisterBranchInput, relay, service};
use crate::{db::test_helpers::fresh_in_memory_db, web::auth::WebAuth};
use axum::{http::StatusCode, routing::post, Json, Router};
use serde_json::json;
fn auth() -> WebAuth {
    WebAuth {
        token: "hq-secret".into(),
        proxy: None,
    }
}
async fn branch(db: &sea_orm::DatabaseConnection, base_url: String) -> String {
    service::register(
        db,
        RegisterBranchInput {
            name: "A".into(),
            base_url,
            token: "branch-secret".into(),
            headers: vec![],
        },
    )
    .await
    .unwrap()
    .id
}
#[tokio::test]
async fn relay_revalidates_headers_loaded_from_existing_connection_configuration() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let upstream = axum_test::TestServer::builder()
        .http_transport()
        .build(Router::new().route("/api/list_folders", post(|| async { Json(json!([])) })))
        .unwrap();
    let db = fresh_in_memory_db().await;
    let id = branch(&db.conn, upstream.server_url("/").unwrap().to_string()).await;
    let connection_id: i32 = db
        .conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT connection_id FROM hall_branch".to_string(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "connection_id")
        .unwrap();
    // Simulate a legacy/direct configuration edit bypassing registration.
    db.conn
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE remote_workspace_connection SET headers=? WHERE id=?",
            [
                r#"[{"name":"Cookie","value":"invalid-stored-secret"}]"#.into(),
                connection_id.into(),
            ],
        ))
        .await
        .unwrap();
    let hq = axum_test::TestServer::new(relay::routes(db.conn, auth())).unwrap();
    hq.post(&format!("/branches/{id}/api/list_folders"))
        .authorization_bearer("hq-secret")
        .json(&json!({}))
        .await
        .assert_status_bad_request();
}
#[tokio::test]
async fn hidden_registry_connections_do_not_break_ordinary_desktop_connection_reordering() {
    use crate::db::service::remote_workspace_connection_service as remote;
    let db = fresh_in_memory_db().await;
    let a = remote::create(&db.conn, "HQ A", "http://127.0.0.1:3001", "hq-a", &[])
        .await
        .unwrap();
    let b = remote::create(&db.conn, "HQ B", "http://127.0.0.1:3002", "hq-b", &[])
        .await
        .unwrap();
    branch(&db.conn, "http://127.0.0.1:3003".into()).await;
    let listed = remote::list(&db.conn).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|c| c.token != "branch-secret"));
    remote::reorder(&db.conn, vec![b.id, a.id]).await.unwrap();
    assert_eq!(
        remote::list(&db.conn)
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        vec![b.id, a.id]
    );
}
#[tokio::test]
async fn http_relay_bounds_known_and_streamed_body_sizes() {
    use axum::body::Bytes;
    let u = axum_test::TestServer::builder()
        .http_transport()
        .build(Router::new().route(
            "/api/save_file_content",
            post(|_: Bytes| async { Json(json!({"saved":true})) }),
        ))
        .unwrap();
    let db = fresh_in_memory_db().await;
    let id = branch(&db.conn, u.server_url("/").unwrap().to_string()).await;
    let limits = relay::Limits {
        request_bytes: 8,
        ..Default::default()
    };
    let hq =
        axum_test::TestServer::new(relay::routes_with_limits(db.conn, auth(), limits)).unwrap();
    let path = format!("/branches/{id}/api/save_file_content");
    hq.post(&path)
        .authorization_bearer("hq-secret")
        .json(&json!({"payload":"oversized"}))
        .await
        .assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    hq.post(&path)
        .authorization_bearer("hq-secret")
        .add_header("content-length", "100")
        .content_type("application/json")
        .text("{}")
        .await
        .assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}
#[tokio::test]
async fn http_relay_deadline_is_a_gateway_timeout_not_hq_auth_failure() {
    use std::time::Duration;
    let u = axum_test::TestServer::builder()
        .http_transport()
        .build(Router::new().route(
            "/api/list_folders",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Json(json!([]))
            }),
        ))
        .unwrap();
    let db = fresh_in_memory_db().await;
    let id = branch(&db.conn, u.server_url("/").unwrap().to_string()).await;
    let limits = relay::Limits {
        http_timeout: Duration::from_millis(20),
        ..Default::default()
    };
    let hq =
        axum_test::TestServer::new(relay::routes_with_limits(db.conn, auth(), limits)).unwrap();
    let r = hq
        .post(&format!("/branches/{id}/api/list_folders"))
        .authorization_bearer("hq-secret")
        .json(&json!({}))
        .await;
    r.assert_status(StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(r.json::<serde_json::Value>()["code"], "branch_timeout");
    assert!(r.maybe_header("www-authenticate").is_none());
}
#[tokio::test]
async fn http_relay_concurrency_slot_remains_held_until_response_finishes() {
    use std::sync::Arc;
    use tokio::sync::{Notify, Semaphore};
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let u = axum_test::TestServer::builder()
        .http_transport()
        .build(Router::new().route(
            "/api/list_folders",
            post({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        let _permit = release.acquire().await.unwrap();
                        Json(json!([]))
                    }
                }
            }),
        ))
        .unwrap();
    let db = fresh_in_memory_db().await;
    let id = branch(&db.conn, u.server_url("/").unwrap().to_string()).await;
    let limits = relay::Limits {
        http_concurrency: 1,
        ..Default::default()
    };
    let hq = axum_test::TestServer::builder()
        .http_transport()
        .build(relay::routes_with_limits(db.conn, auth(), limits))
        .unwrap();
    let url = hq
        .server_url(&format!("/branches/{id}/api/list_folders"))
        .unwrap();
    let client = reqwest::Client::new();
    let request = client
        .post(url.clone())
        .bearer_auth("hq-secret")
        .json(&json!({}));
    let first = tokio::spawn(async move { request.send().await.unwrap().bytes().await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let second = client
        .post(url)
        .bearer_auth("hq-secret")
        .json(&json!({}))
        .timeout(std::time::Duration::from_secs(1))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        second.json::<serde_json::Value>().await.unwrap()["code"],
        "branch_busy"
    );
    release.add_permits(1);
    first.await.unwrap();
}
#[tokio::test]
async fn disabling_does_not_retarget_or_interrupt_a_captured_download() {
    use axum::{
        body::{Body, Bytes},
        response::Response,
        routing::get,
    };
    use futures::StreamExt;
    use std::{io, sync::Arc};
    use tokio::sync::Semaphore;
    let release = Arc::new(Semaphore::new(0));
    let u=axum_test::TestServer::builder().http_transport().build(Router::new()
 .route("/api/workspace_download_ticket",post(||async{Json(json!({"ticket":"0123456789abcdef0123456789abcdef","url":"ignored","filename":"a.html","expiresAt":4102444800i64}))}))
 .route("/api/workspace_download/0123456789abcdef0123456789abcdef",get({let release=release.clone();move||{let release=release.clone();async move{
 let chunks=futures::stream::once(async{Ok::<_,io::Error>(Bytes::from_static(b"first"))}).chain(futures::stream::once(async move{let _permit=release.acquire().await.unwrap();Ok::<_,io::Error>(Bytes::from_static(b"last"))}));
 Response::builder().header("content-type","text/html").body(Body::from_stream(chunks)).unwrap()
 }}}))).unwrap();
    let db = fresh_in_memory_db().await;
    let id = branch(&db.conn, u.server_url("/").unwrap().to_string()).await;
    let limits = relay::Limits {
        http_concurrency: 1,
        ..Default::default()
    };
    let hq = axum_test::TestServer::builder()
        .http_transport()
        .build(relay::routes_with_limits(db.conn.clone(), auth(), limits))
        .unwrap();
    let client = reqwest::Client::new();
    let issue = client
        .post(
            hq.server_url(&format!("/branches/{id}/api/workspace_download_ticket"))
                .unwrap(),
        )
        .bearer_auth("hq-secret")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let response = client
        .get(hq.server_url(issue["url"].as_str().unwrap()).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    let mut bytes = response.bytes_stream();
    assert_eq!(bytes.next().await.unwrap().unwrap().as_ref(), b"first");
    let busy = client
        .post(
            hq.server_url(&format!("/branches/{id}/api/workspace_download_ticket"))
                .unwrap(),
        )
        .bearer_auth("hq-secret")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    service::set_enabled(&db.conn, &id, false, 1).await.unwrap();
    release.add_permits(1);
    assert_eq!(bytes.next().await.unwrap().unwrap().as_ref(), b"last");
    assert!(bytes.next().await.is_none());
    let denied = client
        .post(
            hq.server_url(&format!("/branches/{id}/api/workspace_download_ticket"))
                .unwrap(),
        )
        .bearer_auth("hq-secret")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn manual_connection_apis_cannot_expose_or_modify_registry_credentials() {
    use crate::db::service::remote_workspace_connection_service as remote;
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let db = fresh_in_memory_db().await;
    branch(&db.conn, "http://127.0.0.1:3080".into()).await;
    let id: i32 = db
        .conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT connection_id FROM hall_branch".to_string(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "connection_id")
        .unwrap();
    assert!(remote::get(&db.conn, id).await.unwrap().is_none());
    assert!(remote::update(
        &db.conn,
        id,
        "Changed",
        "http://127.0.0.1:3081",
        "changed-secret",
        &[]
    )
    .await
    .is_err());
    assert_eq!(service::list(&db.conn).await.unwrap()[0].name, "A");
}
