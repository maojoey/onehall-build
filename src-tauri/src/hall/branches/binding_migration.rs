use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000004_branch_task_binding"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(
            "CREATE TABLE hall_branch_task_binding (issuer_id TEXT NOT NULL,dispatch_id TEXT NOT NULL,\
             task_id TEXT NOT NULL UNIQUE REFERENCES hall_task(task_id) ON DELETE RESTRICT,\
             request_hash TEXT NOT NULL,PRIMARY KEY(issuer_id,dispatch_id),\
             FOREIGN KEY(issuer_id,dispatch_id) REFERENCES hall_branch_inbox(issuer_id,dispatch_id) ON DELETE RESTRICT)"
        ).await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager
            .get_connection()
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT 1 FROM hall_branch_task_binding LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Branch task bindings cannot be discarded by downgrade".into(),
            ));
        }
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE hall_branch_task_binding")
            .await?;
        Ok(())
    }
}
