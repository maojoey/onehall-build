use sea_orm_migration::prelude::*;

pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000101_start_recovery"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(
            "CREATE TABLE hall_task_start_recovery(\
             attempt_id TEXT PRIMARY KEY NOT NULL REFERENCES hall_task_attempt(attempt_id) ON DELETE RESTRICT,\
             task_id TEXT NOT NULL REFERENCES hall_task(task_id) ON DELETE RESTRICT,\
             claim_id TEXT NOT NULL,host_epoch TEXT NOT NULL,run_id TEXT UNIQUE,\
             phase TEXT NOT NULL CHECK(phase IN ('preparing','starting','binding','activating','active','aborted')),\
             failure TEXT CHECK(failure IN ('not_started','exit_verified','exit_unverified')),\
             failure_code TEXT, writes_revoked BOOLEAN NOT NULL DEFAULT 0,\
             resolved_revision BIGINT, resolution_reason TEXT, resolution_actor TEXT, resolved_at TEXT,\
             updated_at TEXT NOT NULL)",
        ).await?;
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        if m.get_connection()
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_task_start_recovery LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Start recovery history cannot be discarded".into(),
            ));
        }
        m.get_connection()
            .execute_unprepared("DROP TABLE hall_task_start_recovery")
            .await?;
        Ok(())
    }
}
