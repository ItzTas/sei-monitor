#![allow(dead_code)]

pub mod fake_sei;

use monitor_hub_plugin::migrate::up;
use monitor_hub_plugin_testkit::{Database, database};
use sei_monitor::store::MIGRATIONS;
use tokio_postgres::NoTls;

pub fn ci() -> bool {
    std::env::var_os("CI").is_some_and(|value| !value.is_empty())
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

pub fn database_url_as(url: &str, role: &str, password: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("a postgres:// URL");
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let host_start = rest[..authority_end].rfind('@').map_or(0, |at| at + 1);
    format!("{scheme}://{role}:{password}@{}", &rest[host_start..])
}
