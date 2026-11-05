use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261025_000004_team_runtime"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE hall_task_runtime(attempt_id TEXT PRIMARY KEY NOT \
                NULL REFERENCES hall_task_dispatch(attempt_id) ON DELETE \
                RESTRICT, claim_id TEXT NOT NULL, intent_id TEXT NOT NULL \
                UNIQUE, intent_at TEXT NOT NULL, stage TEXT NOT NULL \
                CHECK(stage IN ('prepared','dispatching','running',\
                'waiting_input','finishing','cancel_requested','rejected',\
                'terminal','needs_review')), runtime_kind TEXT NOT NULL, run_id \
                TEXT UNIQUE, connection_id TEXT UNIQUE, conversation_id \
                INTEGER, external_session_id TEXT, last_sequence BIGINT NOT \
                NULL DEFAULT 0, last_digest TEXT, cancel_requested_at TEXT, \
                cancel_ack_at TEXT, exit_confirmed_at TEXT)",
            "CREATE TABLE hall_task_wait(attempt_id TEXT NOT NULL REFERENCES \
                hall_task_attempt(attempt_id), card_key TEXT NOT NULL, waiting \
                BOOLEAN NOT NULL, PRIMARY KEY(attempt_id,card_key))",
            "CREATE TABLE hall_task_result(result_id TEXT PRIMARY KEY NOT \
                NULL, attempt_id TEXT NOT NULL UNIQUE REFERENCES \
                hall_task_attempt(attempt_id) ON DELETE RESTRICT, outcome TEXT \
                NOT NULL CHECK(outcome IN ('succeeded','failed','cancelled',\
                'rejected')), body TEXT NOT NULL, created_at TEXT NOT NULL)",
            "CREATE TABLE hall_task_event_v2(event_id TEXT PRIMARY KEY NOT \
                NULL, task_id TEXT NOT NULL REFERENCES hall_task(task_id) ON \
                DELETE RESTRICT, attempt_id TEXT NOT NULL, task_revision BIGINT \
                NOT NULL CHECK(task_revision>0), action TEXT NOT NULL, \
                source_key TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, \
                UNIQUE(task_id,task_revision), UNIQUE(attempt_id,source_key), \
                FOREIGN KEY(attempt_id,task_id) REFERENCES \
                hall_task_attempt(attempt_id,task_id) ON DELETE RESTRICT)",
            "INSERT INTO hall_task_event_v2(event_id,task_id,attempt_id,\
                task_revision,action,source_key,created_at) SELECT event_id,\
                task_id,attempt_id,task_revision,action,'host:'||action,\
                created_at FROM hall_task_event",
            "DROP TABLE hall_task_event",
            "ALTER TABLE hall_task_event_v2 RENAME TO hall_task_event",
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        if db
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_task_runtime UNION ALL SELECT 1 FROM \
            hall_task_result UNION ALL SELECT 1 FROM hall_task_wait UNION \
            ALL SELECT 1 FROM hall_task_event WHERE action NOT IN \
            ('task_queued','attempt_claimed','attempt_interrupted') LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Runtime history cannot be discarded by a schema downgrade".into(),
            ));
        }
        // Empty-runtime rollback preserves every schema-61 event and its ID.
        for sql in [
            "CREATE TABLE hall_task_event_legacy(event_id TEXT PRIMARY KEY \
                NOT NULL,task_id TEXT NOT NULL REFERENCES hall_task(task_id) ON \
                DELETE RESTRICT,attempt_id TEXT NOT NULL REFERENCES \
                hall_task_attempt(attempt_id) ON DELETE RESTRICT,task_revision \
                BIGINT NOT NULL CHECK(task_revision>0),action TEXT NOT NULL \
                CHECK(action IN ('task_queued','attempt_claimed',\
                'attempt_interrupted')),created_at TEXT NOT NULL,UNIQUE(task_id,\
                task_revision),UNIQUE(attempt_id,action),FOREIGN KEY(attempt_id,\
                task_id) REFERENCES hall_task_attempt(attempt_id,task_id) ON \
                DELETE RESTRICT)",
            "INSERT INTO hall_task_event_legacy(event_id,task_id,attempt_id,\
                task_revision,action,created_at) SELECT event_id,task_id,\
                attempt_id,task_revision,action,created_at FROM hall_task_event",
            "DROP TABLE hall_task_event",
            "ALTER TABLE hall_task_event_legacy RENAME TO hall_task_event",
            "DROP TABLE hall_task_wait",
            "DROP TABLE hall_task_result",
            "DROP TABLE hall_task_runtime",
        ] {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
