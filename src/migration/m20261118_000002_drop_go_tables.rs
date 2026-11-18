use sea_orm::{ConnectionTrait, DbBackend, DeriveMigrationName, Statement};
use sea_orm_migration::{DbErr, MigrationTrait, SchemaManager, async_trait};

// starts_with rather than LIKE 'whatsmeow\_%': with standard_conforming_strings off the backslash
// is eaten and the pattern would also match a table such as whatsmeowbackup.
const WHATSMEOW_TABLES: &str = "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' AND starts_with(table_name::text, 'whatsmeow_') ORDER BY table_name";

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let tables = db
            .query_all_raw(Statement::from_string(
                DbBackend::Postgres,
                WHATSMEOW_TABLES,
            ))
            .await?
            .iter()
            .map(|row| row.try_get_by_index::<String>(0).map(|name| quoted(&name)))
            .collect::<Result<Vec<_>, _>>()?;
        // No CASCADE, so an object outside these tables that depends on one of them stops the
        // drop and the transaction undoes it. The whatsmeow tables share a statement because
        // their foreign keys point at each other; the other two get their own so an error names
        // the table.
        let mut drop =
            String::from("DROP TABLE IF EXISTS deliveries; DROP TABLE IF EXISTS goose_db_version;");
        if !tables.is_empty() {
            drop.push_str(&format!(" DROP TABLE IF EXISTS {};", tables.join(", ")));
        }
        db.execute_unprepared(&drop).await?;
        Ok(())
    }
}

fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
