use std::collections::HashSet;

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};

const GOOSE_FILES: [(i64, &str); 2] = [
    (1, include_str!("../../sql/schema/00001_initial.sql")),
    (2, include_str!("../../sql/schema/00002_cycle_runs.sql")),
];

const DOWN_LINE: &str = "-- +goose Down";
const TABLE_EXISTS: &str = "SELECT EXISTS ( SELECT 1 FROM pg_tables WHERE (current_schema() IS NULL OR schemaname = current_schema()) AND tablename = 'goose_db_version' )";
const LIST_VERSIONS: &str = "SELECT version_id FROM goose_db_version ORDER BY id DESC";

#[derive(Debug, thiserror::Error)]
pub(crate) enum GooseError {
    #[error(
        "migration version {missing} is not applied but the database is already at version {current}"
    )]
    OutOfOrder { missing: i64, current: i64 },
    #[error("goose_db_version has no rows, not even the version 0 row goose expects")]
    MissingZeroVersion,
    #[error(transparent)]
    Database(#[from] DbErr),
}

pub(crate) fn up_section(file: &str) -> &str {
    let mut offset = 0;
    for line in file.split_inclusive('\n') {
        if line.trim_end() == DOWN_LINE {
            return &file[..offset];
        }
        offset += line.len();
    }
    file
}

/// Runs the goose versions the database lacks with goose's own SQL, refusing what the SDK's
/// runner refused. It writes no goose row: the next migration drops `goose_db_version`.
pub(crate) async fn catch_up(db: &impl ConnectionTrait) -> Result<Vec<i64>, GooseError> {
    let recorded = recorded_versions(db).await?;
    let current = recorded.iter().copied().max().unwrap_or(0);
    let pending: Vec<(i64, &str)> = GOOSE_FILES
        .into_iter()
        .filter(|(version, _)| !recorded.contains(version))
        .collect();
    if let Some(&(missing, _)) = pending.iter().find(|(version, _)| *version < current) {
        return Err(GooseError::OutOfOrder { missing, current });
    }
    let mut applied = Vec::with_capacity(pending.len());
    for (version, file) in pending {
        db.execute_unprepared(up_section(file)).await?;
        applied.push(version);
    }
    Ok(applied)
}

async fn recorded_versions(db: &impl ConnectionTrait) -> Result<HashSet<i64>, GooseError> {
    let exists = match db.query_one_raw(statement(TABLE_EXISTS)).await? {
        Some(row) => row.try_get_by_index::<bool>(0)?,
        None => false,
    };
    if !exists {
        return Ok(HashSet::new());
    }
    let rows = db.query_all_raw(statement(LIST_VERSIONS)).await?;
    if rows.is_empty() {
        return Err(GooseError::MissingZeroVersion);
    }
    Ok(rows
        .iter()
        .map(|row| row.try_get_by_index::<i64>(0))
        .collect::<Result<_, _>>()?)
}

fn statement(sql: &str) -> Statement {
    Statement::from_string(DbBackend::Postgres, sql)
}

#[cfg(test)]
mod tests {
    use super::{GOOSE_FILES, up_section};

    #[test]
    fn up_sections_stop_before_the_down_section() {
        let [(1, initial), (2, cycle_runs)] = GOOSE_FILES else {
            panic!("the goose files are versions 1 and 2");
        };

        let initial = up_section(initial);
        assert!(initial.contains("CREATE TABLE accounts"), "{initial}");
        assert!(!initial.contains("DROP TABLE"), "{initial}");

        let cycle_runs = up_section(cycle_runs);
        assert!(
            cycle_runs.contains("CREATE TABLE cycle_runs"),
            "{cycle_runs}"
        );
        assert!(!cycle_runs.contains("DROP COLUMN"), "{cycle_runs}");
    }
}
