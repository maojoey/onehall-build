use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261029_000001_branches"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(
            "CREATE TABLE hall_branch (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, \
            connection_id INTEGER NOT NULL UNIQUE REFERENCES remote_workspace_connection(id) ON DELETE RESTRICT, \
            enabled INTEGER NOT NULL DEFAULT 1, revision INTEGER NOT NULL DEFAULT 1, \
            status TEXT NOT NULL DEFAULT 'unknown', checked_at TEXT); \
            CREATE TABLE hall_branch_desktop (headquarters_id INTEGER NOT NULL REFERENCES remote_workspace_connection(id) ON DELETE CASCADE, \
            branch_id TEXT NOT NULL, connection_id INTEGER NOT NULL UNIQUE REFERENCES remote_workspace_connection(id) ON DELETE CASCADE, \
            PRIMARY KEY(headquarters_id, branch_id))"
        ).await?;
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE hall_branch_desktop; DROP TABLE hall_branch")
            .await?;
        Ok(())
    }
}
