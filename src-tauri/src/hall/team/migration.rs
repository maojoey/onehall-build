use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261025_000005_team_mailbox"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE hall_role_binding(role TEXT PRIMARY KEY NOT NULL \
                CHECK(role='pm'),member_id TEXT REFERENCES \
                hall_member(member_id) ON DELETE RESTRICT,revision BIGINT NOT \
                NULL CHECK(revision>0))",
            "INSERT INTO hall_role_binding(role,member_id,revision) VALUES('pm',NULL,1)",
            "CREATE TABLE hall_message(message_id TEXT PRIMARY KEY NOT NULL,\
                submission_id TEXT NOT NULL UNIQUE,request_hash TEXT NOT NULL,\
                sender_member_id TEXT REFERENCES hall_member(member_id) ON \
                DELETE RESTRICT,recipient_member_id TEXT NOT NULL REFERENCES \
                hall_member(member_id) ON DELETE RESTRICT,task_id TEXT \
                REFERENCES hall_task(task_id) ON DELETE RESTRICT,attempt_id \
                TEXT,kind TEXT NOT NULL CHECK(kind IN ('message','report')),\
                body TEXT NOT NULL,created_at TEXT NOT NULL,acknowledged_at \
                TEXT,FOREIGN KEY(attempt_id,task_id) REFERENCES \
                hall_task_attempt(attempt_id,task_id) ON DELETE RESTRICT)",
            "CREATE INDEX hall_message_inbox ON \
                hall_message(recipient_member_id,created_at,message_id)",
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
                "SELECT 1 FROM hall_message UNION ALL SELECT 1 FROM \
            hall_role_binding WHERE member_id IS NOT NULL LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "Team mailbox and role bindings cannot be discarded by a \
            downgrade"
                    .into(),
            ));
        }
        db.execute_unprepared("DROP TABLE hall_message").await?;
        db.execute_unprepared("DROP TABLE hall_role_binding")
            .await?;
        Ok(())
    }
}
