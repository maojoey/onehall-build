//! A schema epoch for backups after the Codeg 0.40 import. The upstream
//! migration sorts before our released member migrations, so it alone would
//! leave the old snapshot migration last in the backup manifest. A 0.39
//! reader would accept that manifest before failing to start the restored DB.
use sea_orm_migration::prelude::*;

pub struct Migration;
impl sea_orm_migration::MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261025_000001_upstream040"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
