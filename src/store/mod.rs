mod accounts;
mod connection;
mod cycle;
mod entity;
mod events;
mod records;
mod runs;

use std::error::Error;
use std::fmt;
use std::time::Duration;

use monitor_hub_plugin::migrate::{MigrateError, Migration, up};
use sea_orm::sqlx::postgres::{PgDatabaseError, PgSeverity};
use sea_orm::{ConnAcquireErr, DatabaseConnection, DbErr, RuntimeErr};
use tokio::task::{JoinError, JoinHandle};
use tokio_postgres::{Client, Config, NoTls};

use crate::secret::Secret;
use connection::{CONNECT_TIMEOUT, database_options, pool, with_pool_limits};
use events::Skipped;

pub use records::{
    Account, AccountInfo, AccountKey, CycleChanges, CycleRun, DocumentRow, ProcessRow, Pruned,
    Trigger,
};

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "00001_initial.sql",
        sql: include_str!("../../sql/schema/00001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "00002_cycle_runs.sql",
        sql: include_str!("../../sql/schema/00002_cycle_runs.sql"),
    },
];

pub const WRITER_LOCK_ID: i64 = 0x7365_692d_6d6f_6e69;

const KEEPALIVES_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVES_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVES_RETRIES: u32 = 3;
const TCP_USER_TIMEOUT: Duration = Duration::from_secs(60);

const TRY_WRITER_LOCK: &str = "SELECT pg_try_advisory_lock($1)";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("DATABASE_URL is not a valid Postgres address")]
    InvalidUrl,
    #[error("another sei-monitor process holds the database lock")]
    Locked,
    #[error("monitor: account not found")]
    AccountNotFound,
    #[error("the database did not answer in time: {0}")]
    Timeout(&'static str),
    #[error("db error")]
    Server(#[source] ServerError),
    #[error(transparent)]
    Database(DbErr),
    #[error(transparent)]
    LockConnection(#[from] tokio_postgres::Error),
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    #[error("too many stored events cannot be read: {0} rows read")]
    TooManyUnreadableEvents(u64),
}

impl StoreError {
    pub fn sql_state(&self) -> Option<&str> {
        let StoreError::Server(error) = self else {
            return None;
        };
        Some(error.code())
    }
}

impl From<DbErr> for StoreError {
    fn from(error: DbErr) -> StoreError {
        if matches!(error, DbErr::ConnectionAcquire(ConnAcquireErr::Timeout)) {
            return StoreError::Timeout("getting a connection");
        }
        match server_error(&error) {
            Some(server) => StoreError::Server(server),
            None => StoreError::Database(error),
        }
    }
}

fn server_error(error: &DbErr) -> Option<ServerError> {
    let (DbErr::Exec(RuntimeErr::SqlxError(error))
    | DbErr::Query(RuntimeErr::SqlxError(error))
    | DbErr::Conn(RuntimeErr::SqlxError(error))) = error
    else {
        return None;
    };
    let error = error
        .as_database_error()?
        .try_downcast_ref::<PgDatabaseError>()?;
    Some(ServerError {
        severity: severity_name(error.severity()).to_owned(),
        code: error.code().to_owned(),
        message: error.message().to_owned(),
        detail: error.detail().map(str::to_owned),
        hint: error.hint().map(str::to_owned),
    })
}

fn severity_name(severity: PgSeverity) -> &'static str {
    let names = [
        (PgSeverity::Panic, "PANIC"),
        (PgSeverity::Fatal, "FATAL"),
        (PgSeverity::Error, "ERROR"),
        (PgSeverity::Warning, "WARNING"),
        (PgSeverity::Notice, "NOTICE"),
        (PgSeverity::Debug, "DEBUG"),
        (PgSeverity::Info, "INFO"),
        (PgSeverity::Log, "LOG"),
    ];
    names
        .into_iter()
        .find(|(known, _)| *known == severity)
        .map(|(_, name)| name)
        .expect("every Postgres severity has a name")
}

/// An error the Postgres server answered with. It prints as `tokio-postgres` printed one, so
/// `Status` and `cycle_runs.error` keep showing `db error: ERROR: <message>`.
#[derive(Debug, Clone)]
pub struct ServerError {
    severity: String,
    code: String,
    message: String,
    detail: Option<String>,
    hint: Option<String>,
}

impl ServerError {
    pub fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.severity, self.message)?;
        if let Some(detail) = &self.detail {
            write!(f, "\nDETAIL: {detail}")?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\nHINT: {hint}")?;
        }
        Ok(())
    }
}

impl Error for ServerError {}

pub struct WriterLock {
    client: Client,
    connection: Option<JoinHandle<Result<(), tokio_postgres::Error>>>,
    close_reason: Option<String>,
}

pub async fn acquire_writer_lock(database_url: &Secret) -> Result<WriterLock, StoreError> {
    let config = lock_config(database_url)?;
    tokio::time::timeout(CONNECT_TIMEOUT, take_writer_lock(&config))
        .await
        .map_err(|_| StoreError::Timeout("taking the writer lock"))?
}

async fn take_writer_lock(config: &Config) -> Result<WriterLock, StoreError> {
    let (client, connection) = config.connect(NoTls).await?;
    let connection = tokio::spawn(connection);
    let locked: bool = client
        .query_one(TRY_WRITER_LOCK, &[&WRITER_LOCK_ID])
        .await?
        .try_get(0)?;
    if !locked {
        return Err(StoreError::Locked);
    }
    Ok(WriterLock {
        client,
        connection: Some(connection),
        close_reason: None,
    })
}

impl WriterLock {
    pub async fn migrate(&mut self) -> Result<Vec<i64>, StoreError> {
        Ok(up(&mut self.client, MIGRATIONS).await?)
    }

    pub async fn closed(&mut self) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let ended = connection.await;
        self.connection = None;
        self.close_reason = Some(reason_for(ended));
    }

    pub fn close_reason(&self) -> Option<String> {
        self.close_reason.clone()
    }
}

#[derive(Clone)]
pub struct Store {
    connection: DatabaseConnection,
    skipped: Skipped,
}

impl Store {
    pub fn new(database_url: &Secret) -> Result<Store, StoreError> {
        let options = with_pool_limits(database_options(database_url)?);
        Ok(Store {
            connection: pool(options),
            skipped: Skipped::default(),
        })
    }

    fn connection(&self) -> &DatabaseConnection {
        &self.connection
    }
}

fn count(total: usize) -> i64 {
    i64::try_from(total).unwrap_or(i64::MAX)
}

fn lock_config(database_url: &Secret) -> Result<Config, StoreError> {
    let mut config: Config = database_url
        .reveal()
        .parse()
        .map_err(|_| StoreError::InvalidUrl)?;
    config
        .keepalives(true)
        .keepalives_idle(KEEPALIVES_IDLE)
        .keepalives_interval(KEEPALIVES_INTERVAL)
        .keepalives_retries(KEEPALIVES_RETRIES)
        .tcp_user_timeout(TCP_USER_TIMEOUT);
    Ok(config)
}

fn reason_for(ended: Result<Result<(), tokio_postgres::Error>, JoinError>) -> String {
    match ended {
        Ok(Ok(())) => "the connection closed".to_owned(),
        Ok(Err(error)) => chain_text(&error),
        Err(error) => chain_text(&error),
    }
}

fn chain_text(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(current) = cause {
        let shown = current.to_string();
        if !text.ends_with(&shown) {
            text.push_str(": ");
            text.push_str(&shown);
        }
        cause = current.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::postgres::database;
    use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
    use tokio_postgres::{Client, Config};

    use super::connection::{CONNECT_TIMEOUT, POOL_SIZE, database_options, with_pool_limits};
    use super::{Store, StoreError, acquire_writer_lock, lock_config};
    use crate::secret::Secret;

    const URL: &str = "postgres://sei:hunter2@db.internal:5432/sei";
    const SLACK: Duration = Duration::from_secs(3);

    fn with_query(url: &str, parameter: &str) -> Secret {
        let separator = if url.contains('?') { '&' } else { '?' };
        Secret::new(format!("{url}{separator}{parameter}"))
    }

    async fn show(client: &Client, setting: &str) -> String {
        client
            .query_one(&format!("SHOW {setting}"), &[])
            .await
            .expect("show")
            .get(0)
    }

    async fn shown(connection: &DatabaseConnection, setting: &str) -> String {
        connection
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                format!("SHOW {setting}"),
            ))
            .await
            .expect("show")
            .expect("a row")
            .try_get_by_index(0)
            .expect("a setting")
    }

    fn assert_keepalives(config: &Config) {
        assert!(config.get_keepalives());
        assert_eq!(config.get_keepalives_idle(), Duration::from_secs(30));
        assert_eq!(
            config.get_keepalives_interval(),
            Some(Duration::from_secs(10))
        );
        assert_eq!(config.get_keepalives_retries(), Some(3));
        assert_eq!(
            config.get_tcp_user_timeout(),
            Some(&Duration::from_secs(60))
        );
    }

    #[test]
    fn the_store_settings_win_over_the_urls_own() {
        let url = with_query(
            URL,
            "options=-c%20statement_timeout%3D1&keepalives=0&keepalives_idle=5&keepalives_interval=1&keepalives_retries=9&tcp_user_timeout=1",
        );

        let pool = with_pool_limits(database_options(&url).expect("pool options"));
        assert_eq!(
            pool.get_options(),
            Some(
                "-c statement_timeout=1 -c tcp_keepalives_idle=30 -c tcp_keepalives_interval=10 -c tcp_keepalives_count=3 -c tcp_user_timeout=60000 -c statement_timeout=20000 -c lock_timeout=5000"
            )
        );

        let lock = lock_config(&url).expect("lock config");
        assert_eq!(lock.get_options(), Some("-c statement_timeout=1"));
        assert_keepalives(&lock);

        let plain = lock_config(&Secret::new(URL.to_owned())).expect("lock config");
        assert_eq!(plain.get_options(), None);
        assert_keepalives(&plain);
    }

    #[test]
    fn database_options_accept_only_the_documented_forms() {
        let accepted = [
            (
                "postgres://sei:hunter2@db.internal:5432/sei?sslmode=disable",
                5432,
            ),
            ("postgresql://sei:hunter2@db.internal/sei", 5432),
        ];
        for (url, port) in accepted {
            let options = database_options(&Secret::new(url.to_owned()))
                .unwrap_or_else(|error| panic!("{url}: {error}"));
            assert_eq!(
                (
                    options.get_host(),
                    options.get_port(),
                    options.get_username(),
                    options.get_database()
                ),
                ("db.internal", port, "sei", Some("sei")),
                "{url}"
            );
        }

        let refused = [
            "postgres://sei:hunter2@h1,h2/sei",
            "postgres://sei:hunter2@h1:5432,h2:5432/sei",
            "mysql://sei:hunter2@db/sei",
            "postgres://sei:hunter2@db/sei?sslmode=sometimes",
        ];
        for url in refused {
            let error = database_options(&Secret::new(url.to_owned())).err();
            assert!(
                matches!(error, Some(StoreError::InvalidUrl)),
                "{url}: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn pool_connections_carry_their_limits_and_keepalives() {
        let Some(db) = database().await else { return };
        let url = with_query(
            &db.url,
            "options=-c%20statement_timeout%3D1%20-c%20application_name%3Dsei_test",
        );

        let store = Store::new(&url).expect("store");

        let expected = [
            ("statement_timeout", "20s"),
            ("lock_timeout", "5s"),
            ("application_name", "sei_test"),
            ("tcp_keepalives_idle", "30"),
            ("tcp_keepalives_interval", "10"),
            ("tcp_keepalives_count", "3"),
            ("tcp_user_timeout", "60000"),
        ];
        for (setting, value) in expected {
            assert_eq!(shown(store.connection(), setting).await, value, "{setting}");
        }
    }

    #[tokio::test]
    async fn pool_and_lock_connections_carry_their_limits() {
        let Some(db) = database().await else { return };

        let lock = acquire_writer_lock(&Secret::new(db.url.clone()))
            .await
            .expect("lock");
        assert_eq!(show(&lock.client, "statement_timeout").await, "0");
        assert_eq!(show(&lock.client, "lock_timeout").await, "0");
    }

    #[tokio::test]
    async fn a_full_pool_times_out_getting_a_connection() {
        let Some(db) = database().await else { return };
        let store = Store::new(&Secret::new(db.url.clone())).expect("store");
        let mut held = Vec::new();
        for _ in 0..POOL_SIZE {
            held.push(
                store
                    .connection()
                    .begin()
                    .await
                    .expect("a pooled connection"),
            );
        }

        let started = Instant::now();
        let error = tokio::time::timeout(CONNECT_TIMEOUT + SLACK, store.last_cycle_run(1))
            .await
            .expect("a full pool held the call past its wait limit")
            .expect_err("a call answered with every connection held");
        assert!(
            started.elapsed() >= CONNECT_TIMEOUT,
            "{:?}",
            started.elapsed()
        );
        assert!(
            matches!(error, StoreError::Timeout("getting a connection")),
            "{error:?}"
        );

        held.pop();
        let released = tokio::time::timeout(SLACK, store.connection().begin())
            .await
            .expect("the released connection came back in time")
            .expect("the released connection");
        held.push(released);
    }
}
