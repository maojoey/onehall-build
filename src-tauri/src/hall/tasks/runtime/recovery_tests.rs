use super::{
    tests::{queued, setup},
    *,
};
use crate::hall::tasks::{get_task, queue::QueueHost};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn orphan_release_survives_new_controller_and_exact_replay_only() {
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "restart-orphan").await;
    f.fail_start.store(true, Ordering::SeqCst);
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    let old = get_task(c.co.database(), id.clone()).await.unwrap();
    let attempt = old.attempts.last().unwrap().attempt_id.clone();
    let rev = old.task.revision;
    let fresh = Controller::closed(
        c.co.database().clone(),
        c.service.clone(),
        c.manager.clone_ref(),
    );
    drop(c);
    let recovered = fresh.recover(id.clone()).await.unwrap();
    assert_eq!(recovered.task.status, "interrupted");
    for (a, r, ack, reason) in [
        ("wrong-attempt", rev, true, "checked"),
        (attempt.as_str(), rev - 1, true, "checked"),
        (attempt.as_str(), rev, false, "checked"),
        (attempt.as_str(), rev, true, "  "),
    ] {
        assert!(fresh
            .resolve_orphan(id.clone(), a.into(), r, ack, reason.into())
            .await
            .is_err());
    }
    let d = fresh
        .resolve_orphan(
            id.clone(),
            attempt.clone(),
            rev,
            true,
            " 本机已核查 ".into(),
        )
        .await
        .unwrap();
    assert_eq!(d.task.status, "interrupted");
    assert!(d.runtimes[0].exit_confirmed_at.is_none());
    assert!(d.attempts[0].finished_at.is_none());
    let json = serde_json::to_value(&d).unwrap();
    assert_eq!(json["resolution"]["kind"], "operator_released");
    assert_eq!(json["resolution"]["reason"], "本机已核查");
    let again = fresh
        .resolve_orphan(id.clone(), attempt.clone(), rev, true, "本机已核查".into())
        .await
        .unwrap();
    assert_eq!(again.task.revision, d.task.revision);
    assert!(fresh
        .resolve_orphan(id.clone(), attempt, rev, true, "different".into())
        .await
        .is_err());
    assert_eq!(f.starts.load(Ordering::SeqCst), 1);
    let host = QueueHost::new(fresh.co.database().clone(), 1).unwrap();
    assert_eq!(host.reconcile_previous_boots().await.unwrap(), 0);
}

#[tokio::test]
async fn not_started_and_verified_abort_release_capacity_but_unknown_does_not() {
    for code in [0, 1, 2] {
        let (dir, c, s, f) = setup(true).await;
        let id = queued(&s, dir.path(), "abort").await;
        f.fail_start.store(true, Ordering::SeqCst);
        f.abort_result.store(code, Ordering::SeqCst);
        assert!(c.dispatch(id.clone(), 2).await.is_err());
        let d = get_task(c.co.database(), id).await.unwrap();
        assert_eq!(
            d.task.status,
            if code == 0 { "interrupted" } else { "failed" }
        );
        f.fail_start.store(false, Ordering::SeqCst);
        let next = queued(&s, dir.path(), "next").await;
        assert_eq!(c.dispatch(next, 2).await.is_ok(), code != 0);
    }
}

#[tokio::test]
async fn two_unknown_failures_can_be_reviewed_and_a_third_member_dispatched() {
    let (dir, c, s, f) = setup(true).await;
    for name in ["first-failure", "second-failure"] {
        let id = queued(&s, dir.path(), name).await;
        f.fail_start.store(true, Ordering::SeqCst);
        assert!(c.dispatch(id.clone(), 2).await.is_err());
        let d = c.recover(id.clone()).await.unwrap();
        c.resolve_orphan(
            id,
            d.attempts[0].attempt_id.clone(),
            d.task.revision,
            true,
            "本机核查，接受退出未证实风险".into(),
        )
        .await
        .unwrap();
    }
    f.fail_start.store(false, Ordering::SeqCst);
    let third = queued(&s, dir.path(), "third").await;
    assert!(c.dispatch(third, 2).await.is_ok());
    assert_eq!(f.starts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn appended_recovery_migration_is_repeatable_and_refuses_data_loss() {
    use sea_orm_migration::{MigrationTrait, MigratorTrait, SchemaManager};
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "migration").await;
    f.fail_start.store(true, Ordering::SeqCst);
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    crate::db::migration::Migrator::up(c.co.database(), None)
        .await
        .unwrap();
    assert!(crate::hall::tasks::runtime::recovery_migration::Migration
        .down(&SchemaManager::new(c.co.database()))
        .await
        .is_err());
    assert_eq!(
        get_task(c.co.database(), id).await.unwrap().task.status,
        "interrupted"
    );
}

#[tokio::test]
async fn running_attempt_reopens_from_disk_and_late_old_events_are_fenced() {
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "running-reboot").await;
    c.dispatch(id.clone(), 2).await.unwrap();
    let old = {
        let mut state = c.state.lock().await;
        state.active.remove(&id).unwrap()
    };
    let connection = sea_orm::Database::connect(format!(
        "sqlite:{}?mode=rw",
        dir.path().join("source.db").display()
    ))
    .await
    .unwrap();
    let old_co = c.co.clone();
    let fresh = Controller::closed(connection, c.service.clone(), c.manager.clone_ref());
    drop(s);
    drop(c);
    let d = fresh.recover(id.clone()).await.unwrap();
    fresh
        .resolve_orphan(
            id.clone(),
            d.attempts[0].attempt_id.clone(),
            d.task.revision,
            true,
            "重启后本机核查，接受退出未证实风险".into(),
        )
        .await
        .unwrap();
    assert!(old_co
        .accept_event(
            &old.claim,
            &old.binding,
            1,
            super::super::RuntimeEvent::Result {
                outcome: super::super::Outcome::Succeeded,
                body: "late".into()
            }
        )
        .await
        .is_err());
    assert_eq!(
        get_task(fresh.co.database(), id).await.unwrap().task.status,
        "interrupted"
    );
    assert_eq!(f.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mismatched_persistent_start_identity_cannot_be_recovered_or_released() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "corrupt-owner").await;
    f.fail_start.store(true, Ordering::SeqCst);
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    let d = get_task(c.co.database(), id.clone()).await.unwrap();
    c.co.database()
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_task_start_recovery SET host_epoch='wrong-owner' WHERE task_id=?",
            vec![id.clone().into()],
        ))
        .await
        .unwrap();
    assert!(c.recover(id.clone()).await.is_err());
    assert!(c
        .resolve_orphan(
            id,
            d.attempts[0].attempt_id.clone(),
            d.task.revision,
            true,
            "已核查".into()
        )
        .await
        .is_err());
}

#[tokio::test]
async fn upgrade_preserves_legacy_claim_and_recovers_it_without_inventing_a_run() {
    use sea_orm_migration::MigratorTrait;
    let (dir, c, s, _f) = setup(true).await;
    crate::db::migration::Migrator::down(c.co.database(), Some(1))
        .await
        .unwrap();
    let id = queued(&s, dir.path(), "legacy-upgrade").await;
    c.co.host
        .claim_runtime(true, "fixture")
        .await
        .unwrap()
        .unwrap();
    let before =
        serde_json::to_value(get_task(c.co.database(), id.clone()).await.unwrap()).unwrap();
    crate::db::migration::Migrator::up(c.co.database(), None)
        .await
        .unwrap();
    let after = serde_json::to_value(get_task(c.co.database(), id.clone()).await.unwrap()).unwrap();
    assert_eq!(before, after);
    let reboot = QueueHost::new(c.co.database().clone(), 1).unwrap();
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 1);
    let d = c.recover(id.clone()).await.unwrap();
    let released = c
        .resolve_orphan(
            id,
            d.attempts[0].attempt_id.clone(),
            d.task.revision,
            true,
            "旧库记录已在本机核查".into(),
        )
        .await
        .unwrap();
    assert_eq!(released.task.status, "interrupted");
    assert!(released.runtimes[0].exit_confirmed_at.is_none());
}

#[tokio::test]
async fn durable_reservation_remains_recoverable_when_intent_and_failure_writes_fail() {
    use sea_orm::ConnectionTrait;
    let (dir, c, s, f) = setup(true).await;
    let id = queued(&s, dir.path(), "intent-rollback").await;
    c.co.database().execute_unprepared("CREATE TRIGGER fixture_intent_failure BEFORE INSERT ON hall_task_event WHEN NEW.action IN ('dispatch_intent','start_failed') BEGIN SELECT RAISE(FAIL,'fixture intent rollback'); END").await.unwrap();
    assert!(c.dispatch(id.clone(), 2).await.is_err());
    assert_eq!(f.starts.load(Ordering::SeqCst), 0);
    let reboot = QueueHost::new(c.co.database().clone(), 1).unwrap();
    assert_eq!(reboot.reconcile_previous_boots().await.unwrap(), 1);
    let d = c.recover(id.clone()).await.unwrap();
    let resolved = c
        .resolve_orphan(
            id,
            d.attempts[0].attempt_id.clone(),
            d.task.revision,
            true,
            "已核查持久预约记录，接受未证实退出风险".into(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.task.status, "interrupted");
    assert!(resolved.runtimes[0].exit_confirmed_at.is_none());
}
