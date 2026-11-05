use super::{binding, inbox, trust};
use crate::{
    db::test_helpers::fresh_disk_db,
    hall::{members_tests::server, tasks_tests::fixture},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

#[tokio::test]
async fn local_acceptance_creates_exactly_one_draft_under_concurrent_retries() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let api = server(db, dir.path());
    let (member, snapshot) = fixture(&api, dir.path(), "branch-member").await;
    crate::hall::members::hall_set_member_enabled_core(
        &conn,
        member["memberId"].as_str().unwrap().into(),
        true,
    )
    .await
    .unwrap();
    let a = trust::create_local(&conn, "HQ").await.unwrap();
    let issuer = trust::authenticate(&conn, &a.issuer_id, &a.secret)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    inbox::receive_verified(
        &conn,
        &issuer,
        inbox::Envelope {
            dispatch_id: id.clone(),
            member_ref: "chosen-locally".into(),
            title: "Prepare result".into(),
            description: "Task body".into(),
        },
    )
    .await
    .unwrap();
    let request = binding::Request {
        issuer_id: a.issuer_id.clone(),
        dispatch_id: id.clone(),
        expected_revision: 1,
        member_id: member["memberId"].as_str().unwrap().into(),
        snapshot_id: snapshot["snapshotId"].as_str().unwrap().into(),
        project_id: None,
    };
    let (x, y) = tokio::join!(
        binding::accept_local(&conn, request.clone()),
        binding::accept_local(&conn, request.clone())
    );
    let x = x.unwrap();
    assert_eq!(x.task_id, y.unwrap().task_id);
    let task = crate::hall::tasks::get_task(&conn, x.task_id)
        .await
        .unwrap();
    assert_eq!(task.task.status, "draft");
    assert!(task.attempts.is_empty());
    assert_eq!(task.description, "Task body");
    assert_eq!(
        inbox::get(&conn, &a.issuer_id, &id)
            .await
            .unwrap()
            .receipt
            .status,
        "accepted"
    );
    let mut changed = request;
    changed.member_id = uuid::Uuid::new_v4().to_string();
    assert!(binding::accept_local(&conn, changed).await.is_err());
    let count = conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM hall_task",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn invalid_local_selection_rolls_back_acceptance_and_task_creation() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let a = trust::create_local(&db.conn, "HQ").await.unwrap();
    let issuer = trust::authenticate(&db.conn, &a.issuer_id, &a.secret)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    inbox::receive_verified(
        &db.conn,
        &issuer,
        inbox::Envelope {
            dispatch_id: id.clone(),
            member_ref: "worker".into(),
            title: "test".into(),
            description: "body".into(),
        },
    )
    .await
    .unwrap();
    assert!(binding::accept_local(
        &db.conn,
        binding::Request {
            issuer_id: a.issuer_id.clone(),
            dispatch_id: id.clone(),
            expected_revision: 1,
            member_id: uuid::Uuid::new_v4().to_string(),
            snapshot_id: uuid::Uuid::new_v4().to_string(),
            project_id: None
        }
    )
    .await
    .is_err());
    assert_eq!(
        inbox::get(&db.conn, &a.issuer_id, &id)
            .await
            .unwrap()
            .receipt
            .status,
        "awaiting_confirmation"
    );
    let count = db
        .conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM hall_task",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn local_binding_requires_enabled_current_selection_and_retries_survive_disable() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_disk_db(dir.path()).await;
    let conn = db.conn.clone();
    let api = server(db, dir.path());
    let (member, snapshot) = fixture(&api, dir.path(), "selection-gate-member").await;
    let member_id = member["memberId"].as_str().unwrap().to_owned();
    let registration = trust::create_local(&conn, "HQ").await.unwrap();
    let issuer = trust::authenticate(&conn, &registration.issuer_id, &registration.secret)
        .await
        .unwrap();
    let dispatch = uuid::Uuid::new_v4().to_string();
    inbox::receive_verified(
        &conn,
        &issuer,
        inbox::Envelope {
            dispatch_id: dispatch.clone(),
            member_ref: "worker".into(),
            title: "Local selection".into(),
            description: "Body".into(),
        },
    )
    .await
    .unwrap();
    let selection = binding::Request {
        issuer_id: registration.issuer_id.clone(),
        dispatch_id: dispatch.clone(),
        expected_revision: 1,
        member_id: member_id.clone(),
        snapshot_id: snapshot["snapshotId"].as_str().unwrap().into(),
        project_id: None,
    };
    assert!(binding::accept_local(&conn, selection.clone())
        .await
        .is_err());
    assert_eq!(
        inbox::get(&conn, &registration.issuer_id, &dispatch)
            .await
            .unwrap()
            .receipt
            .status,
        "awaiting_confirmation"
    );
    crate::hall::members::hall_set_member_enabled_core(&conn, member_id.clone(), true)
        .await
        .unwrap();
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_member SET revision=revision+1 WHERE member_id=?",
        [member_id.clone().into()],
    ))
    .await
    .unwrap();
    assert!(binding::accept_local(&conn, selection.clone())
        .await
        .is_err());
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE hall_member SET revision=revision-1 WHERE member_id=?",
        [member_id.clone().into()],
    ))
    .await
    .unwrap();
    let accepted = binding::accept_local(&conn, selection.clone())
        .await
        .unwrap();
    crate::hall::members::hall_set_member_enabled_core(&conn, member_id, false)
        .await
        .unwrap();
    assert_eq!(
        binding::accept_local(&conn, selection)
            .await
            .unwrap()
            .task_id,
        accepted.task_id
    );
    assert_eq!(
        crate::hall::tasks::get_task(&conn, accepted.task_id)
            .await
            .unwrap()
            .task
            .status,
        "draft"
    );
    let count = conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM hall_task",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(count, 1);
}
