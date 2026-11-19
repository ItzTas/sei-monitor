use std::str::FromStr;
use std::time::Duration;

use sea_orm::sqlx::ConnectOptions as _;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sea_orm::{DatabaseConnection, SqlxPostgresConnector};

use super::StoreError;
use crate::secret::Secret;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const POOL_SIZE: u32 = 10;

const SCHEMES: [&str; 2] = ["postgres://", "postgresql://"];

// sqlx has no client-side keepalive, so the server is asked to probe the connection instead,
// with the values 2.1 set on the client.
const KEEPALIVES: [(&str, &str); 4] = [
    ("tcp_keepalives_idle", "30"),
    ("tcp_keepalives_interval", "10"),
    ("tcp_keepalives_count", "3"),
    ("tcp_user_timeout", "60000"),
];

const POOL_LIMITS: [(&str, &str); 2] = [("statement_timeout", "20000"), ("lock_timeout", "5000")];

pub(crate) fn database_options(database_url: &Secret) -> Result<PgConnectOptions, StoreError> {
    let url = database_url.reveal();
    if !SCHEMES.iter().any(|scheme| url.starts_with(scheme)) {
        return Err(StoreError::InvalidUrl);
    }
    let options = PgConnectOptions::from_str(url).map_err(|_| StoreError::InvalidUrl)?;
    if options.get_host().contains(',') {
        return Err(StoreError::InvalidUrl);
    }
    Ok(options.disable_statement_logging().options(KEEPALIVES))
}

pub(crate) fn with_pool_limits(options: PgConnectOptions) -> PgConnectOptions {
    options.options(POOL_LIMITS)
}

// Without idle or lifetime limits and with no minimum, sqlx starts no maintenance task: the pool
// is built outside a runtime and connects only when a query needs it.
pub(crate) fn pool(options: PgConnectOptions) -> DatabaseConnection {
    let pool = PgPoolOptions::new()
        .max_connections(POOL_SIZE)
        .min_connections(0)
        .acquire_timeout(CONNECT_TIMEOUT)
        .idle_timeout(None::<Duration>)
        .max_lifetime(None::<Duration>)
        .test_before_acquire(true)
        .connect_lazy_with(options);
    SqlxPostgresConnector::from_sqlx_postgres_pool(pool)
}
