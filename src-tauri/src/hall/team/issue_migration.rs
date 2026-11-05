//! Key team actions that failed during a run (a report the PM never got, a
//! message or proposal the broker refused). A task can finish "succeeded"
//! while one of these is still open; the task detail shows them.
use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261028_000001_team_task_issue"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE hall_task_issue(issue_id TEXT PRIMARY KEY NOT \
        NULL,task_id TEXT NOT NULL REFERENCES hall_task(task_id) ON DELETE \
        RESTRICT,attempt_id TEXT NOT NULL,tool TEXT NOT NULL,detail TEXT NOT \
        NULL,created_at TEXT NOT NULL,resolved_at TEXT,FOREIGN \
        KEY(attempt_id,task_id) REFERENCES hall_task_attempt(attempt_id,\
        task_id) ON DELETE RESTRICT)",
            )
            .await?;
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE hall_task_issue")
            .await?;
        Ok(())
    }
}
