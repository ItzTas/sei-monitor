use sea_orm::DeriveMigrationName;
use sea_orm_migration::{DbErr, MigrationTrait, SchemaManager, async_trait};

use super::goose::{GooseError, catch_up};

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        match catch_up(manager.get_connection()).await {
            Ok(_) => Ok(()),
            Err(GooseError::Database(error)) => Err(error),
            Err(error) => Err(DbErr::Migration(error.to_string())),
        }
    }
}
