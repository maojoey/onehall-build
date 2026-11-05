use super::inbox::{self, Decision, Envelope};
use crate::db::test_helpers::{fresh_disk_db, fresh_in_memory_db};

fn envelope(dispatch_id: &str) -> Envelope {
    Envelope {
        dispatch_id: dispatch_id.into(),
        member_ref: "windows-codex".into(),
        title: "Prepare a report".into(),
        description: "Use the local project.".into(),
    }
}

#[tokio::test]
async fn duplicate_delivery_is_idempotent_and_never_launches() {
    let db = fresh_in_memory_db().await;
    let issuer = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let a = inbox::receive(&db.conn, &issuer, envelope(&id))
        .await
        .unwrap();
    let b = inbox::receive(&db.conn, &issuer, envelope(&id))
        .await
        .unwrap();
    assert_eq!(a.dispatch_id, b.dispatch_id);
    assert_eq!(a.status, "awaiting_confirmation");
    assert_eq!(a.revision, 1);
    assert!(!a.launch_allowed);
    let mut conflict = envelope(&id);
    conflict.description = "Different task".into();
    assert!(inbox::receive(&db.conn, &issuer, conflict).await.is_err());
    let detail = inbox::get(&db.conn, &issuer, &id).await.unwrap();
    assert_eq!(detail.description, "Use the local project.");
    assert!(!detail.receipt.launch_allowed);
}

#[tokio::test]
async fn only_matching_issuer_can_see_and_locally_decide_a_delivery() {
    let db = fresh_in_memory_db().await;
    let issuer = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    inbox::receive(&db.conn, &issuer, envelope(&id))
        .await
        .unwrap();
    assert!(inbox::get(&db.conn, &other, &id).await.is_err());
    assert!(inbox::decide(&db.conn, &other, &id, 1, Decision::Accept)
        .await
        .is_err());
    let accepted = inbox::decide(&db.conn, &issuer, &id, 1, Decision::Accept)
        .await
        .unwrap();
    assert_eq!(accepted.status, "accepted");
    assert_eq!(accepted.revision, 2);
    assert!(!accepted.launch_allowed);
    let repeated = inbox::decide(&db.conn, &issuer, &id, 1, Decision::Accept)
        .await
        .unwrap();
    assert_eq!(repeated.revision, 2);
    assert!(inbox::decide(&db.conn, &issuer, &id, 1, Decision::Reject)
        .await
        .is_err());
    assert!(inbox::decide(&db.conn, &issuer, &id, 2, Decision::Reject)
        .await
        .is_err());
}

#[tokio::test]
async fn concurrent_local_decisions_have_only_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let issuer = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    inbox::receive(&db.conn, &issuer, envelope(&id))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        inbox::decide(&db.conn, &issuer, &id, 1, Decision::Accept),
        inbox::decide(&db.conn, &issuer, &id, 1, Decision::Reject),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let result = inbox::get(&db.conn, &issuer, &id).await.unwrap();
    assert_eq!(result.receipt.revision, 2);
    assert!(!result.receipt.launch_allowed);
}

#[test]
fn untrusted_envelope_cannot_carry_execution_options() {
    let id = uuid::Uuid::new_v4().to_string();
    for field in ["command", "cwd", "env", "permission", "issuerId"] {
        let mut v = serde_json::to_value(envelope(&id)).unwrap();
        v[field] = "untrusted".into();
        assert!(
            serde_json::from_value::<Envelope>(v).is_err(),
            "accepted {field}"
        );
    }
}

#[tokio::test]
async fn downgrade_refuses_to_discard_received_tasks() {
    use sea_orm_migration::{MigrationTrait, SchemaManager};
    let db = fresh_in_memory_db().await;
    let issuer = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    inbox::receive(&db.conn, &issuer, envelope(&id))
        .await
        .unwrap();
    let manager = SchemaManager::new(&db.conn);
    assert!(super::inbox_migration::Migration
        .down(&manager)
        .await
        .is_err());
    assert_eq!(
        inbox::get(&db.conn, &issuer, &id)
            .await
            .unwrap()
            .receipt
            .status,
        "awaiting_confirmation"
    );
}

#[tokio::test]
async fn concurrent_identical_deliveries_preserve_one_record() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let issuer = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        inbox::receive(&db.conn, &issuer, envelope(&id)),
        inbox::receive(&db.conn, &issuer, envelope(&id)),
    );
    assert_eq!(a.unwrap().dispatch_id, b.unwrap().dispatch_id);
    let count = db
        .conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM hall_branch_inbox",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn received_intent_survives_restart_without_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let issuer = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    inbox::receive(&db.conn, &issuer, envelope(&id))
        .await
        .unwrap();
    db.conn.close().await.unwrap();
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("source.db").display());
    let reopened = sea_orm::Database::connect(url).await.unwrap();
    let item = inbox::get(&reopened, &issuer, &id).await.unwrap();
    assert_eq!(item.receipt.status, "awaiting_confirmation");
    assert!(!item.receipt.launch_allowed);
}

#[tokio::test]
async fn revoked_verified_issuer_cannot_submit_or_read_remote_intake() {
    let db = fresh_in_memory_db().await;
    let registration = super::trust::create_local(&db.conn, "Headquarters")
        .await
        .unwrap();
    let verified =
        super::trust::authenticate(&db.conn, &registration.issuer_id, &registration.secret)
            .await
            .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    inbox::receive_verified(&db.conn, &verified, envelope(&id))
        .await
        .unwrap();
    assert_eq!(
        inbox::get_verified(&db.conn, &verified, &id)
            .await
            .unwrap()
            .receipt
            .status,
        "awaiting_confirmation"
    );
    let other = super::trust::create_local(&db.conn, "Another headquarters")
        .await
        .unwrap();
    let other_verified = super::trust::authenticate(&db.conn, &other.issuer_id, &other.secret)
        .await
        .unwrap();
    assert!(inbox::get_verified(&db.conn, &other_verified, &id)
        .await
        .is_err());
    super::trust::revoke_local(&db.conn, &registration.issuer_id, 1)
        .await
        .unwrap();
    assert!(inbox::receive_verified(&db.conn, &verified, envelope(&id))
        .await
        .is_err());
    assert!(inbox::get_verified(&db.conn, &verified, &id).await.is_err());
    assert!(inbox::get(&db.conn, &registration.issuer_id, &id)
        .await
        .is_ok());
}
