use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261025_000006_team_exit_evidence"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE hall_task_exit_check(attempt_id TEXT PRIMARY KEY \
        NOT NULL REFERENCES hall_task_attempt(attempt_id) ON DELETE \
        RESTRICT,scope_hash TEXT NOT NULL,lease_id TEXT NOT NULL,\
        report_json TEXT NOT NULL)",
            )
            .await?;
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        if m.get_connection()
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_task_exit_check LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Exit observations cannot be discarded by a downgrade".into(),
            ));
        }
        m.get_connection()
            .execute_unprepared("DROP TABLE hall_task_exit_check")
            .await?;
        Ok(())
    }
}
