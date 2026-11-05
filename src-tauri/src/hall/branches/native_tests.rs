use super::{model::*, service};
use crate::{
    db::service::remote_workspace_connection_service as remote,
    db::test_helpers::fresh_in_memory_db,
};
use axum::{routing::post, Json, Router};
use serde_json::json;
#[tokio::test]
async fn derived_connections_are_stable_concurrent_and_refresh_hq_credentials_only() {
    let id = uuid::Uuid::new_v4().to_string();
    let summary = HallBranchSummary {
        id: id.clone(),
        name: "A".into(),
        enabled: true,
        revision: 1,
        status: BranchStatus::Unknown,
        checked_at: None,
    };
    let server = axum_test::TestServer::builder()
        .http_transport()
        .build(Router::new().route(
            "/prefix/api/hall_list_branches",
            post(move || {
                let s = summary.clone();
                async move { Json(vec![s]) }
            }),
        ))
        .unwrap();
    let db = fresh_in_memory_db().await;
    let hq = remote::create(
        &db.conn,
        "HQ",
        server.server_url("/prefix").unwrap().as_str(),
        "hq-secret",
        &[],
    )
    .await
    .unwrap();
    let (a, b) = tokio::join!(
        service::connect_branch(&db.conn, hq.id, &id),
        service::connect_branch(&db.conn, hq.id, &id)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.connection_id, b.connection_id);
    assert_ne!(a.connection_id, hq.id);
    let derived = remote::get(&db.conn, a.connection_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(derived.base_url, format!("{}/branches/{id}", hq.base_url));
    assert_eq!(derived.token, "hq-secret");
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        json!({"connectionId":a.connection_id,"name":"A"})
    );
    remote::update(
        &db.conn,
        hq.id,
        "HQ",
        &hq.base_url,
        "changed-hq-secret",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        service::connect_branch(&db.conn, hq.id, &id)
            .await
            .unwrap()
            .connection_id,
        a.connection_id
    );
    assert_eq!(
        remote::get(&db.conn, a.connection_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        "changed-hq-secret"
    );
    assert_eq!(
        remote::get(&db.conn, hq.id)
            .await
            .unwrap()
            .unwrap()
            .base_url,
        hq.base_url
    );
    assert!(service::connect_branch(&db.conn, a.connection_id, &id)
        .await
        .is_err());
    assert!(service::connect_branch(&db.conn, hq.id, "../evil")
        .await
        .is_err());
    assert!(
        service::connect_branch(&db.conn, hq.id, &uuid::Uuid::new_v4().to_string())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn changing_headquarters_credentials_during_discovery_cannot_save_a_stale_derived_connection()
{
    use std::sync::Arc;
    use tokio::sync::{Notify, Semaphore};
    let id = uuid::Uuid::new_v4().to_string();
    let summary = HallBranchSummary {
        id: id.clone(),
        name: "A".into(),
        enabled: true,
        revision: 1,
        status: BranchStatus::Unknown,
        checked_at: None,
    };
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let server = axum_test::TestServer::builder()
        .http_transport()
        .build(Router::new().route(
            "/api/hall_list_branches",
            post({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let summary = summary.clone();
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        let _permit = release.acquire().await.unwrap();
                        Json(vec![summary])
                    }
                }
            }),
        ))
        .unwrap();
    let db = fresh_in_memory_db().await;
    let hq = remote::create(
        &db.conn,
        "HQ",
        server.server_url("/").unwrap().as_str(),
        "old-hq-secret",
        &[],
    )
    .await
    .unwrap();
    let conn = db.conn.clone();
    let hq_id = hq.id;
    let open = tokio::spawn(async move { service::connect_branch(&conn, hq_id, &id).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    remote::update(&db.conn, hq.id, "HQ", &hq.base_url, "new-hq-secret", &[])
        .await
        .unwrap();
    release.add_permits(1);
    assert!(open.await.unwrap().is_err());
    assert_eq!(remote::list(&db.conn).await.unwrap().len(), 1);
}
