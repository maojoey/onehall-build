use sea_orm_migration::prelude::*;

pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000003_branch_trust"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE hall_branch_trust (\
                issuer_id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL,\
                credential_hash TEXT NOT NULL, enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),\
                revision INTEGER NOT NULL CHECK(revision > 0),\
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
            )
            .await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager
            .get_connection()
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_branch_trust LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Branch trust records cannot be discarded by downgrade".into(),
            ));
        }
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE hall_branch_trust")
            .await?;
        Ok(())
    }
}
