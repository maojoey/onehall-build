use sea_orm_migration::prelude::*;

pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261025_000002_team_ledger"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE hall_task (task_id TEXT PRIMARY KEY NOT NULL, \
                submission_id TEXT NOT NULL UNIQUE, request_hash TEXT NOT NULL, \
                member_id TEXT NOT NULL, project_id INTEGER, snapshot_id TEXT \
                NOT NULL REFERENCES hall_execution_snapshot(snapshot_id) ON \
                DELETE RESTRICT, snapshot_version TEXT NOT NULL, \
                snapshot_member_revision BIGINT NOT NULL \
                CHECK(snapshot_member_revision > 0), purpose TEXT NOT NULL \
                CHECK(purpose IN ('work','acceptance')), title TEXT NOT NULL, \
                description TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN \
                ('draft','cancelled','queued','running','awaiting_permission',\
                'succeeded','failed','interrupted')), revision BIGINT NOT NULL \
                CHECK(revision > 0), created_at TEXT NOT NULL, updated_at TEXT \
                NOT NULL, UNIQUE(task_id,member_id,snapshot_id,\
                snapshot_version))",
            "CREATE INDEX hall_task_member_created ON hall_task(member_id,created_at)",
            "CREATE INDEX hall_task_project_created ON hall_task(project_id,created_at)",
            "CREATE TABLE hall_task_attempt (attempt_id TEXT PRIMARY KEY NOT \
                NULL, submission_id TEXT NOT NULL UNIQUE, task_id TEXT NOT \
                NULL, member_id TEXT NOT NULL, snapshot_id TEXT NOT NULL, \
                snapshot_version TEXT NOT NULL, attempt_number BIGINT NOT NULL \
                CHECK(attempt_number > 0), status TEXT NOT NULL CHECK(status IN \
                ('queued','starting','running','awaiting_permission',\
                'cancel_requested','cancelled','succeeded','failed',\
                'interrupted')), revision BIGINT NOT NULL CHECK(revision > 0), \
                conversation_id INTEGER, result_id TEXT, created_at TEXT NOT \
                NULL, updated_at TEXT NOT NULL, started_at TEXT, finished_at \
                TEXT, UNIQUE(task_id,attempt_number), FOREIGN KEY(task_id,\
                member_id,snapshot_id,snapshot_version) REFERENCES \
                hall_task(task_id,member_id,snapshot_id,snapshot_version) ON \
                DELETE RESTRICT)",
            "CREATE TABLE hall_task_history (task_id TEXT NOT NULL \
                REFERENCES hall_task(task_id) ON DELETE RESTRICT, revision \
                BIGINT NOT NULL CHECK(revision > 0), action TEXT NOT NULL \
                CHECK(action IN ('draft_created','draft_updated',\
                'draft_cancelled')), created_at TEXT NOT NULL, PRIMARY \
                KEY(task_id,revision))",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in ["hall_task_history", "hall_task_attempt", "hall_task"] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
