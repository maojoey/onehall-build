use sea_orm_migration::prelude::*;

pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000002_branch_inbox"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(
            "CREATE TABLE hall_branch_inbox (\
                issuer_id TEXT NOT NULL, dispatch_id TEXT NOT NULL,\
                request_hash TEXT NOT NULL, member_ref TEXT NOT NULL,\
                title TEXT NOT NULL, description TEXT NOT NULL,\
                status TEXT NOT NULL CHECK(status IN ('awaiting_confirmation','accepted','rejected')),\
                revision INTEGER NOT NULL CHECK(revision > 0),\
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL,\
                PRIMARY KEY(issuer_id,dispatch_id))"
        ).await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager
            .get_connection()
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_branch_inbox LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Branch inbox records cannot be discarded by downgrade".into(),
            ));
        }
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE hall_branch_inbox")
            .await?;
        Ok(())
    }
}
