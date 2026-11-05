use super::{dispatch, model::RegisterBranchInput, service};
use crate::db::test_helpers::fresh_in_memory_db;

fn input(branch_id: &str, submission_id: &str) -> dispatch::Draft {
    dispatch::Draft {
        submission_id: submission_id.into(),
        branch_id: branch_id.into(),
        member_ref: "windows-codex".into(),
        title: "Prepare report".into(),
        description: "Use the local project".into(),
        exchange_manifest_id: None,
    }
}

async fn branch(db: &sea_orm::DatabaseConnection) -> String {
    service::register(
        db,
        RegisterBranchInput {
            name: "Windows".into(),
            base_url: "http://127.0.0.1:33080".into(),
            token: "do-not-expose-secret".into(),
            headers: vec![],
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn submission_retries_return_one_waiting_record_and_conflicts_are_rejected() {
    let db = fresh_in_memory_db().await;
    let branch_id = branch(&db.conn).await;
    let submission = uuid::Uuid::new_v4().to_string();
    let first = dispatch::create(&db.conn, input(&branch_id, &submission))
        .await
        .unwrap();
    let second = dispatch::create(&db.conn, input(&branch_id, &submission))
        .await
        .unwrap();
    assert_eq!(first.dispatch_id, second.dispatch_id);
    assert_eq!(first.status, "pending_delivery");
    assert_eq!(first.revision, 1);
    let mut different = input(&branch_id, &submission);
    different.description = "Different work".into();
    assert!(dispatch::create(&db.conn, different).await.is_err());
    let rows = dispatch::list(&db.conn).await.unwrap();
    assert_eq!(rows.len(), 1);
    let output = serde_json::to_string(&rows).unwrap();
    assert!(!output.contains("do-not-expose-secret"));
    assert!(!output.contains("127.0.0.1"));
}

#[tokio::test]
async fn disabled_unknown_or_invalid_branches_cannot_receive_new_dispatches() {
    let db = fresh_in_memory_db().await;
    let branch_id = branch(&db.conn).await;
    service::set_enabled(&db.conn, &branch_id, false, 1)
        .await
        .unwrap();
    assert!(dispatch::create(
        &db.conn,
        input(&branch_id, &uuid::Uuid::new_v4().to_string())
    )
    .await
    .is_err());
    assert!(dispatch::create(
        &db.conn,
        input(
            &uuid::Uuid::new_v4().to_string(),
            &uuid::Uuid::new_v4().to_string()
        )
    )
    .await
    .is_err());
    assert!(dispatch::create(
        &db.conn,
        input("invalid", &uuid::Uuid::new_v4().to_string())
    )
    .await
    .is_err());
    assert!(dispatch::list(&db.conn).await.unwrap().is_empty());
}

#[tokio::test]
async fn retry_after_branch_disable_recovers_existing_receipt_without_new_work() {
    let db = fresh_in_memory_db().await;
    let branch_id = branch(&db.conn).await;
    let submission = uuid::Uuid::new_v4().to_string();
    let first = dispatch::create(&db.conn, input(&branch_id, &submission))
        .await
        .unwrap();
    service::set_enabled(&db.conn, &branch_id, false, 1)
        .await
        .unwrap();
    let retry = dispatch::create(&db.conn, input(&branch_id, &submission))
        .await
        .unwrap();
    assert_eq!(first.dispatch_id, retry.dispatch_id);
    assert_eq!(dispatch::list(&db.conn).await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_duplicate_submissions_create_only_one_intent() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::test_helpers::fresh_disk_db(dir.path()).await;
    let branch_id = branch(&db.conn).await;
    let submission = uuid::Uuid::new_v4().to_string();
    let (left, right) = tokio::join!(
        dispatch::create(&db.conn, input(&branch_id, &submission)),
        dispatch::create(&db.conn, input(&branch_id, &submission)),
    );
    assert_eq!(left.unwrap().dispatch_id, right.unwrap().dispatch_id);
    assert_eq!(dispatch::list(&db.conn).await.unwrap().len(), 1);
}

#[tokio::test]
async fn downgrade_refuses_to_discard_dispatch_intentions() {
    use sea_orm_migration::{MigrationTrait, SchemaManager};
    let db = fresh_in_memory_db().await;
    let branch_id = branch(&db.conn).await;
    dispatch::create(
        &db.conn,
        input(&branch_id, &uuid::Uuid::new_v4().to_string()),
    )
    .await
    .unwrap();
    let manager = SchemaManager::new(&db.conn);
    assert!(super::dispatch_migration::Migration
        .down(&manager)
        .await
        .is_err());
    assert_eq!(dispatch::list(&db.conn).await.unwrap().len(), 1);
}
