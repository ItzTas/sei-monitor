#![allow(dead_code)]

pub mod clock;
pub mod fake_sei;
pub mod go;
pub mod harness;
pub mod logs;
pub mod process;
pub mod webhook;

use std::io;

use monitor_hub_plugin::migrate::up;
use monitor_hub_plugin_testkit::{Database, database};
use sei_monitor::config::{Source, load_sei_url};
use sei_monitor::store::MIGRATIONS;
use tokio_postgres::NoTls;

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
