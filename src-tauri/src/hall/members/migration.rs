use sea_orm_migration::prelude::*;

pub struct Migration;

impl sea_orm_migration::MigrationName for Migration {
    fn name(&self) -> &str {
        // Independent of upstream dates and this module's generic filename.
        "onehall_m20261020_000001_member"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(HallMember::Table)
                    .col(
                        ColumnDef::new(HallMember::MemberId)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(HallMember::Name).text().not_null())
                    .col(ColumnDef::new(HallMember::Role).text().not_null())
                    .col(
                        ColumnDef::new(HallMember::Cwd)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(HallMember::Engine)
                            .text()
                            .not_null()
                            .default("codex"),
                    )
                    // Soft references: deleting a template must not delete a
                    // member or later task history. Dispatch will revalidate.
                    .col(ColumnDef::new(HallMember::AssistantUid).text())
                    .col(ColumnDef::new(HallMember::AssistantRevision).big_integer())
                    .col(
                        ColumnDef::new(HallMember::Enabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(HallMember::Revision)
                            .big_integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(HallMember::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HallMember::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(HallMember::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum HallMember {
    Table,
    MemberId,
    Name,
    Role,
    Cwd,
    Engine,
    AssistantUid,
    AssistantRevision,
    Enabled,
    Revision,
    CreatedAt,
    UpdatedAt,
}
