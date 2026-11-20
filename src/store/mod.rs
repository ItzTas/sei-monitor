mod accounts;
mod connection;
mod cycle;
mod entity;
mod events;
mod lock;
mod records;
mod runs;

use std::error::Error;
use std::fmt;

use monitor_hub_plugin::Notifier;
use sea_orm::sqlx::postgres::{PgDatabaseError, PgSeverity};
use sea_orm::{ConnAcquireErr, DatabaseConnection, DbErr, RuntimeErr};

use crate::secret::Secret;
use connection::{database_options, pool, with_pool_limits};
use events::Skipped;
pub(crate) use lock::LOCK_LOST;
pub use lock::{
    LOCK_CHECK_EVERY, LOCK_CHECK_LIMIT, WRITER_LOCK_ID, WriterLock, acquire_writer_lock,
};

pub use records::{
    Account, AccountInfo, AccountKey, CycleChanges, CycleRun, DocumentRow, ProcessRow, Pruned,
    Trigger,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    InvalidUrl(&'static str),
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
    #[error("{LOCK_LOST}: {0}")]
    LockLost(String),
    #[error("{0}")]
    Migration(String),
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

#[derive(Clone)]
pub struct Store {
    connection: DatabaseConnection,
    notifier: Notifier,
    skipped: Skipped,
}

impl Store {
    pub fn new(database_url: &Secret) -> Result<Store, StoreError> {
        let options = with_pool_limits(database_options(database_url)?);
        Ok(Store {
            connection: pool(options),
            notifier: Notifier::new(),
            skipped: Skipped::default(),
        })
    }

    pub(crate) fn with_notifier(self, notifier: Notifier) -> Store {
        Store { notifier, ..self }
    }

    pub fn notifier(&self) -> &Notifier {
        &self.notifier
    }

    fn connection(&self) -> &DatabaseConnection {
        &self.connection
    }
}

fn count(total: usize) -> i64 {
    i64::try_from(total).unwrap_or(i64::MAX)
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

    use super::connection::{CONNECT_TIMEOUT, POOL_SIZE, database_options, with_pool_limits};
    use super::lock::connect_options;
    use super::{Store, StoreError};
    use crate::secret::Secret;

    const URL: &str = "postgres://sei:hunter2@db.internal:5432/sei";
    const SLACK: Duration = Duration::from_secs(3);

    fn with_query(url: &str, parameter: &str) -> Secret {
        let separator = if url.contains('?') { '&' } else { '?' };
        Secret::new(format!("{url}{separator}{parameter}"))
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

        let lock = connect_options(&url).expect("lock options");
        assert_eq!(
            lock.get_options(),
            Some(
                "-c statement_timeout=1 -c tcp_keepalives_idle=30 -c tcp_keepalives_interval=10 -c tcp_keepalives_count=3 -c tcp_user_timeout=60000 -c client_min_messages=warning"
            )
        );

        let plain = connect_options(&Secret::new(URL.to_owned())).expect("lock options");
        assert_eq!(
            plain.get_options(),
            Some(
                "-c tcp_keepalives_idle=30 -c tcp_keepalives_interval=10 -c tcp_keepalives_count=3 -c tcp_user_timeout=60000 -c client_min_messages=warning"
            )
        );
    }

    #[test]
    fn database_options_accept_only_the_documented_forms() {
        let accepted = [
            (
                "postgres://sei:hunter2@db.internal:5432/sei?sslmode=disable",
                5432,
            ),
            ("postgresql://sei:hunter2@db.internal/sei", 5432),
            (
                "postgres://sei:hunter2@db.internal/sei?sslnegotiation=postgres&connect_timeout=5&tcp_user_timeout=5&keepalives=1&keepalives_idle=5&keepalives_interval=5&keepalives_retries=3&target_session_attrs=any&channel_binding=disable&load_balance_hosts=disable",
                5432,
            ),
            (
                "postgres://sei:hunter2@db.internal/sei?options[application_name]=sei",
                5432,
            ),
            (
                "postgres://sei:hunter2@db.internal/sei?channel_binding=prefer",
                5432,
            ),
            (
                "postgres:///sei?host=db.internal&user=sei&password=hunter2",
                5432,
            ),
            ("postgres://sei@db.internal/sei?password=hunter2", 5432),
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

        let ipv6 = database_options(&Secret::new(
            "postgres://sei:hunter2@[::1]:5433/sei".to_owned(),
        ))
        .expect("an IPv6 address");
        assert_eq!((ipv6.get_host(), ipv6.get_port()), ("::1", 5433));

        let refused = [
            "postgres://sei:hunter2@h1,h2/sei",
            "postgres://sei:hunter2@h1:5432,h2:5432/sei",
            "mysql://sei:hunter2@db/sei",
            "postgres://sei:hunter2@db/sei?sslmode=sometimes",
            "postgres://sei:hunter2@db/sei?sslpassword=x",
            "postgres://sei:hunter2@db/sei?Password=x",
            "postgres:///sei",
            "postgres://sei:hunter2@/sei",
            "postgres://sei:hunter2@:5432/sei",
            "postgresql:///sei?user=sei",
            "postgres:///sei?host=&user=sei&password=hunter2",
            "postgres:///sei?hostaddr=&user=sei",
        ];
        for url in refused {
            let error = database_options(&Secret::new(url.to_owned())).err();
            assert!(
                matches!(error, Some(StoreError::InvalidUrl(_))),
                "{url}: {error:?}"
            );
            assert_eq!(
                error.map(|error| error.to_string()).as_deref(),
                Some("DATABASE_URL is not a valid Postgres address"),
                "{url}"
            );
        }

        let without_password = [
            "postgres://sei@db.internal/sei",
            "postgres://sei:@db.internal/sei",
            "postgres://sei@db.internal/sei?password=",
            "postgres:///sei?host=db.internal&user=sei",
            "postgres:///sei?host=db.internal&user=sei&password=",
            "postgres://db.internal/sei?sslmode=disable",
        ];
        for url in without_password {
            let error = database_options(&Secret::new(url.to_owned())).err();
            assert!(
                matches!(error, Some(StoreError::InvalidUrl(_))),
                "{url}: {error:?}"
            );
            assert_eq!(
                error.map(|error| error.to_string()).as_deref(),
                Some("DATABASE_URL has no password: add the role's password to the URL"),
                "{url}"
            );
        }

        let not_honoured = [
            ("channel_binding", "require"),
            ("channel_binding", "sometimes"),
            ("sslnegotiation", "direct"),
            ("target_session_attrs", "read-write"),
            ("target_session_attrs", "read-only"),
        ];
        for (key, value) in not_honoured {
            let error = database_options(&with_query(URL, &format!("{key}={value}"))).err();
            assert!(
                matches!(error, Some(StoreError::InvalidUrl(_))),
                "{key}={value}: {error:?}"
            );
            let text = error.map(|error| error.to_string()).unwrap_or_default();
            assert!(text.starts_with("DATABASE_URL's "), "{text}");
            assert!(text.contains(key), "{text}");
            assert!(!text.contains(value), "{text}");
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
