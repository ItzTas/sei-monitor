#![allow(dead_code)]

pub mod clock;
pub mod fake_sei;
pub mod harness;
pub mod logs;
pub mod postgres;
pub mod process;
pub mod proxy;
pub mod schema;

pub use postgres::{Database, database};

use std::io;
use std::time::Duration;

use monitor_hub_sdk::Signals;
use sei_monitor::config::{Source, load_sei_url};
use sei_monitor::secret::Secret;
use sei_monitor::store::{WRITER_LOCK_ID, acquire_writer_lock};
use tokio::time::{Instant, sleep};
use tokio_postgres::{Client, NoTls};

pub const SIGNAL_WAIT: Duration = Duration::from_secs(5);
const BACKEND_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);
const WRITER_LOCK_HOLDERS: &str = "SELECT pid FROM pg_locks
WHERE locktype = 'advisory'
  AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
  AND objsubid = 1
  AND ((classid::bigint << 32) | objid::bigint) = $1";
const BACKEND_ALIVE: &str = "SELECT count(*) FROM pg_stat_activity WHERE pid = $1";

pub async fn next_signal(signals: &mut Signals) -> i64 {
    tokio::time::timeout(SIGNAL_WAIT, signals.changed())
        .await
        .expect("a signal within 5 seconds")
}

pub async fn pending_signal(signals: &mut Signals) -> Option<i64> {
    tokio::time::timeout(Duration::ZERO, signals.changed())
        .await
        .ok()
}

pub fn ci() -> bool {
    std::env::var_os("CI").is_some_and(|value| !value.is_empty())
}

pub fn normalized_sei_url(raw: &str) -> String {
    let raw = raw.to_owned();
    let source = Source {
        lookup: Box::new(move |name| (name == "SEI_URL").then(|| raw.clone())),
        read_file: Box::new(|_| Err(io::ErrorKind::NotFound.into())),
    };
    load_sei_url(&source).unwrap_or_else(|error| panic!("normalize the SEI address: {error}"))
}

pub async fn migrated_database() -> Option<Database> {
    let database = database().await?;
    let lock = acquire_writer_lock(&Secret::new(database.url.clone()))
        .await
        .expect("take the test database's writer lock");
    lock.migrate().await.expect("migrate the test database");
    // The pool closes the lock's connection in the background.
    drop(lock);
    let admin = connect_admin(&database.url).await;
    let deadline = Instant::now() + BACKEND_WAIT;
    while !writer_lock_holders(&admin).await.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the migration's writer lock was still held after 5 seconds"
        );
        sleep(POLL).await;
    }
    Some(database)
}

pub async fn connect_admin(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("connect the admin client");
    tokio::spawn(connection);
    client
}

pub async fn writer_lock_holders(client: &Client) -> Vec<i32> {
    client
        .query(WRITER_LOCK_HOLDERS, &[&WRITER_LOCK_ID])
        .await
        .expect("list the writer lock's holders")
        .iter()
        .map(|row| row.get(0))
        .collect()
}

pub async fn end_the_writer_locks_connection(client: &Client) {
    let [holder] = writer_lock_holders(client).await[..] else {
        panic!("the writer lock has no single holder");
    };
    let ended: bool = client
        .query_one("SELECT pg_terminate_backend($1)", &[&holder])
        .await
        .expect("end the writer lock's backend")
        .get(0);
    assert!(ended, "the writer lock's backend was not ended");
    let deadline = Instant::now() + BACKEND_WAIT;
    loop {
        let alive: i64 = client
            .query_one(BACKEND_ALIVE, &[&holder])
            .await
            .expect("look for the writer lock's backend")
            .get(0);
        if alive == 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the writer lock's backend was still running after 5 seconds"
        );
        sleep(POLL).await;
    }
}

pub async fn orm(url: &str) -> sea_orm::DatabaseConnection {
    let mut options = sea_orm::ConnectOptions::new(url);
    options.sqlx_logging(false);
    sea_orm::Database::connect(options)
        .await
        .expect("connect to the test database with SeaORM")
}

pub fn login_role(role: &str, password: &str) -> String {
    format!(
        "DO $$ BEGIN CREATE ROLE {role} LOGIN PASSWORD '{password}';
         EXCEPTION WHEN duplicate_object OR unique_violation THEN
         ALTER ROLE {role} LOGIN PASSWORD '{password}'; END $$;"
    )
}

pub fn database_url_as(url: &str, role: &str, password: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("a postgres:// URL");
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let host_start = rest[..authority_end].rfind('@').map_or(0, |at| at + 1);
    format!("{scheme}://{role}:{password}@{}", &rest[host_start..])
}
