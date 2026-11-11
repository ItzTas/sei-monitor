mod accounts;
mod records;
mod runs;

use std::error::Error;
use std::time::Duration;

use deadpool_postgres::{Manager, Pool, PoolError, Runtime, TimeoutType};
use monitor_hub_plugin::migrate::{MigrateError, Migration, up};
use tokio::task::{JoinError, JoinHandle};
use tokio_postgres::{Client, Config, NoTls};

use crate::secret::Secret;

pub use records::{Account, AccountInfo, AccountKey, CycleRun, Trigger};

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

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POOL_SIZE: usize = 10;
const POOL_LIMITS: &str = "-c statement_timeout=20000 -c lock_timeout=5000";
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
    #[error(transparent)]
    Pool(PoolError),
    #[error(transparent)]
    Database(#[from] tokio_postgres::Error),
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    #[error("event {event_id} has an unreadable payload: {detail}")]
    Payload { event_id: i64, detail: String },
}

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
    pool: Pool,
}

impl Store {
    pub fn new(database_url: &Secret) -> Result<Store, StoreError> {
        let manager = Manager::new(pool_config(database_url)?, NoTls);
        let pool = Pool::builder(manager)
            .max_size(POOL_SIZE)
            .runtime(Runtime::Tokio1)
            .create_timeout(Some(CONNECT_TIMEOUT))
            .wait_timeout(Some(CONNECT_TIMEOUT))
            .build()
            .expect("the pool has a runtime for its timeouts");
        Ok(Store { pool })
    }

    async fn client(&self) -> Result<deadpool_postgres::Client, StoreError> {
        self.pool.get().await.map_err(pool_error)
    }
}

fn pool_config(database_url: &Secret) -> Result<Config, StoreError> {
    let mut config = lock_config(database_url)?;
    let options = after_existing(config.get_options(), POOL_LIMITS);
    config.options(options);
    Ok(config)
}

fn after_existing(existing: Option<&str>, settings: &str) -> String {
    let Some(existing) = existing.filter(|options| !options.trim().is_empty()) else {
        return settings.to_owned();
    };
    format!("{existing} {settings}")
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

fn pool_error(error: PoolError) -> StoreError {
    let PoolError::Timeout(kind) = error else {
        return StoreError::Pool(error);
    };
    StoreError::Timeout(match kind {
        TimeoutType::Wait => "waiting for a free connection",
        TimeoutType::Create => "opening a connection",
        TimeoutType::Recycle => "checking a pooled connection",
    })
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
    use std::time::Duration;

    use monitor_hub_plugin_testkit::database;
    use tokio_postgres::{Client, Config};

    use super::{Store, acquire_writer_lock, lock_config, pool_config};
    use crate::secret::Secret;

    const URL: &str = "postgres://sei:hunter2@db.internal:5432/sei";

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

        let pool = pool_config(&url).expect("pool config");
        assert_eq!(
            pool.get_options(),
            Some("-c statement_timeout=1 -c statement_timeout=20000 -c lock_timeout=5000")
        );
        assert_keepalives(&pool);

        let lock = lock_config(&url).expect("lock config");
        assert_eq!(lock.get_options(), Some("-c statement_timeout=1"));
        assert_keepalives(&lock);

        let plain = lock_config(&Secret::new(URL.to_owned())).expect("lock config");
        assert_eq!(plain.get_options(), None);
        assert_keepalives(&plain);
    }

    #[tokio::test]
    async fn pool_and_lock_connections_carry_their_limits() {
        let Some(db) = database().await else { return };
        let url = with_query(
            &db.url,
            "options=-c%20statement_timeout%3D1%20-c%20application_name%3Dsei_test",
        );

        let store = Store::new(&url).expect("store");
        let pooled = store.client().await.expect("pooled connection");
        assert_eq!(show(&pooled, "statement_timeout").await, "20s");
        assert_eq!(show(&pooled, "lock_timeout").await, "5s");
        assert_eq!(show(&pooled, "application_name").await, "sei_test");

        let lock = acquire_writer_lock(&Secret::new(db.url.clone()))
            .await
            .expect("lock");
        assert_eq!(show(&lock.client, "statement_timeout").await, "0");
        assert_eq!(show(&lock.client, "lock_timeout").await, "0");
    }
}
