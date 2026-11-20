mod support;

use futures_util::future::join_all;
use sea_orm::{ConnectionTrait, DbErr};
use sea_orm_migration::MigratorTrait;
use sei_monitor::migration::{DROP_GO_TABLES, GOOSE_SCHEMA, Migrator};
use tokio_postgres::{Client, NoTls};

use support::schema::{
    REJECTED, WHATSMEOW_TABLES, goose_era_database, rows, schema_snapshot, seed_rows, table_names,
    up_sections, whatsmeow_tables,
};
use support::{database, orm};

const TODAYS_TABLES: [&str; 6] = [
    "accounts",
    "cycle_runs",
    "documents",
    "events",
    "processes",
    "seaql_migrations",
];

const KEPT_TABLES: [&str; 5] = ["accounts", "processes", "documents", "events", "cycle_runs"];

async fn admin(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("connect to the test database");
    tokio::spawn(connection);
    client
}

async fn migrate(url: &str) -> Result<(), DbErr> {
    Migrator::up(&orm(url).await, None).await
}

async fn recorded_migrations(client: &Client) -> Vec<String> {
    client
        .query("SELECT version FROM seaql_migrations ORDER BY version", &[])
        .await
        .expect("read seaql_migrations")
        .iter()
        .map(|row| row.get(0))
        .collect()
}

async fn events_sequence(client: &Client) -> (i64, bool) {
    let row = client
        .query_one("SELECT last_value, is_called FROM events_id_seq", &[])
        .await
        .expect("read events_id_seq");
    (row.get(0), row.get(1))
}

async fn kept_rows(client: &Client, tables: &[&str]) -> (Vec<Vec<String>>, (i64, bool)) {
    tokio::join!(
        join_all(tables.iter().map(|table| rows(client, table))),
        events_sequence(client)
    )
}

async fn events(client: &Client) -> Vec<(i64, String, String)> {
    client
        .query("SELECT id, kind, ref FROM events ORDER BY id", &[])
        .await
        .expect("read the events")
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect()
}

async fn exists(client: &Client, relation: &str) -> bool {
    client
        .query_one("SELECT to_regclass($1::text) IS NOT NULL", &[&relation])
        .await
        .expect("look the relation up")
        .get(0)
}

async fn goose_versions(client: &Client) -> Vec<i64> {
    client
        .query("SELECT version_id FROM goose_db_version ORDER BY id", &[])
        .await
        .expect("read goose_db_version")
        .iter()
        .map(|row| row.get(0))
        .collect()
}

fn with_seaql_migrations(mut tables: Vec<String>) -> Vec<String> {
    tables.push("seaql_migrations".to_owned());
    tables.sort();
    tables
}

#[tokio::test]
async fn a_new_database_gets_todays_tables_and_no_go_tables() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;

    Migrator::up(&orm(&db.url).await, None)
        .await
        .expect("migrate a new database");

    assert_eq!(table_names(&client).await, TODAYS_TABLES);
    assert_eq!(
        recorded_migrations(&client).await,
        [GOOSE_SCHEMA, DROP_GO_TABLES]
    );
}

#[tokio::test]
async fn the_first_migration_builds_what_the_goose_files_build() {
    let (Some(migrated), Some(by_hand)) = (database().await, database().await) else {
        return;
    };
    let (migrated_client, by_hand_client) = tokio::join!(admin(&migrated.url), admin(&by_hand.url));

    Migrator::up(&orm(&migrated.url).await, Some(1))
        .await
        .expect("run the first migration");
    up_sections(&by_hand_client, &[1, 2]).await;

    assert_eq!(recorded_migrations(&migrated_client).await, [GOOSE_SCHEMA]);
    let migrated_schema = schema_snapshot(&migrated_client, &["seaql_migrations"]).await;
    assert!(
        migrated_schema
            .iter()
            .any(|line| line.starts_with("column deliveries.")),
        "{migrated_schema:#?}"
    );
    assert_eq!(migrated_schema, schema_snapshot(&by_hand_client, &[]).await);
}

#[tokio::test]
async fn a_database_at_goose_version_2_is_adopted_without_loss() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[1, 2]).await;
    seed_rows(&client, true).await;
    let before = kept_rows(&client, &KEPT_TABLES).await;
    assert!(before.0.iter().all(|rows| !rows.is_empty()), "{before:#?}");

    migrate(&db.url).await.expect("adopt the goose schema");

    assert_eq!(kept_rows(&client, &KEPT_TABLES).await, before);
    assert_eq!(table_names(&client).await, TODAYS_TABLES);
    assert_eq!(
        recorded_migrations(&client).await,
        [GOOSE_SCHEMA, DROP_GO_TABLES]
    );
}

#[tokio::test]
async fn the_adoption_keeps_the_pause_reason_and_every_event() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[1, 2]).await;
    seed_rows(&client, true).await;
    let before = events(&client).await;
    assert_eq!(before.len(), 2, "{before:?}");

    migrate(&db.url).await.expect("adopt the goose schema");

    let paused: Vec<String> = client
        .query("SELECT paused_reason FROM accounts", &[])
        .await
        .expect("read the pause reason")
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(paused, [REJECTED]);
    assert_eq!(events(&client).await, before);
}

#[tokio::test]
async fn a_database_at_goose_version_1_is_caught_up_and_ends_like_a_new_one() {
    let (Some(db), Some(new)) = (database().await, database().await) else {
        return;
    };
    let (client, new_client) = tokio::join!(admin(&db.url), admin(&new.url));
    goose_era_database(&client, &[1], &[1]).await;
    seed_rows(&client, false).await;
    let before = kept_rows(&client, &["processes", "documents", "events"]).await;
    let account_before = rows(&client, "accounts").await;

    migrate(&db.url).await.expect("catch up and adopt");
    migrate(&new.url).await.expect("migrate a new database");

    let account = client
        .query_one(
            "SELECT org_acronym, sei_version, wssei_version, paused_reason FROM accounts",
            &[],
        )
        .await
        .expect("read the account");
    let account: (String, String, String, String) = (
        account.get(0),
        account.get(1),
        account.get(2),
        account.get(3),
    );
    assert_eq!(
        account,
        (
            String::new(),
            String::new(),
            String::new(),
            REJECTED.to_owned()
        )
    );
    let account_kept_columns = client
        .query(
            "SELECT row_to_json(t)::text FROM (SELECT id, sei_base_url, username, org_id, created_at,
             baseline_at, paused_reason FROM accounts) t ORDER BY 1",
            &[],
        )
        .await
        .expect("read the account's version 1 columns")
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect::<Vec<_>>();
    assert_eq!(account_kept_columns, account_before);
    assert_eq!(
        kept_rows(&client, &["processes", "documents", "events"]).await,
        before
    );
    assert_eq!(table_names(&client).await, TODAYS_TABLES);
    assert_eq!(
        schema_snapshot(&client, &[]).await,
        schema_snapshot(&new_client, &[]).await
    );
}

#[tokio::test]
async fn a_goose_version_above_2_is_adopted() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[1, 2, 3]).await;
    seed_rows(&client, true).await;
    let before = rows(&client, "events").await;

    migrate(&db.url)
        .await
        .expect("adopt a database goose took past version 2");

    assert_eq!(rows(&client, "events").await, before);
    assert_eq!(
        recorded_migrations(&client).await,
        [GOOSE_SCHEMA, DROP_GO_TABLES]
    );
}

#[tokio::test]
async fn a_gap_in_goose_db_version_is_refused_and_nothing_is_dropped() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[2]).await;
    seed_rows(&client, true).await;
    whatsmeow_tables(&client).await;
    let tables = table_names(&client).await;
    let before = kept_rows(&client, &KEPT_TABLES).await;

    let error = migrate(&db.url).await.expect_err("a gap was adopted");

    assert!(
        error.to_string().contains(
            "migration version 1 is not applied but the database is already at version 2"
        ),
        "{error}"
    );
    assert_eq!(table_names(&client).await, with_seaql_migrations(tables));
    assert_eq!(kept_rows(&client, &KEPT_TABLES).await, before);
    assert_eq!(recorded_migrations(&client).await, Vec::<String>::new());
}

#[tokio::test]
async fn an_empty_goose_db_version_is_refused_and_nothing_is_dropped() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[]).await;
    client
        .batch_execute("DELETE FROM goose_db_version")
        .await
        .expect("empty goose_db_version");
    seed_rows(&client, true).await;
    whatsmeow_tables(&client).await;
    let tables = table_names(&client).await;
    let before = kept_rows(&client, &KEPT_TABLES).await;

    let error = migrate(&db.url)
        .await
        .expect_err("an empty goose_db_version was adopted");

    assert!(
        error
            .to_string()
            .contains("goose_db_version has no rows, not even the version 0 row goose expects"),
        "{error}"
    );
    assert_eq!(table_names(&client).await, with_seaql_migrations(tables));
    assert_eq!(kept_rows(&client, &KEPT_TABLES).await, before);
    assert_eq!(recorded_migrations(&client).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_failed_catch_up_applies_nothing_and_drops_nothing() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1], &[1]).await;
    seed_rows(&client, false).await;
    client
        .batch_execute("CREATE TABLE cycle_runs (id BIGINT)")
        .await
        .expect("create a cycle_runs table by hand");
    let before = kept_rows(&client, &["accounts", "processes", "documents", "events"]).await;

    let error = migrate(&db.url)
        .await
        .expect_err("a catch-up over an existing cycle_runs worked");

    assert!(error.to_string().contains("already exists"), "{error}");
    let org_acronym: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = 'accounts'
               AND column_name = 'org_acronym')",
            &[],
        )
        .await
        .expect("look the column up")
        .get(0);
    assert!(
        !org_acronym,
        "the part of version 2 before the error stayed"
    );
    assert_eq!(goose_versions(&client).await, [0, 1]);
    assert!(exists(&client, "deliveries").await);
    assert_eq!(
        kept_rows(&client, &["accounts", "processes", "documents", "events"]).await,
        before
    );
    assert_eq!(recorded_migrations(&client).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_catch_up_that_fails_at_version_2_keeps_none_of_version_1() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[], &[]).await;
    client
        .batch_execute("CREATE TABLE cycle_runs (id BIGINT)")
        .await
        .expect("create a cycle_runs table by hand");

    let error = migrate(&db.url)
        .await
        .expect_err("a catch-up over an existing cycle_runs worked");

    assert!(error.to_string().contains("already exists"), "{error}");
    assert!(
        !exists(&client, "accounts").await,
        "version 1 stayed after version 2 failed"
    );
    assert_eq!(goose_versions(&client).await, [0]);
    assert_eq!(recorded_migrations(&client).await, Vec::<String>::new());
}

#[tokio::test]
async fn an_object_that_depends_on_a_go_table_stops_the_drop() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[1, 2]).await;
    seed_rows(&client, true).await;
    client
        .batch_execute("CREATE VIEW sent AS SELECT * FROM deliveries")
        .await
        .expect("create a view over deliveries");
    let accounts = rows(&client, "accounts").await;
    let deliveries = rows(&client, "deliveries").await;

    let error = migrate(&db.url)
        .await
        .expect_err("the drop went through a dependent view");

    assert!(
        error
            .to_string()
            .contains("cannot drop table deliveries because other objects depend on it"),
        "{error}"
    );
    assert_eq!(recorded_migrations(&client).await, [GOOSE_SCHEMA]);
    for relation in ["deliveries", "goose_db_version", "sent"] {
        assert!(exists(&client, relation).await, "{relation} is gone");
    }
    assert_eq!(rows(&client, "accounts").await, accounts);
    assert_eq!(rows(&client, "deliveries").await, deliveries);
}

#[tokio::test]
async fn a_whatsmeow_table_with_a_dependent_view_is_named_and_nothing_is_dropped() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[1, 2]).await;
    whatsmeow_tables(&client).await;
    client
        .batch_execute("CREATE VIEW paired AS SELECT jid FROM whatsmeow_device")
        .await
        .expect("create a view over whatsmeow_device");

    let error = migrate(&db.url)
        .await
        .expect_err("the drop went through a dependent view");

    assert!(
        error
            .to_string()
            .contains("view paired depends on table whatsmeow_device"),
        "{error}"
    );
    assert_eq!(recorded_migrations(&client).await, [GOOSE_SCHEMA]);
    for table in WHATSMEOW_TABLES
        .into_iter()
        .chain(["deliveries", "goose_db_version"])
    {
        assert!(exists(&client, table).await, "{table} is gone");
    }
}

#[tokio::test]
async fn every_whatsmeow_table_goes_and_nothing_else() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[1, 2]).await;
    seed_rows(&client, true).await;
    whatsmeow_tables(&client).await;
    client
        .batch_execute(r#"CREATE TABLE "whatsmeow_Odd""Name" (id INTEGER)"#)
        .await
        .expect("create a whatsmeow table whose name needs quoting");
    let tables = table_names(&client).await;
    for table in WHATSMEOW_TABLES
        .into_iter()
        .chain([r#"whatsmeow_Odd"Name"#])
    {
        assert!(tables.iter().any(|name| name == table), "{table} missing");
    }
    let events = rows(&client, "events").await;
    let backup = rows(&client, "whatsmeowbackup").await;
    assert_eq!(backup.len(), 1);

    migrate(&db.url).await.expect("drop the Go-era tables");

    let tables = table_names(&client).await;
    assert!(
        !tables.iter().any(|name| name.starts_with("whatsmeow_")),
        "{tables:?}"
    );
    let mut expected: Vec<&str> = TODAYS_TABLES.to_vec();
    expected.push("whatsmeowbackup");
    assert_eq!(tables, expected);
    assert_eq!(rows(&client, "whatsmeowbackup").await, backup);
    assert_eq!(rows(&client, "events").await, events);
}

#[tokio::test]
async fn the_whatsmeow_prefix_holds_without_standard_conforming_strings() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    goose_era_database(&client, &[1, 2], &[1, 2]).await;
    whatsmeow_tables(&client).await;
    let backup = rows(&client, "whatsmeowbackup").await;
    client
        .batch_execute(
            "DO $$ BEGIN EXECUTE format('ALTER DATABASE %I SET standard_conforming_strings = off',
             current_database()); END $$",
        )
        .await
        .expect("turn standard_conforming_strings off for new connections");

    let orm = orm(&db.url).await;
    let setting = orm
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SHOW standard_conforming_strings",
        ))
        .await
        .expect("show the setting")
        .expect("a row")
        .try_get_by_index::<String>(0)
        .expect("the setting");
    assert_eq!(setting, "off");
    Migrator::up(&orm, None)
        .await
        .expect("drop the Go-era tables");

    let tables = table_names(&client).await;
    assert!(
        !tables.iter().any(|name| name.starts_with("whatsmeow_")),
        "{tables:?}"
    );
    assert_eq!(rows(&client, "whatsmeowbackup").await, backup);
}

#[tokio::test]
async fn running_the_migrations_again_changes_nothing() {
    let Some(db) = database().await else { return };
    let client = admin(&db.url).await;
    let orm = orm(&db.url).await;
    let applied = "SELECT version, applied_at FROM seaql_migrations ORDER BY version";

    Migrator::up(&orm, None).await.expect("migrate");
    let first: Vec<(String, i64)> = client
        .query(applied, &[])
        .await
        .expect("read seaql_migrations")
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    Migrator::up(&orm, None).await.expect("migrate again");

    let pending = Migrator::get_pending_migrations(&orm)
        .await
        .expect("pending migrations");
    assert!(pending.is_empty(), "{} pending", pending.len());
    let again: Vec<(String, i64)> = client
        .query(applied, &[])
        .await
        .expect("read seaql_migrations")
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(again, first);
    assert_eq!(table_names(&client).await, TODAYS_TABLES);
}

#[tokio::test]
async fn go_tables_in_another_schema_on_the_search_path_are_left_alone() {
    let Some(db) = database().await else { return };
    let setup = admin(&db.url).await;
    goose_era_database(&setup, &[], &[1, 2]).await;
    setup
        .batch_execute(
            "CREATE TABLE deliveries (event_id BIGINT, channel TEXT);
             INSERT INTO deliveries VALUES (7, 'email');
             CREATE TABLE whatsmeow_device (jid TEXT PRIMARY KEY);
             INSERT INTO whatsmeow_device VALUES ('another-application');
             CREATE SCHEMA sei;
             DO $$ BEGIN EXECUTE format('ALTER DATABASE %I SET search_path = sei, public',
             current_database()); END $$",
        )
        .await
        .expect("set up another application's tables in public");
    let public = ["deliveries", "goose_db_version", "whatsmeow_device"]
        .map(|table| format!("public.{table}"));
    let before = join_all(public.iter().map(|table| rows(&setup, table))).await;
    assert!(before.iter().all(|rows| !rows.is_empty()), "{before:?}");

    migrate(&db.url).await.expect("migrate");

    let client = admin(&db.url).await;
    let schema: String = client
        .query_one("SELECT current_schema()::text", &[])
        .await
        .expect("read the current schema")
        .get(0);
    assert_eq!(schema, "sei");
    assert_eq!(table_names(&client).await, TODAYS_TABLES);
    assert_eq!(
        join_all(public.iter().map(|table| rows(&client, table))).await,
        before
    );
}
