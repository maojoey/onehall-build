use super::{api, service};
use crate::{
    db::test_helpers::fresh_in_memory_db,
    web::auth::{self, WebAuth},
};
use axum::{middleware, Router};
use serde_json::{json, Value};

#[tokio::test]
async fn authenticated_management_routes_return_only_public_dtos_and_enforce_revisions() {
    let db = fresh_in_memory_db().await;
    let web_auth = WebAuth {
        token: "hq-secret".into(),
        proxy: None,
    };
    let router = Router::new()
        .nest("/api", api::routes(db.conn.clone(), None))
        .layer(middleware::from_fn(move |request, next| {
            auth::require_auth(request, next, web_auth.clone())
        }));
    let hq = axum_test::TestServer::new(router).unwrap();
    hq.post("/api/hall_list_branches")
        .json(&json!({}))
        .await
        .assert_status_unauthorized();
    let input = json!({"input":{"name":"A","baseUrl":"http://127.0.0.1:3080","token":"branch-secret","headers":[{"name":"X-Branch-Key","value":"custom-secret"}]}});
    hq.post("/api/hall_register_branch")
        .json(&input)
        .await
        .assert_status_unauthorized();
    let registered = hq
        .post("/api/hall_register_branch")
        .authorization_bearer("hq-secret")
        .json(&input)
        .await;
    registered.assert_status_ok();
    let summary: Value = registered.json();
    let id = summary["id"].as_str().unwrap();
    assert!(!registered.text().contains("secret"));
    assert!(!registered.text().contains("127.0.0.1"));
    let listed = hq
        .post("/api/hall_list_branches")
        .authorization_bearer("hq-secret")
        .json(&json!({}))
        .await;
    assert_eq!(listed.json::<Value>(), json!([summary]));
    let disabled = hq
        .post("/api/hall_set_branch_enabled")
        .authorization_bearer("hq-secret")
        .json(&json!({"branchId":id,"enabled":false,"expectedRevision":1}))
        .await;
    disabled.assert_status_ok();
    assert_eq!(disabled.json::<Value>()["revision"], 2);
    hq.post("/api/hall_set_branch_enabled")
        .authorization_bearer("hq-secret")
        .json(&json!({"branchId":id,"enabled":true,"expectedRevision":1}))
        .await
        .assert_status_conflict();
    hq.post("/api/hall_probe_branch")
        .authorization_bearer("hq-secret")
        .json(&json!({"branchId":id}))
        .await
        .assert_status_forbidden();
    assert!(!service::list(&db.conn).await.unwrap()[0].enabled);
}
