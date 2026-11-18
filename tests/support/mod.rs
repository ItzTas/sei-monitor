#![allow(dead_code)]

pub mod clock;
pub mod fake_sei;
pub mod harness;
pub mod logs;
pub mod postgres;
pub mod process;

pub use postgres::{Database, database};

use std::io;
use std::time::Duration;

use monitor_hub_plugin::Signals;
use monitor_hub_plugin::migrate::up;
use sei_monitor::config::{Source, load_sei_url};
use sei_monitor::store::MIGRATIONS;
use tokio_postgres::NoTls;

pub const SIGNAL_WAIT: Duration = Duration::from_secs(5);

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
        invalid_utf8: Box::new(|_| false),
    };
    load_sei_url(&source).unwrap_or_else(|error| panic!("normalize the SEI address: {error}"))
}

pub async fn migrated_database() -> Option<Database> {
    let database = database().await?;
    let (mut client, connection) = tokio_postgres::connect(&database.url, NoTls)
        .await
        .expect("connect to the test database");
    let driver = tokio::spawn(connection);
    up(&mut client, MIGRATIONS)
        .await
        .expect("migrate the test database");
    drop(client);
    driver
        .await
        .expect("join the migration connection")
        .expect("close the migration connection");
    Some(database)
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
