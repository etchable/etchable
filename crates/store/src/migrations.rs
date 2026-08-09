//! Embedded schema migrations (sea-orm-migration). Rust migrations compile
//! into the binary, run inside `Store::open`, and are tracked in the
//! `seaql_migrations` table — a db touched by a newer build (applied
//! migrations this build doesn't know) makes `up` fail, which the caller
//! treats as "run without persistence", never wipe.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(InitMigration)]
    }
}

#[derive(DeriveMigrationName)]
pub struct InitMigration;

#[derive(DeriveIden)]
enum Projects {
    Table,
    Root,
    Name,
    Board,
    LastOpenedAt,
}

#[derive(DeriveIden)]
enum AgentSessions {
    Table,
    SessionId,
    WorkspaceRoot,
    Title,
    Model,
    ResumedFrom,
    CreatedAt,
    LastUsedAt,
}

#[derive(DeriveIden)]
enum Prefs {
    Table,
    Key,
    Value,
}

#[async_trait::async_trait]
impl MigrationTrait for InitMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Projects::Table)
                    .col(ColumnDef::new(Projects::Root).text().not_null().primary_key())
                    .col(ColumnDef::new(Projects::Name).text().not_null())
                    .col(ColumnDef::new(Projects::Board).text())
                    .col(ColumnDef::new(Projects::LastOpenedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AgentSessions::Table)
                    .col(
                        ColumnDef::new(AgentSessions::SessionId)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgentSessions::WorkspaceRoot).text().not_null())
                    .col(ColumnDef::new(AgentSessions::Title).text())
                    .col(ColumnDef::new(AgentSessions::Model).text())
                    .col(ColumnDef::new(AgentSessions::ResumedFrom).text())
                    .col(ColumnDef::new(AgentSessions::CreatedAt).big_integer().not_null())
                    .col(ColumnDef::new(AgentSessions::LastUsedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("agent_sessions_ws")
                    .table(AgentSessions::Table)
                    .col(AgentSessions::WorkspaceRoot)
                    .col(AgentSessions::LastUsedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Prefs::Table)
                    .col(ColumnDef::new(Prefs::Key).text().not_null().primary_key())
                    .col(ColumnDef::new(Prefs::Value).text().not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Prefs::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AgentSessions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Projects::Table).to_owned())
            .await
    }
}
