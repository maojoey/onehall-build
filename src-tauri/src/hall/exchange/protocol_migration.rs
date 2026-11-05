use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000105_exchange_protocol"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // Additive only: older rows keep NULL and retain v1 meaning.
        for sql in [
            "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_attempt_id TEXT",
            "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_run_id TEXT",
            "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_attempt_number BIGINT",
            "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_exit_verified BOOLEAN",
            "ALTER TABLE hall_branch_dispatch ADD COLUMN artifact_sync_error TEXT",
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
                "SELECT 1 FROM hall_branch_dispatch WHERE remote_attempt_id IS NOT NULL OR remote_run_id IS NOT NULL OR remote_attempt_number IS NOT NULL OR remote_exit_verified IS NOT NULL OR artifact_sync_error IS NOT NULL LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Exchange receipt evidence cannot be discarded by downgrade".into(),
            ));
        }
        for column in [
            "remote_attempt_id",
            "remote_run_id",
            "remote_attempt_number",
            "remote_exit_verified",
            "artifact_sync_error",
        ] {
            db.execute_unprepared(&format!(
                "ALTER TABLE hall_branch_dispatch DROP COLUMN {column}"
            ))
            .await?;
        }
        Ok(())
    }
}
