use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000005_branch_dispatch_link"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        for q in [
   "CREATE TABLE hall_branch_dispatch_credential(branch_id TEXT PRIMARY KEY NOT NULL REFERENCES hall_branch(id) ON DELETE RESTRICT,issuer_id TEXT NOT NULL,secret TEXT NOT NULL,endpoint TEXT NOT NULL)",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_revision INTEGER NOT NULL DEFAULT 0 CHECK(remote_revision>=0)",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_task_revision INTEGER NOT NULL DEFAULT 0 CHECK(remote_task_revision>=0)",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_task_id TEXT",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN delivery_issuer_id TEXT",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN delivery_endpoint TEXT",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN task_status TEXT",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN result_text TEXT",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN remote_result_outcome TEXT",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN sync_error TEXT",
   "ALTER TABLE hall_branch_dispatch ADD COLUMN creator_member_id TEXT REFERENCES hall_member(member_id) ON DELETE RESTRICT",
  ] {m.get_connection().execute_unprepared(q).await?;}
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        if m.get_connection().query_one(sea_orm::Statement::from_string(sea_orm::DbBackend::Sqlite,"SELECT 1 FROM hall_branch_dispatch_credential UNION ALL SELECT 1 FROM hall_branch_dispatch WHERE remote_revision>0 OR sync_error IS NOT NULL OR delivery_issuer_id IS NOT NULL OR creator_member_id IS NOT NULL LIMIT 1")).await?.is_some(){return Err(DbErr::Custom("Dispatch credentials and delivery evidence cannot be discarded by downgrade".into()));}
        for q in [
            "ALTER TABLE hall_branch_dispatch DROP COLUMN creator_member_id",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN sync_error",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN remote_result_outcome",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN result_text",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN task_status",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN delivery_endpoint",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN delivery_issuer_id",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN remote_task_id",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN remote_task_revision",
            "ALTER TABLE hall_branch_dispatch DROP COLUMN remote_revision",
            "DROP TABLE hall_branch_dispatch_credential",
        ] {
            m.get_connection().execute_unprepared(q).await?;
        }
        Ok(())
    }
}
