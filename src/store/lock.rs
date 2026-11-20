use std::future::Future;
use std::time::Duration;

use sea_orm::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sea_orm::{
    ConnAcquireErr, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, SqlxPostgresConnector,
    Statement, TransactionTrait,
};
use sea_orm_migration::MigratorTrait;
use tokio::time::{MissedTickBehavior, interval, timeout};

use super::connection::{CONNECT_TIMEOUT, database_options};
use super::{StoreError, chain_text};
use crate::migration::Migrator;
use crate::secret::Secret;

pub const WRITER_LOCK_ID: i64 = 0x7365_692d_6d6f_6e69;
pub const LOCK_CHECK_EVERY: Duration = Duration::from_millis(500);
// Thirty seconds and no less: a false loss stops the process, and the next start logs in to SEI.
// The server frees the lock of a connection that vanished only after about 60 s (its keepalives
// and tcp_user_timeout), so 30.5 s still comes first.
pub const LOCK_CHECK_LIMIT: Duration = Duration::from_secs(30);
pub(crate) const LOCK_LOST: &str = "the database lock was lost";

const TRY_WRITER_LOCK: &str = "SELECT pg_try_advisory_lock($1), pg_backend_pid()";
// Gives a row only while the backend that runs it holds the writer lock.
const LOCK_HOLDER: &str = "SELECT pg_backend_pid() FROM pg_locks
WHERE locktype = 'advisory' AND pid = pg_backend_pid() AND granted
  AND objsubid = 1 AND ((classid::bigint << 32) | objid::bigint) = $1";
const TAKING_THE_LOCK: &str = "taking the writer lock";
const REPLACED: &str = "the lock's connection was replaced";

pub struct WriterLock {
    connection: DatabaseConnection,
    backend: i32,
    close_reason: Option<String>,
}

pub async fn acquire_writer_lock(database_url: &Secret) -> Result<WriterLock, StoreError> {
    let connection = single_connection(connect_options(database_url)?);
    let Ok(taken) = timeout(CONNECT_TIMEOUT, try_lock(&connection)).await else {
        return Err(StoreError::Timeout(TAKING_THE_LOCK));
    };
    let (locked, backend) = taken?;
    if !locked {
        return Err(StoreError::Locked);
    }
    Ok(WriterLock {
        connection,
        backend,
        close_reason: None,
    })
}

impl WriterLock {
    pub async fn migrate(&self) -> Result<Vec<String>, StoreError> {
        // One transaction keeps the check and every migration on the connection that holds the
        // lock: the pool could otherwise hand over a new connection, one without it.
        let transaction = answered(self.connection.begin())
            .await
            .map_err(StoreError::LockLost)?;
        self.check(&transaction)
            .await
            .map_err(StoreError::LockLost)?;
        // Read without creating the migration table, so a role with table rights only still
        // starts once nothing is pending, as it did with 2.1.
        let pending = Migrator::get_pending_migrations_read_only(&transaction).await?;
        if !pending.is_empty() {
            Migrator::up(&transaction, None)
                .await
                .map_err(migration_error)?;
        }
        transaction.commit().await?;
        Ok(pending
            .iter()
            .map(|migration| migration.name().to_owned())
            .collect())
    }

    pub async fn closed(&mut self) {
        if self.close_reason.is_some() {
            return;
        }
        let mut checks = interval(LOCK_CHECK_EVERY);
        checks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            checks.tick().await;
            if let Err(reason) = self.check(&self.connection).await {
                self.close_reason = Some(reason);
                return;
            }
        }
    }

    pub fn close_reason(&self) -> Option<String> {
        self.close_reason.clone()
    }

    async fn check(&self, on: &impl ConnectionTrait) -> Result<(), String> {
        let holder = answered(lock_holder(on)).await?;
        if holder != Some(self.backend) {
            return Err(REPLACED.to_owned());
        }
        Ok(())
    }
}

pub(super) fn connect_options(database_url: &Secret) -> Result<PgConnectOptions, StoreError> {
    // Keeps the migrations' "already exists, skipping" notices out of the log.
    Ok(database_options(database_url)?.options([("client_min_messages", "warning")]))
}

fn single_connection(options: PgConnectOptions) -> DatabaseConnection {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        // sqlx tests the connection each time it comes back to the pool; waiting for that test
        // counts toward the loss limit, as a check that gets no answer does.
        .acquire_timeout(LOCK_CHECK_LIMIT)
        .idle_timeout(None::<Duration>)
        .max_lifetime(None::<Duration>)
        // A failed test before use would quietly open a new connection, one without the lock.
        .test_before_acquire(false)
        .connect_lazy_with(options);
    SqlxPostgresConnector::from_sqlx_postgres_pool(pool)
}

async fn try_lock(connection: &DatabaseConnection) -> Result<(bool, i32), StoreError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        TRY_WRITER_LOCK,
        [WRITER_LOCK_ID.into()],
    );
    let row = connection
        .query_one_raw(statement)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("the writer lock query gave no row".into()))?;
    Ok((row.try_get_by_index(0)?, row.try_get_by_index(1)?))
}

async fn lock_holder(on: &impl ConnectionTrait) -> Result<Option<i32>, DbErr> {
    let statement =
        Statement::from_sql_and_values(DbBackend::Postgres, LOCK_HOLDER, [WRITER_LOCK_ID.into()]);
    on.query_one_raw(statement)
        .await?
        .map(|row| row.try_get_by_index(0))
        .transpose()
}

async fn answered<T>(work: impl Future<Output = Result<T, DbErr>>) -> Result<T, String> {
    let no_answer = format!("the lock's connection did not answer within {LOCK_CHECK_LIMIT:?}");
    let Ok(done) = timeout(LOCK_CHECK_LIMIT, work).await else {
        return Err(no_answer);
    };
    done.map_err(|error| {
        // The pool's wait for the connection ends at the same limit, and wins a tie.
        if matches!(error, DbErr::ConnectionAcquire(ConnAcquireErr::Timeout)) {
            return no_answer;
        }
        format!("the lock's connection failed: {}", chain_text(&error))
    })
}

fn migration_error(error: DbErr) -> StoreError {
    let DbErr::Migration(text) = error else {
        return StoreError::from(error);
    };
    StoreError::Migration(text)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sea_orm::{ConnectionTrait, DbBackend, Statement};

    use super::{WRITER_LOCK_ID, WriterLock, acquire_writer_lock};
    use crate::postgres::database;
    use crate::secret::Secret;
    use crate::store::StoreError;

    async fn answer<T: sea_orm::TryGetable>(lock: &WriterLock, statement: Statement) -> T {
        lock.connection
            .query_one_raw(statement)
            .await
            .expect("a query on the lock's connection")
            .expect("a row")
            .try_get_by_index(0)
            .expect("a value")
    }

    #[tokio::test]
    async fn the_lock_connection_carries_keepalives_and_no_limits() {
        let Some(db) = database().await else { return };
        let lock = acquire_writer_lock(&Secret::new(db.url.clone()))
            .await
            .expect("lock");

        let expected = [
            ("statement_timeout", "0"),
            ("lock_timeout", "0"),
            ("tcp_keepalives_idle", "30"),
            ("tcp_keepalives_interval", "10"),
            ("tcp_keepalives_count", "3"),
            ("tcp_user_timeout", "60000"),
            ("client_min_messages", "warning"),
        ];
        for (setting, value) in expected {
            let show = Statement::from_string(DbBackend::Postgres, format!("SHOW {setting}"));
            let shown: String = answer(&lock, show).await;
            assert_eq!(shown, value, "{setting}");
        }
    }

    #[tokio::test]
    async fn a_connection_that_no_longer_holds_the_lock_neither_migrates_nor_stays() {
        let Some(db) = database().await else { return };
        let mut lock = acquire_writer_lock(&Secret::new(db.url.clone()))
            .await
            .expect("lock");
        let unlock = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_unlock($1)",
            [WRITER_LOCK_ID.into()],
        );
        let released: bool = answer(&lock, unlock).await;
        assert!(released);

        let error = lock
            .migrate()
            .await
            .expect_err("a connection without the lock migrated");

        assert!(matches!(error, StoreError::LockLost(_)), "{error:?}");
        assert_eq!(
            error.to_string(),
            "the database lock was lost: the lock's connection was replaced"
        );
        let migrated = Statement::from_string(
            DbBackend::Postgres,
            "SELECT to_regclass('seaql_migrations') IS NOT NULL",
        );
        let migrated: bool = answer(&lock, migrated).await;
        assert!(!migrated, "the migrations ran without the lock");
        tokio::time::timeout(Duration::from_secs(1), lock.closed())
            .await
            .expect("closed resolves on a connection without the lock");
        assert_eq!(
            lock.close_reason().as_deref(),
            Some("the lock's connection was replaced")
        );
    }
}
