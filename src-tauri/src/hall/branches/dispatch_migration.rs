use sea_orm_migration::prelude::*;

pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000001_branch_dispatch"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE hall_branch_dispatch (\
                dispatch_id TEXT PRIMARY KEY NOT NULL,\
                submission_id TEXT NOT NULL UNIQUE,\
                request_hash TEXT NOT NULL,\
                branch_id TEXT NOT NULL REFERENCES hall_branch(id) ON DELETE RESTRICT,\
                member_ref TEXT NOT NULL, title TEXT NOT NULL, description TEXT NOT NULL,\
                status TEXT NOT NULL CHECK(status IN ('pending_delivery','awaiting_acceptance',\
                'rejected','accepted','running','awaiting_permission','succeeded',\
                'failed','interrupted','outcome_unknown')),\
                revision INTEGER NOT NULL CHECK(revision > 0),\
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL);\
             CREATE INDEX hall_branch_dispatch_branch_created \
                ON hall_branch_dispatch(branch_id,created_at,dispatch_id)",
            )
            .await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager
            .get_connection()
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_branch_dispatch LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Branch dispatch records cannot be discarded by downgrade".into(),
            ));
        }
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE hall_branch_dispatch")
            .await?;
        Ok(())
    }
}
