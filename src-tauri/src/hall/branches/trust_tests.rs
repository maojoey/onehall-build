use super::trust;
use crate::db::test_helpers::fresh_in_memory_db;

#[tokio::test]
async fn locally_created_credential_authenticates_only_its_own_issuer() {
    let db = fresh_in_memory_db().await;
    let a = trust::create_local(&db.conn, "Headquarters A")
        .await
        .unwrap();
    let b = trust::create_local(&db.conn, "Headquarters B")
        .await
        .unwrap();
    assert_ne!(a.issuer_id, b.issuer_id);
    assert!(trust::authenticate(&db.conn, &a.issuer_id, &a.secret)
        .await
        .is_ok());
    assert!(trust::authenticate(&db.conn, &a.issuer_id, &b.secret)
        .await
        .is_err());
    assert!(trust::authenticate(&db.conn, &a.issuer_id, "wrong")
        .await
        .is_err());
    assert!(
        trust::authenticate(&db.conn, &uuid::Uuid::new_v4().to_string(), &a.secret)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn revocation_blocks_old_credential_without_erasing_history() {
    let db = fresh_in_memory_db().await;
    let a = trust::create_local(&db.conn, "Headquarters").await.unwrap();
    trust::revoke_local(&db.conn, &a.issuer_id, 1)
        .await
        .unwrap();
    assert!(trust::authenticate(&db.conn, &a.issuer_id, &a.secret)
        .await
        .is_err());
    assert!(trust::revoke_local(&db.conn, &a.issuer_id, 1)
        .await
        .is_err());
    let rows = trust::list_local(&db.conn).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].enabled);
    assert!(!serde_json::to_string(&rows).unwrap().contains(&a.secret));
}

#[tokio::test]
async fn trust_store_never_persists_raw_credential_and_protects_downgrade() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    use sea_orm_migration::{MigrationTrait, SchemaManager};
    let db = fresh_in_memory_db().await;
    let a = trust::create_local(&db.conn, "Headquarters").await.unwrap();
    let row = db
        .conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT credential_hash FROM hall_branch_trust",
        ))
        .await
        .unwrap()
        .unwrap();
    let stored = row.try_get::<String>("", "credential_hash").unwrap();
    assert!(!stored.contains(&a.secret));
    assert!(super::trust_migration::Migration
        .down(&SchemaManager::new(&db.conn))
        .await
        .is_err());
}
