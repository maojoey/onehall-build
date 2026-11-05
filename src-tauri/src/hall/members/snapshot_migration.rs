use sea_orm_migration::prelude::*;

pub struct Migration;
impl sea_orm_migration::MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261024_000001_execution_snapshot"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(HallExecutionSnapshot::Table)
                    .col(
                        ColumnDef::new(HallExecutionSnapshot::SnapshotId)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    // Soft identity: history must not disappear with a later registry edit.
                    .col(
                        ColumnDef::new(HallExecutionSnapshot::MemberId)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HallExecutionSnapshot::MemberRevision)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HallExecutionSnapshot::CheckVersion)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HallExecutionSnapshot::SnapshotVersion)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HallExecutionSnapshot::Payload)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HallExecutionSnapshot::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .index(
                        Index::create()
                            .name("hall_snapshot_member_version")
                            .unique()
                            .col(HallExecutionSnapshot::MemberId)
                            .col(HallExecutionSnapshot::SnapshotVersion),
                    )
                    .to_owned(),
            )
            .await
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(HallExecutionSnapshot::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum HallExecutionSnapshot {
    Table,
    SnapshotId,
    MemberId,
    MemberRevision,
    CheckVersion,
    SnapshotVersion,
    Payload,
    CreatedAt,
}
