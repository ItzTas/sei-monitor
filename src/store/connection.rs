use std::time::Duration;

// The url crate's type, the one sqlx parses addresses with.
use reqwest::Url;
use sea_orm::sqlx::ConnectOptions as _;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sea_orm::{DatabaseConnection, SqlxPostgresConnector};

use super::StoreError;
use crate::secret::Secret;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const POOL_SIZE: u32 = 10;

const NOT_A_POSTGRES_ADDRESS: &str = "DATABASE_URL is not a valid Postgres address";
const NO_PASSWORD: &str = "DATABASE_URL has no password: add the role's password to the URL";

const SCHEMES: [&str; 2] = ["postgres://", "postgresql://"];

// sqlx has no client-side keepalive, so the server is asked to probe the connection instead.
const KEEPALIVES: [(&str, &str); 4] = [
    ("tcp_keepalives_idle", "30"),
    ("tcp_keepalives_interval", "10"),
    ("tcp_keepalives_count", "3"),
    ("tcp_user_timeout", "60000"),
];

const POOL_LIMITS: [(&str, &str); 2] = [("statement_timeout", "20000"), ("lock_timeout", "5000")];

pub(crate) fn database_options(database_url: &Secret) -> Result<PgConnectOptions, StoreError> {
    let address = database_url.reveal();
    if !SCHEMES.iter().any(|scheme| address.starts_with(scheme)) {
        return Err(StoreError::InvalidUrl(NOT_A_POSTGRES_ADDRESS));
    }
    let url = Url::parse(address).map_err(|_| StoreError::InvalidUrl(NOT_A_POSTGRES_ADDRESS))?;
    if !names_a_host(&url) {
        return Err(StoreError::InvalidUrl(NOT_A_POSTGRES_ADDRESS));
    }
    if !names_a_password(&url) {
        return Err(StoreError::InvalidUrl(NO_PASSWORD));
    }
    let mut options = PgConnectOptions::from_url(&keys_sqlx_reads(url)?)
        .map_err(|_| StoreError::InvalidUrl(NOT_A_POSTGRES_ADDRESS))?;
    if options.get_host().contains(',') {
        return Err(StoreError::InvalidUrl(NOT_A_POSTGRES_ADDRESS));
    }
    // sqlx keeps an IPv6 host's brackets, and the resolver then takes it for a name.
    let unbracketed = options
        .get_host()
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .map(str::to_owned);
    if let Some(address) = unbracketed {
        options = options.host(&address);
    }
    Ok(options.disable_statement_logging().options(KEEPALIVES))
}

// Without a host in the address, sqlx would fall back to PGHOST, a socket or localhost.
fn names_a_host(url: &Url) -> bool {
    url.has_host() || in_query(url, &["host", "hostaddr"])
}

// sqlx reads PGPASSWORD or a pgpass file, and logs a malformed pgpass line whole, only when the
// address has no password. An empty one counts as none.
fn names_a_password(url: &Url) -> bool {
    url.password().is_some() || in_query(url, &["password"])
}

fn in_query(url: &Url, keys: &[&str]) -> bool {
    url.query_pairs()
        .any(|(key, value)| keys.contains(&key.as_ref()) && !value.is_empty())
}

// sqlx logs every query key it does not read, value included, so it only gets the keys it reads.
fn keys_sqlx_reads(mut url: Url) -> Result<Url, StoreError> {
    let read = [
        "sslmode",
        "ssl-mode",
        "sslrootcert",
        "ssl-root-cert",
        "ssl-ca",
        "sslcert",
        "ssl-cert",
        "sslkey",
        "ssl-key",
        "statement-cache-capacity",
        "host",
        "hostaddr",
        "port",
        "dbname",
        "user",
        "password",
        "application_name",
        "options",
    ];
    // 2.1's client took these too; sqlx has no equivalent, so they are left out.
    let left_out = [
        "sslnegotiation",
        "connect_timeout",
        "tcp_user_timeout",
        "keepalives",
        "keepalives_idle",
        "keepalives_interval",
        "keepalives_retries",
        "target_session_attrs",
        "channel_binding",
        "load_balance_hosts",
    ];
    let read_by_sqlx =
        |key: &str| read.contains(&key) || (key.starts_with("options[") && key.ends_with(']'));
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let unknown = pairs
        .iter()
        .any(|(key, _)| !read_by_sqlx(key) && !left_out.contains(&key.as_str()));
    if unknown {
        return Err(StoreError::InvalidUrl(NOT_A_POSTGRES_ADDRESS));
    }
    // sqlx cannot honour other values of these; left out, they would connect without what was
    // asked for.
    let honoured_only_as = [
        (
            "channel_binding",
            &["disable", "prefer"][..],
            "DATABASE_URL's channel_binding needs TLS, and the monitor connects without TLS",
        ),
        (
            "sslnegotiation",
            &["postgres"][..],
            "DATABASE_URL's sslnegotiation needs TLS, and the monitor connects without TLS",
        ),
        (
            "target_session_attrs",
            &["any"][..],
            "DATABASE_URL's target_session_attrs needs a session check the monitor does not run",
        ),
    ];
    let not_honoured = pairs.iter().find_map(|(key, value)| {
        honoured_only_as
            .iter()
            .find(|(name, values, _)| name == key && !values.contains(&value.as_str()))
            .map(|(_, _, refusal)| *refusal)
    });
    if let Some(refusal) = not_honoured {
        return Err(StoreError::InvalidUrl(refusal));
    }
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(pairs.iter().filter(|(key, _)| read_by_sqlx(key)));
    Ok(url)
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
