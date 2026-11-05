use sea_orm_migration::prelude::*;
pub struct Migration;
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261101_000104_exchange_core"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE hall_exchange_submission(submission_id TEXT PRIMARY KEY NOT NULL,owner_id TEXT NOT NULL)",
            "CREATE TABLE hall_exchange_blob(blob_id TEXT PRIMARY KEY NOT NULL,size BIGINT NOT NULL CHECK(size BETWEEN 0 AND 536870912),sha256 TEXT NOT NULL)",
            "CREATE TABLE hall_exchange_input(object_id TEXT PRIMARY KEY NOT NULL,submission_id TEXT NOT NULL REFERENCES hall_exchange_submission(submission_id) ON DELETE RESTRICT,display_name TEXT NOT NULL,name_key TEXT NOT NULL,size BIGINT NOT NULL CHECK(size BETWEEN 0 AND 536870912),sha256 TEXT NOT NULL,transfer_id TEXT NOT NULL UNIQUE,blob_id TEXT REFERENCES hall_exchange_blob(blob_id) ON DELETE RESTRICT,confirmed BOOLEAN NOT NULL DEFAULT 0,UNIQUE(submission_id,name_key))",
            "CREATE TABLE hall_exchange_manifest(manifest_id TEXT PRIMARY KEY NOT NULL,submission_id TEXT NOT NULL REFERENCES hall_exchange_submission(submission_id) ON DELETE RESTRICT,manifest_json TEXT NOT NULL,digest TEXT NOT NULL,selection_json TEXT NOT NULL,UNIQUE(submission_id,selection_json))",
            "CREATE TABLE hall_exchange_attachment(dispatch_id TEXT PRIMARY KEY NOT NULL,submission_id TEXT NOT NULL UNIQUE REFERENCES hall_exchange_submission(submission_id) ON DELETE RESTRICT,issuer_id TEXT NOT NULL,branch_id TEXT NOT NULL,manifest_id TEXT NOT NULL UNIQUE REFERENCES hall_exchange_manifest(manifest_id) ON DELETE RESTRICT,task_id TEXT,current_run_id TEXT,generation BIGINT NOT NULL DEFAULT 0 CHECK(generation>=0),revoked BOOLEAN NOT NULL DEFAULT 0)",
            "CREATE TABLE hall_exchange_attempt(run_id TEXT PRIMARY KEY NOT NULL,attempt_id TEXT NOT NULL UNIQUE,task_id TEXT NOT NULL,dispatch_id TEXT NOT NULL REFERENCES hall_exchange_attachment(dispatch_id) ON DELETE RESTRICT,generation BIGINT NOT NULL CHECK(generation>0),nonce TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('preparing','ready')),revoked BOOLEAN NOT NULL DEFAULT 0,UNIQUE(dispatch_id,generation))",
            "CREATE TABLE hall_exchange_slot(run_id TEXT NOT NULL REFERENCES hall_exchange_attempt(run_id) ON DELETE RESTRICT,slot_id TEXT NOT NULL,display_name TEXT NOT NULL,nonce TEXT, reserved_size BIGINT NOT NULL DEFAULT 0 CHECK(reserved_size BETWEEN 0 AND 536870912),blob_id TEXT REFERENCES hall_exchange_blob(blob_id) ON DELETE RESTRICT,object_id TEXT UNIQUE,artifact_json TEXT,PRIMARY KEY(run_id,slot_id))",
            "CREATE TABLE hall_exchange_collection(object_id TEXT PRIMARY KEY NOT NULL,dispatch_id TEXT NOT NULL REFERENCES hall_exchange_attachment(dispatch_id) ON DELETE RESTRICT,attempt_id TEXT NOT NULL,run_id TEXT NOT NULL,slot_id TEXT NOT NULL,blob_id TEXT NOT NULL REFERENCES hall_exchange_blob(blob_id) ON DELETE RESTRICT,artifact_json TEXT NOT NULL,collected_at TEXT NOT NULL,UNIQUE(dispatch_id,attempt_id,slot_id))",
        ] {m.get_connection().execute_unprepared(sql).await?;}
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        if m.get_connection().query_one(sea_orm::Statement::from_string(sea_orm::DbBackend::Sqlite,
            "SELECT 1 FROM hall_exchange_submission UNION ALL SELECT 1 FROM hall_exchange_blob UNION ALL SELECT 1 FROM hall_exchange_manifest UNION ALL SELECT 1 FROM hall_exchange_attachment LIMIT 1")).await?.is_some() {return Err(DbErr::Custom("Exchange ownership and bytes cannot be discarded".into()));}
        for table in [
            "hall_exchange_collection",
            "hall_exchange_slot",
            "hall_exchange_attempt",
            "hall_exchange_attachment",
            "hall_exchange_manifest",
            "hall_exchange_input",
            "hall_exchange_blob",
            "hall_exchange_submission",
        ] {
            m.get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
