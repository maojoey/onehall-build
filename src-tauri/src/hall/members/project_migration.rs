use sea_orm_migration::prelude::*;

pub struct Migration;

impl sea_orm_migration::MigrationName for Migration {
    fn name(&self) -> &str {
        "onehall_m20261021_000001_member_project"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(HallMember::Table)
                    // Soft reference: removing a project must not erase identity
                    // or silently turn an unavailable binding into 'unbound'.
                    .add_column(ColumnDef::new(HallMember::ProjectId).integer())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(HallMember::Table)
                    .drop_column(HallMember::ProjectId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum HallMember {
    Table,
    ProjectId,
}
