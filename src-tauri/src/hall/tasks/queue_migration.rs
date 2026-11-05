use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261025_000003_team_queue"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE hall_task_dispatch (attempt_id TEXT PRIMARY KEY \
                NOT NULL REFERENCES hall_task_attempt(attempt_id) ON DELETE \
                RESTRICT, claim_id TEXT NOT NULL UNIQUE, host_epoch TEXT NOT \
                NULL, review_required BOOLEAN NOT NULL DEFAULT 0, created_at \
                TEXT NOT NULL)",
            "CREATE UNIQUE INDEX hall_attempt_one_active_member ON \
                hall_task_attempt(member_id) WHERE status IN ('starting',\
                'running','awaiting_permission','cancel_requested')",
            "CREATE UNIQUE INDEX hall_attempt_task_identity ON \
                hall_task_attempt(attempt_id,task_id)",
            "CREATE TABLE hall_task_event (event_id TEXT PRIMARY KEY NOT \
                NULL, task_id TEXT NOT NULL REFERENCES hall_task(task_id) ON \
                DELETE RESTRICT, attempt_id TEXT NOT NULL REFERENCES \
                hall_task_attempt(attempt_id) ON DELETE RESTRICT, task_revision \
                BIGINT NOT NULL CHECK(task_revision>0), action TEXT NOT NULL \
                CHECK(action IN ('task_queued','attempt_claimed',\
                'attempt_interrupted')), created_at TEXT NOT NULL, \
                UNIQUE(task_id,task_revision), UNIQUE(attempt_id,action), \
                FOREIGN KEY(attempt_id,task_id) REFERENCES \
                hall_task_attempt(attempt_id,task_id) ON DELETE RESTRICT)",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "DROP TABLE hall_task_event",
            "DROP INDEX hall_attempt_task_identity",
            "DROP INDEX hall_attempt_one_active_member",
            "DROP TABLE hall_task_dispatch",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
