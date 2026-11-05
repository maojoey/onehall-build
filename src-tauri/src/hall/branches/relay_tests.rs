//! Real loopback upstreams catch credential forwarding, redirect and body bugs.
use super::{model::*, service};
use crate::{db::test_helpers::fresh_in_memory_db, web::auth::WebAuth};
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};

async fn upstream(app: Router) -> axum_test::TestServer {
    axum_test::TestServer::builder()
        .http_transport()
        .build(app)
        .unwrap()
}
async fn register(db: &sea_orm::DatabaseConnection, url: String, token: &str) -> HallBranchSummary {
    service::register(
        db,
        RegisterBranchInput {
            name: token.into(),
            base_url: url,
            token: token.into(),
            headers: vec![crate::models::RemoteWorkspaceHeader {
                name: "X-Branch-Key".into(),
                value: format!("key-{token}"),
            }],
        },
    )
    .await
    .unwrap()
}
fn relay(db: sea_orm::DatabaseConnection) -> axum_test::TestServer {
    axum_test::TestServer::new(super::relay::routes(
        db,
        WebAuth {
            token: "hq-secret".into(),
            proxy: None,
        },
    ))
    .unwrap()
}
#[tokio::test]
async fn branch_requests_use_only_registered_credentials_and_keep_same_ids_isolated() {
    async fn echo(headers: HeaderMap) -> Json<Value> {
        Json(
            json!({"id":1,"auth":headers.get("authorization").unwrap().to_str().unwrap(),"key":headers.get("x-branch-key").unwrap().to_str().unwrap(),"cookie":headers.get("cookie").is_some(),"forwarded":headers.get("x-forwarded-user").is_some()}),
        )
    }
    let a = upstream(Router::new().route("/api/list_folders", post(echo))).await;
    let b = upstream(Router::new().route("/api/list_folders", post(echo))).await;
    let db = fresh_in_memory_db().await;
    let ba = register(&db.conn, a.server_url("/").unwrap().to_string(), "branch-a").await;
    let bb = register(&db.conn, b.server_url("/").unwrap().to_string(), "branch-b").await;
    let hq = relay(db.conn);
    for (branch, token) in [(ba, "branch-a"), (bb, "branch-b")] {
        let r = hq
            .post(&format!("/branches/{}/api/list_folders", branch.id))
            .authorization_bearer("hq-secret")
            .add_header("cookie", "hq-cookie")
            .add_header("x-forwarded-user", "hq-user")
            .add_header("x-branch-key", "client-controlled")
            .json(&json!({}))
            .await;
        r.assert_status_ok();
        assert_eq!(
            r.json::<Value>(),
            json!({"id":1,"auth":format!("Bearer {token}"),"key":format!("key-{token}"),"cookie":false,"forwarded":false})
        );
        assert_eq!(r.header("cache-control"), "no-store");
    }
}
#[tokio::test]
async fn relay_distinguishes_hq_auth_from_branch_auth_and_denies_unsafe_commands() {
    let u = upstream(
        Router::new().fallback(|| async { (StatusCode::UNAUTHORIZED, "upstream-secret") }),
    )
    .await;
    let db = fresh_in_memory_db().await;
    let branch = register(&db.conn, u.server_url("/").unwrap().to_string(), "branch-a").await;
    let hq = relay(db.conn.clone());
    let path = format!("/branches/{}/api/list_folders", branch.id);
    hq.post(&path).await.assert_status_unauthorized();
    let failed = hq
        .post(&path)
        .authorization_bearer("hq-secret")
        .json(&json!({}))
        .await;
    failed.assert_status(StatusCode::BAD_GATEWAY);
    assert_eq!(failed.json::<Value>()["code"], "branch_auth_failed");
    assert!(failed.maybe_header("www-authenticate").is_none());
    assert!(!failed.text().contains("upstream-secret"));
    for command in [
        "hall_register_branch",
        "hall_list_branches",
        "backup_create_ticket",
        "get_account_token",
        "restart_app",
        "browser_bridge_open",
        "officecli_render_html",
        "start_office_watch",
    ] {
        hq.post(&format!("/branches/{}/api/{command}", branch.id))
            .authorization_bearer("hq-secret")
            .json(&json!({}))
            .await
            .assert_status_forbidden();
    }
    service::set_enabled(&db.conn, &branch.id, false, 1)
        .await
        .unwrap();
    hq.post(&path)
        .authorization_bearer("hq-secret")
        .json(&json!({}))
        .await
        .assert_status_forbidden();
}
#[tokio::test]
async fn redirects_are_not_followed_and_html_never_becomes_hq_inline_content() {
    let u = upstream(
        Router::new()
            .route(
                "/api/list_folders",
                post(|| async {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [("location", "http://127.0.0.1:1/stolen")],
                        "redirect-secret",
                    )
                }),
            )
            .route(
                "/api/get_folder",
                post(|| async {
                    (
                        [("content-type", "text/html"), ("set-cookie", "stolen=1")],
                        "<script>bad()</script>",
                    )
                }),
            ),
    )
    .await;
    let db = fresh_in_memory_db().await;
    let branch = register(&db.conn, u.server_url("/").unwrap().to_string(), "branch-a").await;
    let hq = relay(db.conn);
    for command in ["list_folders", "get_folder"] {
        let r = hq
            .post(&format!("/branches/{}/api/{command}", branch.id))
            .authorization_bearer("hq-secret")
            .json(&json!({}))
            .await;
        r.assert_status(StatusCode::BAD_GATEWAY);
        assert!(r.maybe_header("location").is_none());
        assert!(r.maybe_header("set-cookie").is_none());
        assert!(!r.text().contains("<script>"));
    }
}
#[tokio::test]
async fn multipart_is_forwarded_byte_for_byte_with_boundary() {
    async fn uploaded(headers: HeaderMap, bytes: Bytes) -> Json<Value> {
        Json(
            json!({"contentType":headers["content-type"].to_str().unwrap(),"bytes":bytes.to_vec()}),
        )
    }
    let u = upstream(
        Router::new()
            .route("/api/upload_workspace_file", post(uploaded))
            .route("/api/upload_attachment", post(uploaded)),
    )
    .await;
    let db = fresh_in_memory_db().await;
    let branch = register(&db.conn, u.server_url("/").unwrap().to_string(), "branch-a").await;
    let hq = relay(db.conn);
    let bytes=Bytes::from_static(b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\r\n\x00\xff\x80\r\n--boundary--\r\n");
    for command in ["upload_workspace_file", "upload_attachment"] {
        let r = hq
            .post(&format!("/branches/{}/api/{command}", branch.id))
            .authorization_bearer("hq-secret")
            .content_type("multipart/form-data; boundary=boundary")
            .bytes(bytes.clone())
            .await;
        r.assert_status_ok();
        assert_eq!(
            r.json::<Value>(),
            json!({"contentType":"multipart/form-data; boundary=boundary","bytes":bytes.to_vec()})
        );
    }
}
#[tokio::test]
async fn websocket_connects_upstream_first_with_branch_auth_and_relays_frames() {
    use axum::extract::ws::{Message as AxumMessage, WebSocketUpgrade};
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };
    let u = upstream(Router::new().route(
        "/ws/events",
        axum::routing::get(|headers: HeaderMap, ws: WebSocketUpgrade| async move {
            let auth = headers["authorization"].to_str().unwrap().to_owned();
            let protocols = headers["sec-websocket-protocol"]
                .to_str()
                .unwrap()
                .to_owned();
            ws.protocols(["codeg-events"])
                .on_upgrade(move |mut s| async move {
                    s.send(AxumMessage::Text(
                        json!({"event":"__ready__","auth":auth,"protocol":protocols})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                    // A normal snapshot can exceed the finished-tool text budget
                    // once generated images or running-tool data are included.
                    s.send(AxumMessage::Text(json!({"type":"snapshot","subscriptionId":"1","content":"x".repeat(3*1024*1024)}).to_string().into())).await.unwrap();
                    while let Some(Ok(frame)) = s.recv().await {
                        let close = matches!(frame, AxumMessage::Close(_));
                        if s.send(frame).await.is_err() || close {
                            break;
                        }
                    }
                })
        }),
    ))
    .await;
    let db = fresh_in_memory_db().await;
    let branch = register(&db.conn, u.server_url("/").unwrap().to_string(), "branch-a").await;
    let hq = upstream(super::relay::routes(
        db.conn.clone(),
        WebAuth {
            token: "hq-secret".into(),
            proxy: None,
        },
    ))
    .await;
    let mut url = hq
        .server_url(&format!("/branches/{}/ws/events", branch.id))
        .unwrap();
    url.set_scheme("ws").unwrap();
    let mut request = url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer hq-secret".parse().unwrap());
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "codeg-events".parse().unwrap());
    let (mut ws, response) = connect_async(request).await.unwrap();
    assert_eq!(response.headers()["sec-websocket-protocol"], "codeg-events");
    let ready = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ready: Value = serde_json::from_str(ready.to_text().unwrap()).unwrap();
    assert_eq!(ready["auth"], "Bearer branch-a");
    assert_eq!(ready["protocol"], "codeg-events");
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot: Value = serde_json::from_str(snapshot.to_text().unwrap()).unwrap();
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["content"].as_str().unwrap().len(), 3 * 1024 * 1024);

    ws.send(Message::Text(
        r#"{"type":"attach","subscriptionId":"1"}"#.into(),
    ))
    .await
    .unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap().to_text().unwrap(),
        r#"{"type":"attach","subscriptionId":"1"}"#
    );
    ws.send(Message::Binary(vec![0, 255, 128].into()))
        .await
        .unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap().into_data().as_ref(),
        &[0, 255, 128]
    );
    // Disabling blocks new traffic, but must not interrupt work already
    // submitted to the immutable target captured by this event connection.
    service::set_enabled(&db.conn, &branch.id, false, 1)
        .await
        .unwrap();
    ws.send(Message::Text("still-running".into()))
        .await
        .unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap().to_text().unwrap(),
        "still-running"
    );
    let mut retry = url.as_str().into_client_request().unwrap();
    retry
        .headers_mut()
        .insert("authorization", "Bearer hq-secret".parse().unwrap());
    retry
        .headers_mut()
        .insert("sec-websocket-protocol", "codeg-events".parse().unwrap());
    assert!(
        matches!(connect_async(retry).await,Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status()==StatusCode::FORBIDDEN)
    );
    ws.close(None).await.unwrap();
}
#[tokio::test]
async fn wrapped_download_is_one_use_attachment_and_invalidated_by_registration_revision() {
    use axum::routing::get;
    let u=upstream(Router::new()
        .route("/api/workspace_download_ticket",post(||async{Json(json!({"ticket":"0123456789abcdef0123456789abcdef","url":"http://untrusted.example/private","filename":"page.html","expiresAt":4102444800i64}))}))
        .route("/api/workspace_download/0123456789abcdef0123456789abcdef",get(||async{([( "content-type","text/html"),("set-cookie","bad=1"),("content-disposition","inline")],"<script>file()</script>")}))).await;
    let db = fresh_in_memory_db().await;
    let branch = register(&db.conn, u.server_url("/").unwrap().to_string(), "branch-a").await;
    let other = register(
        &db.conn,
        u.server_url("/other").unwrap().to_string(),
        "branch-b",
    )
    .await;
    let hq = relay(db.conn.clone());
    let path = format!("/branches/{}/api/workspace_download_ticket", branch.id);
    let issue = hq
        .post(&path)
        .authorization_bearer("hq-secret")
        .json(&json!({"kind":"file","rootPath":"/tmp","path":"page.html"}))
        .await;
    issue.assert_status_ok();
    let issued: Value = issue.json();
    assert!(!issued
        .to_string()
        .contains("0123456789abcdef0123456789abcdef"));
    assert!(!issued.to_string().contains("untrusted.example"));
    let url = issued["url"].as_str().unwrap();
    hq.get(&format!("/branches/{}{url}", other.id))
        .await
        .assert_status_not_found();
    let downloaded = hq.get(&format!("/branches/{}{url}", branch.id)).await;
    downloaded.assert_status_ok();
    assert_eq!(downloaded.text(), "<script>file()</script>");
    assert!(downloaded
        .header("content-disposition")
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    assert!(downloaded.maybe_header("set-cookie").is_none());
    assert_eq!(downloaded.header("x-content-type-options"), "nosniff");
    hq.get(url).await.assert_status_not_found();
    let issue = hq
        .post(&path)
        .authorization_bearer("hq-secret")
        .json(&json!({}))
        .await
        .json::<Value>();
    service::set_enabled(&db.conn, &branch.id, false, 1)
        .await
        .unwrap();
    service::set_enabled(&db.conn, &branch.id, true, 2)
        .await
        .unwrap();
    hq.get(issue["url"].as_str().unwrap())
        .await
        .assert_status_forbidden();
}
#[tokio::test]
async fn pi_project_trust_launch_gate_commands_reach_the_selected_workspace() {
    async fn echo(Json(input): Json<Value>) -> Json<Value> {
        Json(input)
    }
    let u = upstream(
        Router::new()
            .route("/api/acp_pi_project_trust_state", post(echo))
            .route("/api/acp_pi_set_project_trust", post(echo))
            .route("/api/acp_pi_acknowledge_project_trust", post(echo)),
    )
    .await;
    let db = fresh_in_memory_db().await;
    let branch = register(&db.conn, u.server_url("/").unwrap().to_string(), "branch-a").await;
    let hq = relay(db.conn);
    for (command, input) in [
        (
            "acp_pi_project_trust_state",
            json!({"workspace":"/project"}),
        ),
        (
            "acp_pi_set_project_trust",
            json!({"workspace":"/project","trusted":true}),
        ),
        (
            "acp_pi_acknowledge_project_trust",
            json!({"workspace":"/project"}),
        ),
    ] {
        let result = hq
            .post(&format!("/branches/{}/api/{command}", branch.id))
            .authorization_bearer("hq-secret")
            .json(&input)
            .await;
        result.assert_status_ok();
        assert_eq!(result.json::<Value>(), input);
    }
    hq.post(&format!(
        "/branches/{}/api/acp_antigravity_login_start",
        branch.id
    ))
    .authorization_bearer("hq-secret")
    .json(&json!({}))
    .await
    .assert_status_forbidden();
}
