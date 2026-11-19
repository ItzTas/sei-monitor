use sea_orm::sqlx::postgres::PgDatabaseError;
use sea_orm::{ConnectionTrait, DbBackend, DeriveMigrationName, RuntimeErr, Statement};
use sea_orm_migration::{DbErr, MigrationTrait, SchemaManager, async_trait};

const CURRENT_SCHEMA: &str = "SELECT quote_ident(current_schema())";
// starts_with rather than LIKE 'whatsmeow\_%': with standard_conforming_strings off the backslash
// is eaten and the pattern would also match a table such as whatsmeowbackup.
const WHATSMEOW_TABLES: &str = "SELECT format('%I.%I', table_schema, table_name) FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' AND starts_with(table_name::text, 'whatsmeow_') ORDER BY table_name";

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let schema: String = db
            .query_one_raw(statement(CURRENT_SCHEMA))
            .await?
            .ok_or_else(|| DbErr::Migration("the current schema query returned no row".to_owned()))?
            .try_get_by_index(0)?;
        let tables = db
            .query_all_raw(statement(WHATSMEOW_TABLES))
            .await?
            .iter()
            .map(|row| row.try_get_by_index::<String>(0))
            .collect::<Result<Vec<_>, _>>()?;
        // Qualified with the schema the adoption read: unqualified, a table missing there would
        // resolve through search_path to another schema's table.
        // No CASCADE, so an object outside these tables that depends on one of them stops the
        // drop and the transaction undoes it. The whatsmeow tables share a statement because
        // their foreign keys point at each other, so a refusal there names its table only in
        // Postgres's detail, which with_detail adds; the other two get their own statements.
        let mut drop = format!(
            "DROP TABLE IF EXISTS {schema}.deliveries; DROP TABLE IF EXISTS {schema}.goose_db_version;"
        );
        if !tables.is_empty() {
            drop.push_str(&format!(" DROP TABLE IF EXISTS {};", tables.join(", ")));
        }
        db.execute_unprepared(&drop).await.map_err(with_detail)?;
        Ok(())
    }
}

fn with_detail(error: DbErr) -> DbErr {
    let DbErr::Exec(RuntimeErr::SqlxError(sqlx)) = &error else {
        return error;
    };
    let Some(database) = sqlx
        .as_database_error()
        .and_then(|database| database.try_downcast_ref::<PgDatabaseError>())
    else {
        return error;
    };
    let Some(detail) = database.detail() else {
        return error;
    };
    DbErr::Migration(format!("{}: {detail}", database.message()))
}

fn statement(sql: &str) -> Statement {
    Statement::from_string(DbBackend::Postgres, sql)
}
