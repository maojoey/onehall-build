use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261025_000007_team_launch_review"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE hall_task_review(attempt_id TEXT PRIMARY KEY NOT \
     NULL REFERENCES hall_task_attempt(attempt_id) ON DELETE \
     RESTRICT,submission_id TEXT NOT NULL UNIQUE,queue_revision \
     BIGINT NOT NULL,plan_hash TEXT NOT NULL,plan_json TEXT NOT NULL,\
     approved_at TEXT NOT NULL)",
            )
            .await?;
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        if m.get_connection()
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_task_review LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Launch reviews cannot be discarded by downgrade".into(),
            ));
        }
        m.get_connection()
            .execute_unprepared("DROP TABLE hall_task_review")
            .await?;
        Ok(())
    }
}
