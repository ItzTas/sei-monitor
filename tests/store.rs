mod support;

use std::collections::HashSet;
use std::error::Error;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use sei_monitor::event::{Event, Kind, Payload};
use sei_monitor::secret::Secret;
use sei_monitor::store::{
    Account, AccountInfo, AccountKey, CycleChanges, CycleRun, DocumentRow, ProcessRow, Pruned,
    Store, StoreError, Trigger, WRITER_LOCK_ID, WriterLock, acquire_writer_lock,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};

use support::logs::{LoggedEvent, captured, logged};
use support::{database, database_url_as, migrated_database, next_signal, pending_signal};

const PASSWORD: &str = "hunter2";
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const SLACK: Duration = Duration::from_secs(3);
const READ_LIMIT: Duration = Duration::from_secs(10);

fn repo_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-04T18:00:00Z")
        .expect("timestamp")
        .to_utc()
}

fn key() -> AccountKey {
    AccountKey {
        sei_base_url: "https://sei.rr.gov.br".to_owned(),
        username: "fulano.tal".to_owned(),
        org_id: 63,
    }
}

fn secret(url: &str) -> Secret {
    Secret::new(url.to_owned())
}

fn store(url: &str) -> Store {
    Store::new(&secret(url)).expect("store")
}

async fn connect(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls).await.expect("connect");
    tokio::spawn(connection);
    client
}

async fn acquire_within(url: &str, limit: Duration) -> WriterLock {
    let started = Instant::now();
    loop {
        match acquire_writer_lock(&secret(url)).await {
            Ok(lock) => return lock,
            Err(StoreError::Locked) if started.elapsed() < limit => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(err) => panic!("acquire the writer lock: {err}"),
        }
    }
}

fn everything_printed(error: &(dyn Error + 'static)) -> String {
    let mut printed = format!("{error:?}\n{error:#?}");
    let mut current = Some(error);
    while let Some(error) = current {
        printed.push('\n');
        printed.push_str(&error.to_string());
        current = error.source();
    }
    printed
}

fn split_url(url: &str) -> (&str, &str, &str) {
    let (address, query) = url.split_once('?').unwrap_or((url, ""));
    let (scheme, rest) = address.split_once("://").expect("a postgres:// URL");
    let authority = rest.split('/').next().unwrap_or(rest);
    (scheme, authority, query)
}

fn host_of(url: &str) -> &str {
    let (_, authority, query) = split_url(url);
    let socket = query.split('&').find_map(|pair| pair.strip_prefix("host="));
    let host_and_port = authority.rsplit('@').next().unwrap_or(authority);
    socket.unwrap_or_else(|| host_and_port.split(':').next().unwrap_or(host_and_port))
}

fn with_database(url: &str, name: &str) -> String {
    let (scheme, authority, query) = split_url(url);
    format!("{scheme}://{authority}/{name}?{query}")
}

fn with_query(url: &str, parameter: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{parameter}")
}

fn assert_hides_the_login(error: &StoreError, url: &str) {
    let printed = everything_printed(error);
    let host = host_of(url);
    assert!(!printed.contains(PASSWORD), "{printed}");
    assert!(!printed.contains(host), "{host} in {printed}");
    assert!(!printed.contains(url), "{printed}");
}

#[tokio::test]
async fn migrations_apply_once_on_an_empty_database() {
    let Some(db) = database().await else { return };
    let mut lock = acquire_writer_lock(&secret(&db.url)).await.expect("lock");

    assert_eq!(lock.migrate().await.expect("migrate"), vec![1, 2]);
    assert_eq!(
        lock.migrate().await.expect("migrate again"),
        Vec::<i64>::new()
    );

    let client = connect(&db.url).await;
    let versions: Vec<(i64, bool, i64)> = client
        .query(
            "SELECT version_id, bool_and(is_applied), count(*) FROM goose_db_version GROUP BY version_id ORDER BY version_id",
            &[],
        )
        .await
        .expect("versions")
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect();
    assert_eq!(versions, vec![(0, true, 1), (1, true, 1), (2, true, 1)]);
}

#[tokio::test]
async fn a_database_migrated_by_goose_is_taken_as_is() {
    let Some(db) = database().await else { return };
    let client = connect(&db.url).await;
    let schema = [
        include_str!("../sql/schema/00001_initial.sql"),
        include_str!("../sql/schema/00002_cycle_runs.sql"),
    ];
    for migration in schema {
        let (up, _) = migration
            .split_once("-- +goose Down")
            .expect("a down section");
        client.batch_execute(up).await.expect("apply the schema");
    }
    client
        .batch_execute(
            "CREATE TABLE goose_db_version (
                id integer PRIMARY KEY GENERATED BY DEFAULT AS IDENTITY,
                version_id bigint NOT NULL,
                is_applied boolean NOT NULL,
                tstamp timestamp NOT NULL DEFAULT now()
            );
            INSERT INTO goose_db_version (version_id, is_applied) VALUES (0, true), (1, true), (2, true)",
        )
        .await
        .expect("record the versions goose applied");

    let mut lock = acquire_writer_lock(&secret(&db.url)).await.expect("lock");
    assert_eq!(lock.migrate().await.expect("migrate"), Vec::<i64>::new());

    let account = store(&db.url)
        .ensure_account(&key(), repo_now())
        .await
        .expect("an account on the goose schema");
    assert_eq!(account.key, key());
}

#[tokio::test]
async fn opening_again_keeps_the_schema_and_the_data() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let first = store(&db.url)
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");

    let mut lock = acquire_writer_lock(&secret(&db.url)).await.expect("lock");
    assert_eq!(lock.migrate().await.expect("migrate"), Vec::<i64>::new());
    let again = store(&db.url)
        .ensure_account(&key(), repo_now() + TimeDelta::hours(1))
        .await
        .expect("account again");

    assert_eq!(again, first);
}

#[tokio::test]
async fn an_address_that_is_not_postgres_is_refused() {
    let url = secret("mysql://u:hunter2@h/db");

    let Err(store_error) = Store::new(&url) else {
        panic!("the store accepted a MySQL address")
    };
    let Err(lock_error) = acquire_writer_lock(&url).await else {
        panic!("the lock accepted a MySQL address")
    };

    for error in [&store_error, &lock_error] {
        assert!(matches!(error, StoreError::InvalidUrl), "{error:?}");
        assert_eq!(
            error.to_string(),
            "DATABASE_URL is not a valid Postgres address"
        );
        let printed = everything_printed(error);
        assert!(!printed.contains(PASSWORD), "{printed}");
    }
}

#[tokio::test]
async fn failures_never_show_the_password_or_the_host() {
    let Some(db) = database().await else { return };
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let closed_port = listener.local_addr().expect("address").port();
    drop(listener);

    let invalid =
        format!("postgres://sei:{PASSWORD}@db.internal.example:5432/sei?sslmode=sometimes");
    let Err(error) = Store::new(&secret(&invalid)) else {
        panic!("the store accepted an invalid address")
    };
    assert!(matches!(error, StoreError::InvalidUrl), "{error:?}");
    assert_hides_the_login(&error, &invalid);
    let Err(error) = acquire_writer_lock(&secret(&invalid)).await else {
        panic!("the lock accepted an invalid address")
    };
    assert!(matches!(error, StoreError::InvalidUrl), "{error:?}");
    assert_hides_the_login(&error, &invalid);

    let closed = format!("postgres://sei:{PASSWORD}@127.0.0.1:{closed_port}/sei");
    let refused = [
        database_url_as(&db.url, "intruder", PASSWORD),
        with_database(
            &database_url_as(&db.url, "postgres", PASSWORD),
            "no_such_database",
        ),
    ];
    for url in [&closed, &refused[0], &refused[1]] {
        let Err(error) = acquire_writer_lock(&secret(url)).await else {
            panic!("a lock through a failing connection")
        };
        assert!(matches!(error, StoreError::LockConnection(_)), "{error:?}");
        assert_hides_the_login(&error, url);
    }

    let error = tokio::time::timeout(
        Duration::from_secs(10) + SLACK,
        store(&closed).last_cycle_run(1),
    )
    .await
    .expect("a closed port held the call past its connect limit")
    .expect_err("a closed port answered");
    assert!(matches!(error, StoreError::Timeout(_)), "{error:?}");
    assert_hides_the_login(&error, &closed);
    for url in &refused {
        let error = store(url)
            .last_cycle_run(1)
            .await
            .expect_err("a refused connection answered");
        assert!(matches!(error, StoreError::Server(_)), "{error:?}");
        assert_hides_the_login(&error, url);
    }
}

#[tokio::test]
async fn a_url_that_requires_tls_is_refused_at_once() {
    let Some(db) = database().await else { return };
    let url = with_query(&db.url, "sslmode=require");

    let error = tokio::time::timeout(Duration::from_secs(2), store(&url).last_cycle_run(1))
        .await
        .expect("a URL that requires TLS was not refused within 2 seconds")
        .expect_err("a connection without TLS answered");

    assert!(matches!(error, StoreError::Database(_)), "{error:?}");
    assert_hides_the_login(&error, &url);
}

#[tokio::test]
async fn a_silent_server_times_out_naming_the_step() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let url = format!("postgres://sei:{PASSWORD}@{address}/sei");
    let pooled = store(&url);
    let database_url = secret(&url);

    let both = tokio::time::timeout(Duration::from_secs(10) + SLACK, async {
        tokio::join!(acquire_writer_lock(&database_url), pooled.last_cycle_run(1))
    })
    .await;
    silent.abort();
    let (lock, run) = both.expect("a silent server held a call past its connect limit");

    let Err(lock) = lock else {
        panic!("a silent server gave a lock")
    };
    let run = run.expect_err("a silent server answered");
    assert_eq!(
        lock.to_string(),
        "the database did not answer in time: taking the writer lock"
    );
    assert_eq!(
        run.to_string(),
        "the database did not answer in time: getting a connection"
    );
    for error in [&lock, &run] {
        assert!(matches!(error, StoreError::Timeout(_)), "{error:?}");
        assert_hides_the_login(error, &url);
    }
}

#[tokio::test]
async fn writer_lock_admits_one_holder() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let first = acquire_writer_lock(&secret(&db.url))
        .await
        .expect("first lock");
    let second = acquire_writer_lock(&secret(&db.url)).await;
    assert!(matches!(second, Err(StoreError::Locked)));
    assert_eq!(
        StoreError::Locked.to_string(),
        "another sei-monitor process holds the database lock"
    );

    drop(first);
    acquire_within(&db.url, Duration::from_secs(5)).await;
}

#[tokio::test]
async fn writer_lock_reports_why_its_connection_ended() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let mut lock = acquire_writer_lock(&secret(&db.url)).await.expect("lock");
    tokio::select! {
        () = lock.closed() => panic!("closed resolved while the connection was alive"),
        () = tokio::time::sleep(Duration::from_millis(50)) => {}
    }
    assert_eq!(lock.close_reason(), None);

    let admin = connect(&db.url).await;
    let terminated: Vec<bool> = admin
        .query(
            "SELECT pg_terminate_backend(pid) FROM pg_locks
             WHERE locktype = 'advisory' AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
               AND classid::bigint = ($1::bigint >> 32) AND objid::bigint = ($1::bigint & 4294967295)",
            &[&WRITER_LOCK_ID],
        )
        .await
        .expect("terminate the lock's backend")
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(terminated, vec![true]);

    tokio::time::timeout(Duration::from_secs(1), lock.closed())
        .await
        .expect("closed resolves once the backend ends");
    let reason = lock
        .close_reason()
        .expect("a reason for the closed connection");
    assert!(
        reason.contains("terminating connection due to administrator command"),
        "{reason}"
    );
    assert!(!reason.contains(host_of(&db.url)), "{reason}");
    tokio::time::timeout(Duration::from_millis(10), lock.closed())
        .await
        .expect("closed keeps resolving after the end");
}

#[tokio::test]
async fn a_held_account_row_fails_the_call_within_the_lock_timeout() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = pooled
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");
    let mut outside = connect(&db.url).await;
    let held = outside.transaction().await.expect("begin");
    held.query_one(
        "SELECT id FROM accounts WHERE id = $1 FOR UPDATE",
        &[&account.id],
    )
    .await
    .expect("hold the account row");

    let started = Instant::now();
    let error = tokio::time::timeout(
        LOCK_TIMEOUT + SLACK,
        pooled.set_paused_reason(account.id, "SEI refused the login"),
    )
    .await
    .expect("the held row stopped the call within the lock timeout")
    .expect_err("the row is held");
    assert!(started.elapsed() >= LOCK_TIMEOUT, "{:?}", started.elapsed());
    assert!(matches!(error, StoreError::Server(_)), "{error:?}");
    assert_eq!(error.sql_state(), Some("55P03"));
    assert_eq!(error.to_string(), "db error");

    let started = Instant::now();
    let last = pooled
        .last_cycle_run(account.id)
        .await
        .expect("a call that needs no row");
    assert_eq!(last, None);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );

    held.rollback().await.expect("release the row");
    pooled
        .set_paused_reason(account.id, "SEI refused the login")
        .await
        .expect("pause once released");
}

const OTHER_CLIENTS: &str = "SELECT pid FROM pg_stat_activity
WHERE datname = current_database() AND backend_type = 'client backend' AND pid <> pg_backend_pid()";

#[tokio::test]
async fn a_pooled_connection_the_server_ended_is_replaced_before_use() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    pooled
        .last_cycle_run(1)
        .await
        .expect("a read that leaves one pooled connection");
    let admin = connect(&db.url).await;
    let ended: Vec<bool> = admin
        .query(
            &format!("SELECT pg_terminate_backend(pid) FROM ({OTHER_CLIENTS}) pooled"),
            &[],
        )
        .await
        .expect("end the pooled connection's backend")
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(ended, [true]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !admin
        .query(OTHER_CLIENTS, &[])
        .await
        .expect("look for the ended backend")
        .is_empty()
    {
        assert!(
            Instant::now() < deadline,
            "the ended backend was still running after 5 seconds"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let run = pooled.last_cycle_run(1).await;

    assert!(run.is_ok(), "{run:?}");
}

#[tokio::test]
async fn ensure_account_returns_the_same_account_per_key() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let first = pooled
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");

    let again = pooled
        .ensure_account(&key(), repo_now() + TimeDelta::hours(1))
        .await
        .expect("account again");
    let other = pooled
        .ensure_account(
            &AccountKey {
                org_id: 64,
                ..key()
            },
            repo_now(),
        )
        .await
        .expect("another agency");

    assert_eq!(
        again,
        Account {
            id: first.id,
            key: key(),
            created_at: repo_now(),
            baseline_at: None,
            paused_reason: String::new(),
            info: AccountInfo::default(),
        }
    );
    assert_ne!(other.id, first.id);
    assert_eq!(other.key.org_id, 64);
}

#[tokio::test]
async fn account_details_are_stored() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = pooled
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");
    let info = AccountInfo {
        org_acronym: "SEDEMO".to_owned(),
        sei_version: "5.0.5".to_owned(),
        wssei_version: "3.0.4".to_owned(),
    };

    pooled
        .update_account_info(account.id, &info)
        .await
        .expect("update the details");
    pooled
        .set_paused_reason(account.id, "SEI refused the login")
        .await
        .expect("pause");

    let stored = pooled
        .ensure_account(&key(), repo_now())
        .await
        .expect("account again");
    assert_eq!(
        stored,
        Account {
            info,
            paused_reason: "SEI refused the login".to_owned(),
            ..account.clone()
        }
    );
    let missing = pooled.set_paused_reason(account.id + 1000, "x").await;
    assert!(
        matches!(missing, Err(StoreError::AccountNotFound)),
        "{missing:?}"
    );
    assert_eq!(
        StoreError::AccountNotFound.to_string(),
        "monitor: account not found"
    );
}

#[tokio::test]
async fn cycle_runs() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = pooled
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");
    assert_eq!(
        pooled.last_cycle_run(account.id).await.expect("no run"),
        None
    );

    let failed = pooled
        .start_cycle_run(account.id, Trigger::Scheduled, repo_now())
        .await
        .expect("start");
    pooled
        .finish_cycle_run(
            failed,
            account.id,
            repo_now() + TimeDelta::seconds(1),
            "SEI answered 500",
        )
        .await
        .expect("finish");
    assert_eq!(
        pooled.last_cycle_run(account.id).await.expect("failed run"),
        Some(CycleRun {
            id: failed,
            account_id: account.id,
            trigger: Trigger::Scheduled,
            started_at: repo_now(),
            finished_at: Some(repo_now() + TimeDelta::seconds(1)),
            error: "SEI answered 500".to_owned(),
            processes_seen: 0,
            events_created: 0,
        })
    );

    let manual = pooled
        .start_cycle_run(
            account.id,
            Trigger::Manual,
            repo_now() + TimeDelta::minutes(1),
        )
        .await
        .expect("start manual");
    assert_eq!(
        pooled.last_cycle_run(account.id).await.expect("running"),
        Some(CycleRun {
            id: manual,
            account_id: account.id,
            trigger: Trigger::Manual,
            started_at: repo_now() + TimeDelta::minutes(1),
            finished_at: None,
            error: String::new(),
            processes_seen: 0,
            events_created: 0,
        })
    );
    pooled
        .finish_cycle_run(manual, account.id, repo_now() + TimeDelta::minutes(2), "")
        .await
        .expect("finish manual");
    assert_eq!(
        pooled.last_cycle_run(account.id).await.expect("manual run"),
        Some(CycleRun {
            id: manual,
            account_id: account.id,
            trigger: Trigger::Manual,
            started_at: repo_now() + TimeDelta::minutes(1),
            finished_at: Some(repo_now() + TimeDelta::minutes(2)),
            error: String::new(),
            processes_seen: 0,
            events_created: 0,
        })
    );
}

#[tokio::test]
async fn only_the_latest_cycle_runs_are_kept() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = pooled
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");
    let client = connect(&db.url).await;
    client
        .execute(
            r#"INSERT INTO cycle_runs (account_id, "trigger", started_at, finished_at)
            SELECT $1, 'scheduled', $2::timestamptz + make_interval(mins => n), $2::timestamptz + make_interval(mins => n)
            FROM generate_series(0, $3::int - 1) AS n"#,
            &[&account.id, &repo_now(), &1002_i32],
        )
        .await
        .expect("insert old runs");

    let latest = pooled
        .start_cycle_run(
            account.id,
            Trigger::Scheduled,
            repo_now() + TimeDelta::hours(24),
        )
        .await
        .expect("start");
    pooled
        .finish_cycle_run(latest, account.id, repo_now() + TimeDelta::hours(24), "")
        .await
        .expect("finish");

    let total: i64 = client
        .query_one(
            "SELECT count(*) FROM cycle_runs WHERE account_id = $1",
            &[&account.id],
        )
        .await
        .expect("total")
        .get(0);
    let newest: i64 = client
        .query_one(
            "SELECT count(*) FROM cycle_runs WHERE account_id = $1 AND id > $2",
            &[&account.id, &(latest - 1000)],
        )
        .await
        .expect("newest")
        .get(0);
    assert_eq!((total, newest), (1000, 1000));
}

const GO_PAYLOAD: &str = concat!(
    r#"{"process_number":"00000.000001/2026-00","description":"Compra \u003curgente\u003e \u0026 \"rápida\"\u2028linha\u2029fim","activity_text":"tab\there\nline\b\f\u0001\u001f"#,
    "\u{7f}",
    r#"\\ é 😀","assigned_to_self":true,"reason":"a\u003cb\u003ec\u0026d"}"#,
);

async fn account(pooled: &Store) -> Account {
    pooled
        .ensure_account(&key(), repo_now())
        .await
        .expect("account")
}

fn cycle(account_id: i64, now: DateTime<Utc>) -> CycleChanges {
    CycleChanges {
        account_id,
        now,
        ..CycleChanges::default()
    }
}

fn process(procedure_id: i64, number: &str) -> ProcessRow {
    ProcessRow {
        procedure_id,
        number: number.to_owned(),
        unit_id: 1001,
        ..ProcessRow::default()
    }
}

fn document(document_id: i64, procedure_id: i64) -> DocumentRow {
    DocumentRow {
        document_id,
        procedure_id,
        ..DocumentRow::default()
    }
}

fn event(kind: Kind, procedure_id: i64, reference: &str) -> Event {
    Event {
        id: 0,
        account_id: 0,
        kind,
        procedure_id,
        reference: reference.to_owned(),
        occurred_at: None,
        detected_at: None,
        payload: Payload::default(),
    }
}

async fn apply(pooled: &Store, changes: CycleChanges) -> Vec<Event> {
    pooled.apply_cycle(&changes).await.expect("apply the cycle")
}

async fn all_events(pooled: &Store, account_id: i64) -> Vec<Event> {
    pooled
        .events_after(account_id, 0, 100)
        .await
        .expect("events")
}

fn references(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event.reference.as_str())
        .collect()
}

async fn procedure_ids(pooled: &Store, account_id: i64) -> Vec<i64> {
    pooled
        .processes(account_id)
        .await
        .expect("processes")
        .iter()
        .map(|process| process.procedure_id)
        .collect()
}

async fn count(client: &Client, query: &str) -> i64 {
    client.query_one(query, &[]).await.expect("count").get(0)
}

#[tokio::test]
async fn apply_cycle_keeps_the_snapshot() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let first = ProcessRow {
        procedure_id: 9001,
        number: "00000.000001/2026-00".to_owned(),
        process_type: "Contrato".to_owned(),
        description: "Compra de cadeiras".to_owned(),
        unit_id: 1001,
        unit_acronym: "SEDEMO/NUPROC".to_owned(),
        assigned_user: "fulano.tal".to_owned(),
        assigned_user_id: 101,
        return_date: "2026-11-06".to_owned(),
        return_overdue: true,
        last_activity_id: 50003,
        document_total: 2,
        ..ProcessRow::default()
    };
    let second = ProcessRow {
        unit_acronym: "SEDEMO/NUPROC".to_owned(),
        ..process(9002, "00000.000002/2026-00")
    };
    let named = DocumentRow {
        protocol: "0012345".to_owned(),
        document_type: "Ofício".to_owned(),
        name: "Ofício 12 (0012345)".to_owned(),
        ..document(80001, 9001)
    };
    let renamed = DocumentRow {
        name: "Ofício 12 renomeado".to_owned(),
        ..named.clone()
    };

    for (now, document) in [
        (repo_now(), named),
        (repo_now() + TimeDelta::hours(1), renamed),
    ] {
        apply(
            &pooled,
            CycleChanges {
                baseline: true,
                processes: vec![first.clone(), second.clone()],
                documents: vec![document],
                ..cycle(account.id, now)
            },
        )
        .await;
    }

    let seen = |row: ProcessRow| ProcessRow {
        first_seen_at: Some(repo_now()),
        last_seen_at: Some(repo_now() + TimeDelta::hours(1)),
        ..row
    };
    assert_eq!(
        pooled.processes(account.id).await.expect("processes"),
        vec![seen(first), seen(second)]
    );
    assert_eq!(
        pooled
            .document_ids(account.id, 9001)
            .await
            .expect("document ids"),
        HashSet::from([80001])
    );
    let client = connect(&db.url).await;
    assert_eq!(
        count(
            &client,
            "SELECT count(*) FROM documents WHERE name = 'Ofício 12 (0012345)'"
        )
        .await,
        1
    );
    assert_eq!(
        pooled
            .ensure_account(&key(), repo_now())
            .await
            .expect("account again")
            .baseline_at,
        Some(repo_now())
    );
}

#[tokio::test]
async fn processes_leave_and_come_back() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let staying = process(9001, "00000.000001/2026-00");
    let leaving = process(9002, "00000.000002/2026-00");

    apply(
        &pooled,
        CycleChanges {
            processes: vec![staying.clone(), leaving.clone()],
            ..cycle(account.id, repo_now())
        },
    )
    .await;
    for minutes in [15, 20] {
        apply(
            &pooled,
            CycleChanges {
                processes: vec![staying.clone()],
                left: vec![9002],
                ..cycle(account.id, repo_now() + TimeDelta::minutes(minutes))
            },
        )
        .await;
    }

    let gone = pooled.processes(account.id).await.expect("processes");
    assert_eq!(gone[0].left_at, None);
    assert_eq!(gone[1].left_at, Some(repo_now() + TimeDelta::minutes(15)));

    apply(
        &pooled,
        CycleChanges {
            processes: vec![staying, leaving.clone()],
            ..cycle(account.id, repo_now() + TimeDelta::minutes(30))
        },
    )
    .await;

    let back = pooled.processes(account.id).await.expect("processes");
    assert_eq!(
        back[1],
        ProcessRow {
            first_seen_at: Some(repo_now()),
            last_seen_at: Some(repo_now() + TimeDelta::minutes(30)),
            left_at: None,
            ..leaving
        }
    );
}

#[tokio::test]
async fn a_process_both_seen_and_left_in_one_cycle_ends_left() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let both = process(9001, "00000.000001/2026-00");

    apply(
        &pooled,
        CycleChanges {
            processes: vec![both.clone()],
            left: vec![9001],
            ..cycle(account.id, repo_now())
        },
    )
    .await;

    assert_eq!(
        pooled.processes(account.id).await.expect("processes"),
        vec![ProcessRow {
            first_seen_at: Some(repo_now()),
            last_seen_at: Some(repo_now()),
            left_at: Some(repo_now()),
            ..both
        }]
    );
}

#[tokio::test]
async fn events_are_stored_once_per_key() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let entered = Event {
        payload: Payload {
            process_number: "00000.000001/2026-00".to_owned(),
            ..Payload::default()
        },
        ..event(Kind::ProcessEntered, 9001, "9001:1")
    };
    let activity = Event {
        occurred_at: Some(repo_now() - TimeDelta::hours(1)),
        payload: Payload {
            activity_text: "Processo recebido".to_owned(),
            ..Payload::default()
        },
        ..event(Kind::ActivityAdded, 9001, "50003")
    };
    let mut changes = CycleChanges {
        events: vec![entered.clone(), activity.clone()],
        ..cycle(account.id, repo_now())
    };

    let inserted = apply(&pooled, changes.clone()).await;
    changes
        .events
        .push(event(Kind::ActivityAdded, 9001, "50004"));
    let again = apply(&pooled, changes).await;

    assert_eq!(inserted.len(), 2, "{inserted:?}");
    assert_ne!(inserted[0].id, inserted[1].id);
    assert_eq!(
        inserted,
        vec![
            Event {
                id: inserted[0].id,
                account_id: account.id,
                occurred_at: Some(repo_now()),
                detected_at: Some(repo_now()),
                ..entered
            },
            Event {
                id: inserted[1].id,
                account_id: account.id,
                occurred_at: Some(repo_now() - TimeDelta::hours(1)),
                detected_at: Some(repo_now()),
                ..activity
            },
        ]
    );
    assert_eq!(references(&again), vec!["50004"]);
    let client = connect(&db.url).await;
    assert_eq!(count(&client, "SELECT count(*) FROM events").await, 3);
    assert_eq!(
        all_events(&pooled, account.id).await,
        [inserted, again].concat()
    );
}

#[tokio::test]
async fn payloads_are_stored_as_go_writes_them() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let payload = Payload {
        process_number: "00000.000001/2026-00".to_owned(),
        description: "Compra <urgente> & \"rápida\"\u{2028}linha\u{2029}fim".to_owned(),
        activity_text: "tab\there\nline\u{8}\u{c}\u{1}\u{1f}\u{7f}\\ é 😀".to_owned(),
        assigned_to_self: true,
        reason: "a<b>c&d".to_owned(),
        ..Payload::default()
    };

    apply(
        &pooled,
        CycleChanges {
            events: vec![Event {
                payload: payload.clone(),
                ..event(Kind::ActivityAdded, 9001, "50001")
            }],
            ..cycle(account.id, repo_now())
        },
    )
    .await;

    let client = connect(&db.url).await;
    let stored: String = client
        .query_one("SELECT payload FROM events", &[])
        .await
        .expect("payload")
        .get(0);
    assert_eq!(stored, GO_PAYLOAD);
    assert_eq!(all_events(&pooled, account.id).await[0].payload, payload);
}

#[tokio::test]
async fn apply_cycle_is_all_or_nothing() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let run = pooled
        .start_cycle_run(account.id, Trigger::Scheduled, repo_now())
        .await
        .expect("start");

    let error = pooled
        .apply_cycle(&CycleChanges {
            run_id: Some(run),
            finished_at: repo_now() + TimeDelta::minutes(1),
            baseline: true,
            processes: vec![process(9001, "00000.000001/2026-00")],
            documents: vec![document(80001, 9999)],
            events: vec![event(Kind::ProcessEntered, 9001, "9001:1")],
            ..cycle(account.id, repo_now())
        })
        .await
        .expect_err("a document of an unknown process was accepted");

    assert!(matches!(error, StoreError::Server(_)), "{error:?}");
    assert_eq!(error.sql_state(), Some("23503"));
    let client = connect(&db.url).await;
    assert_eq!(
        count(
            &client,
            "SELECT (SELECT count(*) FROM processes) + (SELECT count(*) FROM events)"
        )
        .await,
        0
    );
    assert_eq!(
        pooled
            .ensure_account(&key(), repo_now())
            .await
            .expect("account again")
            .baseline_at,
        None
    );
    let last = pooled
        .last_cycle_run(account.id)
        .await
        .expect("last run")
        .expect("the started run");
    assert_eq!((last.id, last.finished_at), (run, None));
}

#[tokio::test]
async fn a_finished_cycle_closes_its_run() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let started_at = repo_now() + TimeDelta::minutes(1);
    let run = pooled
        .start_cycle_run(account.id, Trigger::Manual, started_at)
        .await
        .expect("start");

    apply(
        &pooled,
        CycleChanges {
            run_id: Some(run),
            finished_at: repo_now() + TimeDelta::minutes(2),
            processes: vec![process(9001, "00000.000001/2026-00")],
            events: vec![event(Kind::ProcessEntered, 9001, "9001:1")],
            ..cycle(account.id, started_at)
        },
    )
    .await;

    assert_eq!(
        pooled.last_cycle_run(account.id).await.expect("last run"),
        Some(CycleRun {
            id: run,
            account_id: account.id,
            trigger: Trigger::Manual,
            started_at,
            finished_at: Some(repo_now() + TimeDelta::minutes(2)),
            error: String::new(),
            processes_seen: 1,
            events_created: 1,
        })
    );
}

#[tokio::test]
async fn last_successful_run_skips_failures_and_unfinished_runs() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let other = pooled
        .ensure_account(
            &AccountKey {
                org_id: 64,
                ..key()
            },
            repo_now(),
        )
        .await
        .expect("another agency");
    let finished = |start: DateTime<Utc>| start + TimeDelta::seconds(10);
    let mut runs = Vec::new();
    for (hours, error) in [(0, ""), (1, "SEI answered 500")] {
        let started_at = repo_now() + TimeDelta::hours(hours);
        let run = pooled
            .start_cycle_run(account.id, Trigger::Scheduled, started_at)
            .await
            .expect("start");
        pooled
            .finish_cycle_run(run, account.id, finished(started_at), error)
            .await
            .expect("finish");
        runs.push(run);
    }
    pooled
        .start_cycle_run(
            account.id,
            Trigger::Scheduled,
            repo_now() + TimeDelta::hours(2),
        )
        .await
        .expect("start the unfinished run");

    assert_eq!(
        pooled
            .last_successful_cycle_run(account.id)
            .await
            .expect("last success"),
        Some(CycleRun {
            id: runs[0],
            account_id: account.id,
            trigger: Trigger::Scheduled,
            started_at: repo_now(),
            finished_at: Some(finished(repo_now())),
            error: String::new(),
            processes_seen: 0,
            events_created: 0,
        })
    );
    assert_eq!(
        pooled
            .last_successful_cycle_run(other.id)
            .await
            .expect("no runs"),
        None
    );
}

#[tokio::test]
async fn events_after_returns_one_account_in_increasing_ids() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let first = account(&pooled).await;
    let second = pooled
        .ensure_account(
            &AccountKey {
                org_id: 64,
                ..key()
            },
            repo_now(),
        )
        .await
        .expect("another agency");
    for (owner, reference) in [
        (&first, "a1"),
        (&second, "b1"),
        (&first, "a2"),
        (&second, "b2"),
        (&first, "a3"),
    ] {
        apply(
            &pooled,
            CycleChanges {
                events: vec![event(Kind::ActivityAdded, 9001, reference)],
                ..cycle(owner.id, repo_now())
            },
        )
        .await;
    }

    let events = all_events(&pooled, first.id).await;
    assert_eq!(references(&events), vec!["a1", "a2", "a3"]);
    assert!(events.iter().all(|event| event.account_id == first.id));
    assert!(events.windows(2).all(|pair| pair[0].id < pair[1].id));
    assert_eq!(
        pooled
            .events_after(first.id, events[0].id, 1)
            .await
            .expect("one after the first"),
        vec![events[1].clone()]
    );
}

async fn prune_scenario(pooled: &Store, account_id: i64) {
    let staying = process(9001, "00000.000001/2026-00");
    let left_early = process(9002, "00000.000002/2026-00");
    let left_late = process(9003, "00000.000003/2026-00");
    apply(
        pooled,
        CycleChanges {
            processes: vec![staying.clone(), left_early, left_late.clone()],
            documents: vec![document(80001, 9001), document(80002, 9002)],
            events: vec![event(Kind::ActivityAdded, 9001, "50001")],
            ..cycle(account_id, repo_now())
        },
    )
    .await;
    apply(
        pooled,
        CycleChanges {
            processes: vec![staying.clone(), left_late],
            left: vec![9002],
            ..cycle(account_id, repo_now() + TimeDelta::hours(1))
        },
    )
    .await;
    apply(
        pooled,
        CycleChanges {
            processes: vec![staying],
            left: vec![9003],
            events: vec![event(Kind::ActivityAdded, 9001, "50002")],
            ..cycle(account_id, repo_now() + TimeDelta::days(40))
        },
    )
    .await;
}

fn prune_cutoff() -> DateTime<Utc> {
    repo_now() + TimeDelta::days(30)
}

#[tokio::test]
async fn prune_before_removes_only_what_is_older_than_the_cutoff() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    prune_scenario(&pooled, account.id).await;
    let acked = all_events(&pooled, account.id).await[1].id;

    let pruned = pooled
        .prune_before(account.id, prune_cutoff(), acked)
        .await
        .expect("prune");

    assert_eq!(
        pruned,
        Pruned {
            events: 1,
            processes: 1
        }
    );
    assert_eq!(
        references(&all_events(&pooled, account.id).await),
        vec!["50002"]
    );
    assert_eq!(procedure_ids(&pooled, account.id).await, vec![9001, 9003]);
    assert_eq!(
        pooled
            .document_ids(account.id, 9001)
            .await
            .expect("document ids"),
        HashSet::from([80001])
    );
    let client = connect(&db.url).await;
    assert_eq!(
        count(
            &client,
            "SELECT count(*) FROM documents WHERE procedure_id = 9002"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn prune_before_keeps_events_the_hub_has_not_read() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    prune_scenario(&pooled, account.id).await;
    let before = all_events(&pooled, account.id).await;

    let unread = pooled
        .prune_before(account.id, prune_cutoff(), 0)
        .await
        .expect("prune with nothing read");
    assert_eq!(
        unread,
        Pruned {
            events: 0,
            processes: 1
        }
    );
    assert_eq!(all_events(&pooled, account.id).await, before);

    let future = pooled
        .prune_before(account.id, repo_now() + TimeDelta::days(400), before[0].id)
        .await
        .expect("prune with a cutoff in the future");
    assert_eq!(
        future,
        Pruned {
            events: 1,
            processes: 1
        }
    );
    assert_eq!(all_events(&pooled, account.id).await, before[1..].to_vec());
    assert_eq!(procedure_ids(&pooled, account.id).await, vec![9001]);
}

const REFUSE_EVENTS_AT_COMMIT: &str = "CREATE FUNCTION refuse_events() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$;
     CREATE CONSTRAINT TRIGGER refuse_at_commit AFTER INSERT ON events DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_events();";

#[tokio::test]
async fn apply_cycle_signals_the_newest_id_it_committed() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    apply(&pooled, readable(account.id, "earlier")).await;
    let mut signals = pooled.notifier().subscribe();
    signals.latest_seq();
    let changes = CycleChanges {
        events: vec![
            event(Kind::ProcessEntered, 9001, "9001:1"),
            event(Kind::ActivityAdded, 9001, "50003"),
            // Meets the stored event's key: no row, but its insert spends a sequence value.
            event(Kind::ActivityAdded, 9001, "earlier"),
        ],
        ..cycle(account.id, repo_now())
    };

    let inserted = apply(&pooled, changes.clone()).await;
    let signalled = next_signal(&mut signals).await;
    let issued = pooled.last_issued_seq().await.expect("the last issued seq");
    let again = apply(&pooled, changes).await;
    let after_the_conflict = pending_signal(&mut signals).await;

    assert_eq!(inserted.len(), 2, "{inserted:?}");
    assert_eq!(Some(signalled), inserted.iter().map(|event| event.id).max());
    assert_eq!(issued, signalled + 1);
    assert_eq!(again, Vec::new());
    assert_eq!(after_the_conflict, None);
}

#[tokio::test]
async fn an_apply_cycle_refused_at_commit_signals_nothing() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    apply(&pooled, readable(account.id, "committed")).await;
    connect(&db.url)
        .await
        .batch_execute(REFUSE_EVENTS_AT_COMMIT)
        .await
        .expect("a trigger that refuses every event at commit");
    let before = all_events(&pooled, account.id).await;
    let mut signals = pooled.notifier().subscribe();
    let seen = signals.latest_seq();

    let outcome = pooled
        .apply_cycle(&readable(account.id, "refused"))
        .await
        .expect_err("a cycle refused at commit");
    let signalled = pending_signal(&mut signals).await;

    assert!(matches!(outcome, StoreError::Server(_)), "{outcome:?}");
    assert_eq!(outcome.sql_state(), Some("P0001"));
    assert_eq!(signalled, None);
    assert_eq!(signals.latest_seq(), seen);
    assert_eq!(all_events(&pooled, account.id).await, before);
}

const LOCK_WAITERS: &str = "SELECT count(*) FROM pg_stat_activity
WHERE datname = current_database() AND wait_event_type = 'Lock'";

type CycleTask = JoinHandle<Result<Vec<Event>, StoreError>>;

async fn settle(watcher: &Client, waiters: i64, task: &CycleTask) {
    let started = Instant::now();
    loop {
        let waiting: i64 = watcher
            .query_one(LOCK_WAITERS, &[])
            .await
            .expect("lock waiters")
            .get(0);
        if waiting >= waiters || task.is_finished() {
            return;
        }
        assert!(
            started.elapsed() < LOCK_TIMEOUT,
            "{waiting} lock waiters, want {waiters}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn spawn_cycle(pooled: &Store, changes: CycleChanges) -> CycleTask {
    let pooled = pooled.clone();
    tokio::spawn(async move { pooled.apply_cycle(&changes).await })
}

#[tokio::test]
async fn concurrent_cycles_commit_in_position_order() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let run = pooled
        .start_cycle_run(account.id, Trigger::Scheduled, repo_now())
        .await
        .expect("start");
    let mut outside = connect(&db.url).await;
    let held = outside.transaction().await.expect("begin");
    held.query_one(
        "SELECT id FROM cycle_runs WHERE id = $1 FOR UPDATE",
        &[&run],
    )
    .await
    .expect("hold the run row");
    let watcher = connect(&db.url).await;

    let first = spawn_cycle(
        &pooled,
        CycleChanges {
            run_id: Some(run),
            finished_at: repo_now() + TimeDelta::minutes(1),
            events: vec![event(Kind::ActivityAdded, 9001, "first")],
            ..cycle(account.id, repo_now())
        },
    );
    settle(&watcher, 1, &first).await;
    assert!(
        !first.is_finished(),
        "the first cycle did not wait on its run"
    );
    let second = spawn_cycle(
        &pooled,
        CycleChanges {
            events: vec![event(Kind::ActivityAdded, 9001, "second")],
            ..cycle(account.id, repo_now())
        },
    );
    settle(&watcher, 2, &second).await;

    assert_eq!(
        all_events(&pooled, account.id).await,
        Vec::new(),
        "a later position was readable while an earlier one was still uncommitted"
    );
    assert!(!second.is_finished());

    held.rollback().await.expect("release the run row");
    let first = first.await.expect("join").expect("the first cycle");
    let second = second.await.expect("join").expect("the second cycle");
    assert!(first[0].id < second[0].id, "{first:?} {second:?}");
    assert_eq!(
        all_events(&pooled, account.id).await,
        [first, second].concat()
    );
}

#[tokio::test]
async fn a_run_starts_while_a_cycle_of_its_account_is_open() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let run = pooled
        .start_cycle_run(account.id, Trigger::Scheduled, repo_now())
        .await
        .expect("start");
    let mut outside = connect(&db.url).await;
    let held = outside.transaction().await.expect("begin");
    held.query_one(
        "SELECT id FROM cycle_runs WHERE id = $1 FOR UPDATE",
        &[&run],
    )
    .await
    .expect("hold the run row");
    let watcher = connect(&db.url).await;
    let open = spawn_cycle(
        &pooled,
        CycleChanges {
            run_id: Some(run),
            finished_at: repo_now() + TimeDelta::minutes(1),
            events: vec![event(Kind::ActivityAdded, 9001, "open")],
            ..cycle(account.id, repo_now())
        },
    );
    settle(&watcher, 1, &open).await;
    assert!(!open.is_finished(), "the cycle did not wait on its run");

    let next = tokio::time::timeout(
        Duration::from_secs(1),
        pooled.start_cycle_run(
            account.id,
            Trigger::Manual,
            repo_now() + TimeDelta::minutes(1),
        ),
    )
    .await
    .expect("start_cycle_run waited behind the open cycle")
    .expect("start the next run");

    held.rollback().await.expect("release the run row");
    let inserted = open.await.expect("join").expect("the open cycle");
    assert_eq!(references(&inserted), vec!["open"]);
    assert_eq!(
        pooled
            .last_cycle_run(account.id)
            .await
            .expect("last run")
            .map(|last| last.id),
        Some(next)
    );
}

async fn store_broken(client: &Client, account_id: i64, kind: &str, payload: &str) -> i64 {
    let reference = format!("broken-{kind}-{payload}");
    client
        .query_one(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             VALUES ($1, $2, 9001, $3, $4, $4, $5) RETURNING id",
            &[&account_id, &kind, &reference, &repo_now(), &payload],
        )
        .await
        .expect("store a broken event")
        .get(0)
}

async fn read_within(
    pooled: &Store,
    account_id: i64,
    after: i64,
    limit: i64,
) -> Result<Vec<Event>, StoreError> {
    tokio::time::timeout(READ_LIMIT, pooled.events_after(account_id, after, limit))
        .await
        .expect("events_after outlived its time limit")
}

fn readable(account_id: i64, reference: &str) -> CycleChanges {
    CycleChanges {
        events: vec![event(Kind::ActivityAdded, 9001, reference)],
        ..cycle(account_id, repo_now())
    }
}

#[tokio::test]
async fn unreadable_stored_events_are_skipped_and_reported_once() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let client = connect(&db.url).await;
    let before = apply(&pooled, readable(account.id, "before")).await;
    let unknown = store_broken(&client, account.id, "activity_renamed", "{}").await;
    let garbled = store_broken(
        &client,
        account.id,
        "activity_added",
        r#"{"assigned_to_self":"do-not-print"}"#,
    )
    .await;
    let after = apply(&pooled, readable(account.id, "after")).await;

    let (first, logs) = captured(read_within(&pooled, account.id, 0, 100)).await;

    assert_eq!(
        first.expect("the readable events"),
        [before, after].concat()
    );
    let skipped = |event_id: i64, error: &str| {
        logged(&[
            ("message", "skipping a stored event that cannot be read"),
            ("level", "ERROR"),
            ("event_id", &event_id.to_string()),
            ("error", error),
        ])
    };
    assert_eq!(
        logs,
        vec![
            skipped(unknown, "its kind is not one sei-monitor knows"),
            skipped(
                garbled,
                "its payload is not an event payload: a field has the wrong type at line 1, column 34"
            ),
        ]
    );

    let (again, logs) = captured(read_within(&pooled, account.id, 0, 100)).await;
    assert_eq!(again.expect("the same events again").len(), 2);
    assert_eq!(logs, Vec::<LoggedEvent>::new());
}

#[tokio::test]
async fn a_page_of_unreadable_events_is_not_the_end() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let client = connect(&db.url).await;
    for payload in ["{", r#"{"unit":7}"#] {
        store_broken(&client, account.id, "activity_added", payload).await;
    }
    let mut readable_events = Vec::new();
    for reference in ["a1", "a2"] {
        readable_events.extend(apply(&pooled, readable(account.id, reference)).await);
    }
    store_broken(&client, account.id, "activity_added", r#""x""#).await;

    assert_eq!(
        read_within(&pooled, account.id, 0, 2)
            .await
            .expect("the events after a page of unreadable ones"),
        readable_events
    );
    assert_eq!(
        read_within(&pooled, account.id, readable_events[1].id, 2)
            .await
            .expect("only unreadable events left"),
        Vec::new()
    );
}

async fn store_unreadable_run(client: &Client, account_id: i64, rows: i32) {
    client
        .execute(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             SELECT $1, 'activity_added', 9001, 'unreadable-' || n, $2, $2, '{'
             FROM generate_series(1, $3::int) AS n",
            &[&account_id, &repo_now(), &rows],
        )
        .await
        .expect("store a run of unreadable events");
}

#[tokio::test]
async fn a_long_run_of_unreadable_events_is_read_past_whatever_the_limit() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let client = connect(&db.url).await;
    store_unreadable_run(&client, account.id, 50_000).await;
    let last = apply(&pooled, readable(account.id, "last")).await;

    for limit in [1, 10] {
        assert_eq!(
            read_within(&pooled, account.id, 0, limit)
                .await
                .expect("the event after the unreadable run"),
            last
        );
    }
}

#[tokio::test]
async fn a_read_gives_up_after_walking_too_many_unreadable_events() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let client = connect(&db.url).await;
    store_unreadable_run(&client, account.id, 100_001).await;
    apply(&pooled, readable(account.id, "unreached")).await;

    let error = read_within(&pooled, account.id, 0, 100)
        .await
        .expect_err("a read walked past its row limit");

    assert!(
        matches!(error, StoreError::TooManyUnreadableEvents(100_000)),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "too many stored events cannot be read: 100000 rows read"
    );
}

#[tokio::test]
async fn past_the_reported_limit_one_line_says_so_and_reads_stay_quiet() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let pooled = store(&db.url);
    let account = account(&pooled).await;
    let client = connect(&db.url).await;
    store_unreadable_run(&client, account.id, 3_000).await;
    let reported: Vec<i64> = client
        .query("SELECT id FROM events ORDER BY id LIMIT 1024", &[])
        .await
        .expect("the first ids")
        .iter()
        .map(|row| row.get(0))
        .collect();

    let (first, logs) = captured(read_within(&pooled, account.id, 0, 100)).await;

    assert_eq!(first.expect("no readable events"), Vec::new());
    let mut expected: Vec<LoggedEvent> = reported
        .iter()
        .map(|event_id| {
            logged(&[
                ("message", "skipping a stored event that cannot be read"),
                ("level", "ERROR"),
                ("event_id", &event_id.to_string()),
                (
                    "error",
                    "its payload is not an event payload: the JSON ends early at line 1, column 1",
                ),
            ])
        })
        .collect();
    expected.push(logged(&[
        (
            "message",
            "more stored events cannot be read than are reported one by one",
        ),
        ("level", "ERROR"),
        ("limit", "1024"),
    ]));
    assert_eq!(logs.len(), expected.len());
    assert_eq!(logs, expected);

    let (again, logs) = captured(read_within(&pooled, account.id, 0, 100)).await;
    assert_eq!(again.expect("no readable events again"), Vec::new());
    assert_eq!(logs, Vec::<LoggedEvent>::new());
}

#[tokio::test]
async fn the_newest_event_id_is_the_id_of_the_newest_stored_event() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let store = store(&db.url);
    assert_eq!(store.newest_event_id().await.expect("on a new database"), 0);
    let account = store
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");
    let client = connect(&db.url).await;
    let stored: Vec<i64> = client
        .query(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             SELECT $1, 'process_left', 1, 'r' || n, $2, $2, '{}'
             FROM generate_series(1, 3) AS n
             RETURNING id",
            &[&account.id, &repo_now()],
        )
        .await
        .expect("insert events")
        .iter()
        .map(|row| row.get(0))
        .collect();
    let newest = *stored.iter().max().expect("three events");
    client
        .execute(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             VALUES ($1, 'process_left', 1, 'r1', $2, $2, '{}')
             ON CONFLICT (account_id, kind, ref) DO NOTHING",
            &[&account.id, &repo_now()],
        )
        .await
        .expect("insert an event that exists");

    assert_eq!(store.last_issued_seq().await.expect("issued"), newest + 1);
    assert_eq!(store.newest_event_id().await.expect("stored"), newest);
}

#[tokio::test]
async fn the_last_issued_seq_never_goes_back() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let store = store(&db.url);
    assert_eq!(store.last_issued_seq().await.expect("on a new database"), 0);
    let account = store
        .ensure_account(&key(), repo_now())
        .await
        .expect("account");
    let client = connect(&db.url).await;
    let issued: Vec<i64> = client
        .query(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             SELECT $1, 'process_left', 1, 'r' || n, $2, $2, '{}'
             FROM generate_series(1, 3) AS n
             RETURNING id",
            &[&account.id, &repo_now()],
        )
        .await
        .expect("insert events")
        .iter()
        .map(|row| row.get(0))
        .collect();
    let newest = *issued.iter().max().expect("three events");

    assert_eq!(store.last_issued_seq().await.expect("issued"), newest);
    client
        .execute("DELETE FROM events", &[])
        .await
        .expect("remove every event");
    assert_eq!(
        store.last_issued_seq().await.expect("after pruning"),
        newest
    );
}

#[tokio::test]
async fn test_databases_are_reached_over_tcp() {
    let Some(db) = database().await else {
        return;
    };
    let row = connect(&db.url)
        .await
        .query_one("SELECT inet_server_addr() IS NOT NULL", &[])
        .await
        .expect("read the server address");
    assert!(row.get::<_, bool>(0));
}
