mod support;

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Days, Utc};
use monitor_hub_plugin_testkit::database;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, NoTls};

use sei_monitor::monitor::{CycleResult, Monitor, SystemClock};
use sei_monitor::secret::Secret;
use sei_monitor::store::{Store, Trigger, acquire_writer_lock};

use support::fake_sei::{FakeSei, ORG_ACRONYM, PASSWORD, TIME_ZONE, UNIT_NUPROC, USERNAME};
use support::go::{RunningGo, go_v1_binary, run_go};
use support::harness::{activity, bounded, config, document, no_jitter, populate, process, user};
use support::webhook::Webhook;
use support::{migrated_database, normalized_sei_url};

const GO_LIMIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);
const WEBHOOK_SECRET: &str = "webhook-secret-for-tests";
const RUST_TABLES: [&str; 5] = ["accounts", "cycle_runs", "documents", "events", "processes"];

type StoredEvent = (i64, String, String);
type TableRows = (String, Vec<String>);
type DeliveryRow = (String, i64, String, String, i64);

async fn admin(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("connect as the test superuser");
    tokio::spawn(connection);
    client
}

fn sei_url(fake: &FakeSei) -> String {
    format!("{}/sei/", fake.url())
}

fn populate_due_tomorrow(fake: &FakeSei) {
    populate(fake);
    let today = Utc::now().with_timezone(&fake.zone()).date_naive();
    let tomorrow = (today + Days::new(1)).format("%d/%m/%Y").to_string();
    fake.update_process(UNIT_NUPROC, 9001, |process| process.return_date = tomorrow);
}

fn change_like_the_next_cycle(fake: &FakeSei) {
    let now = Utc::now();
    fake.remove_process(UNIT_NUPROC, 9002);
    fake.add_process(UNIT_NUPROC, process(9003, 3, "Ofício"));
    fake.add_activities(
        9003,
        vec![
            activity(70001, now, "Processo gerado"),
            activity(70002, now, "Processo remetido"),
        ],
    );
    fake.add_documents(9003, vec![document(90001, "Ofício 9")]);
    fake.update_process(UNIT_NUPROC, 9001, |process| {
        process.assignee = Some(user(202, "beltrano.silva", "Beltrano da Silva"));
    });
    fake.add_activities(9001, vec![activity(50004, now, "Documento assinado")]);
    fake.add_documents(9001, vec![document(80004, "Ofício 4")]);
}

fn go_env(fake: &FakeSei, database_url: &str, webhook: &Webhook) -> Vec<(&'static str, String)> {
    vec![
        ("SEI_URL", sei_url(fake)),
        ("SEI_USERNAME", USERNAME.to_owned()),
        ("SEI_PASSWORD", PASSWORD.to_owned()),
        ("SEI_ORG", ORG_ACRONYM.to_owned()),
        ("SEI_TIMEZONE", TIME_ZONE.to_owned()),
        ("DATABASE_URL", database_url.to_owned()),
        ("HTTP_ADDRESS", String::new()),
        ("POLL_INTERVAL", "5m".to_owned()),
        ("LOG_LEVEL", "debug".to_owned()),
        ("WEBHOOK_URL", webhook.url()),
        ("WEBHOOK_SECRET", WEBHOOK_SECRET.to_owned()),
    ]
}

fn start_go(binary: &Path, env: &[(&'static str, String)]) -> RunningGo {
    let env: Vec<(&str, &str)> = env
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    run_go(binary, &env)
}

async fn wait_for<F: Future<Output = bool>>(
    go: &mut RunningGo,
    what: &str,
    mut done: impl FnMut() -> F,
) {
    let deadline = Instant::now() + GO_LIMIT;
    while !done().await {
        if !go.still_running() {
            go.fail(&format!("the Go program exited while waiting until {what}"));
        }
        if Instant::now() >= deadline {
            go.fail(&format!(
                "timed out after {GO_LIMIT:?} waiting until {what}"
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn finished_runs(admin: &Client) -> i64 {
    let counted = admin
        .query_one(
            "SELECT count(*) FROM cycle_runs WHERE finished_at IS NOT NULL",
            &[],
        )
        .await;
    match counted {
        Ok(row) => row.get(0),
        Err(error) if error.code() == Some(&SqlState::UNDEFINED_TABLE) => 0,
        Err(error) => panic!("count the finished cycle runs: {error}"),
    }
}

async fn run_go_until_finished(
    binary: &Path,
    env: &[(&'static str, String)],
    admin: &Client,
    runs: i64,
) {
    let mut go = start_go(binary, env);
    wait_for(
        &mut go,
        &format!("cycle_runs holds {runs} finished runs"),
        || async { finished_runs(admin).await >= runs },
    )
    .await;
    go.stop();
}

async fn run_errors(admin: &Client) -> Vec<String> {
    admin
        .query("SELECT error FROM cycle_runs ORDER BY id", &[])
        .await
        .expect("cycle_runs")
        .iter()
        .map(|row| row.get(0))
        .collect()
}

async fn events(admin: &Client) -> Vec<StoredEvent> {
    admin
        .query("SELECT id, kind, ref FROM events ORDER BY id", &[])
        .await
        .expect("events")
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect()
}

async fn account_ids(admin: &Client) -> Vec<i64> {
    admin
        .query("SELECT id FROM accounts ORDER BY id", &[])
        .await
        .expect("accounts")
        .iter()
        .map(|row| row.get(0))
        .collect()
}

async fn applied_versions(admin: &Client) -> Vec<(i64, i64)> {
    admin
        .query(
            "SELECT version_id, count(*) FROM goose_db_version WHERE is_applied
             GROUP BY version_id ORDER BY version_id",
            &[],
        )
        .await
        .expect("goose_db_version")
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect()
}

async fn tables_rust_never_writes(admin: &Client) -> Vec<TableRows> {
    let names: Vec<String> = admin
        .query(
            "SELECT table_name::text FROM information_schema.tables
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
             ORDER BY table_name",
            &[],
        )
        .await
        .expect("the tables")
        .iter()
        .map(|row| row.get(0))
        .filter(|name: &String| !RUST_TABLES.contains(&name.as_str()))
        .collect();
    let mut tables = Vec::new();
    for name in names {
        let rows = admin
            .query(
                &format!("SELECT row_to_json(t)::text FROM \"{name}\" t ORDER BY 1"),
                &[],
            )
            .await
            .unwrap_or_else(|error| panic!("read the table {name}: {error}"))
            .iter()
            .map(|row| row.get(0))
            .collect();
        tables.push((name, rows));
    }
    tables
}

async fn deliveries(admin: &Client) -> Vec<DeliveryRow> {
    admin
        .query(
            "SELECT e.kind, e.procedure_id, d.channel, d.status, d.attempts
             FROM deliveries d JOIN events e ON e.id = d.event_id
             ORDER BY d.event_id, d.channel",
            &[],
        )
        .await
        .expect("deliveries")
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2), row.get(3), row.get(4)))
        .collect()
}

async fn barrier_sent(admin: &Client) -> bool {
    let sent: i64 = admin
        .query_one(
            "SELECT count(*) FROM deliveries d JOIN events e ON e.id = d.event_id
             WHERE e.kind = 'process_entered' AND e.procedure_id = 9005
               AND d.channel = 'webhook' AND d.status = 'sent'",
            &[],
        )
        .await
        .expect("the delivery of process 9005")
        .get(0);
    sent == 1
}

fn rust_monitor(fake: &FakeSei, store: &Store) -> Arc<Monitor> {
    let mut monitor_config = config(fake, PASSWORD);
    monitor_config.target.sei_url = normalized_sei_url(&sei_url(fake));
    Monitor::new(
        monitor_config,
        store.clone(),
        Arc::new(SystemClock),
        no_jitter(),
    )
}

async fn rust_cycle(monitor: &Monitor) -> CycleResult {
    bounded(monitor.run_once(Trigger::Scheduled))
        .await
        .expect("the Rust cycle")
}

fn kinds(events: &[StoredEvent]) -> Vec<&str> {
    events.iter().map(|(_, kind, _)| kind.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_continues_a_database_left_by_go() {
    let Some(go) = go_v1_binary() else {
        return;
    };
    let Some(database) = database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate_due_tomorrow(&fake);
    let webhook = Webhook::start();
    let admin = admin(&database.url).await;
    let env = go_env(&fake, &database.url, &webhook);

    run_go_until_finished(&go, &env, &admin, 1).await;
    fake.remove_process(UNIT_NUPROC, 9002);
    fake.add_process(UNIT_NUPROC, process(9003, 3, "Ofício"));
    fake.add_activities(
        9001,
        vec![activity(50004, Utc::now(), "Documento assinado")],
    );
    run_go_until_finished(&go, &env, &admin, 2).await;
    assert_eq!(run_errors(&admin).await, ["", ""]);
    let left_by_go = events(&admin).await;
    let mut go_kinds = kinds(&left_by_go);
    go_kinds.sort_unstable();
    assert_eq!(
        go_kinds,
        [
            "activity_added",
            "process_entered",
            "process_left",
            "return_due_soon"
        ]
    );
    let go_tables = tables_rust_never_writes(&admin).await;
    let names: Vec<&str> = go_tables.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["deliveries", "goose_db_version"]);
    assert_eq!(go_tables[0].1.len(), left_by_go.len(), "{go_tables:?}");
    let go_accounts = account_ids(&admin).await;
    assert_eq!(go_accounts.len(), 1);
    fake.add_activities(9001, vec![activity(50005, Utc::now(), "Processo enviado")]);

    let secret = Secret::new(database.url.clone());
    let mut lock = acquire_writer_lock(&secret).await.expect("writer lock");
    assert_eq!(lock.migrate().await.expect("migrate"), Vec::<i64>::new());
    let store = Store::new(&secret).expect("store");
    let monitor = rust_monitor(&fake, &store);
    bounded(monitor.prepare()).await.expect("prepare");
    let result = rust_cycle(&monitor).await;

    let all = events(&admin).await;
    let newest_from_go = left_by_go.last().map_or(0, |(id, _, _)| *id);
    let added: Vec<(&str, &str)> = all
        .iter()
        .filter(|(id, _, _)| *id > newest_from_go)
        .map(|(_, kind, reference)| (kind.as_str(), reference.as_str()))
        .collect();
    assert_eq!(added, [("activity_added", "50005")]);
    assert_eq!(result.events_created, 1);
    assert_eq!(account_ids(&admin).await, go_accounts);
    assert_eq!(monitor.state().account_id, go_accounts[0]);
    let mut pairs: Vec<(&str, &str)> = all
        .iter()
        .map(|(_, kind, reference)| (kind.as_str(), reference.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    assert_eq!(pairs.len(), all.len(), "{all:?}");
    let due_soon = kinds(&all)
        .into_iter()
        .filter(|kind| *kind == "return_due_soon")
        .count();
    assert_eq!(due_soon, 1);
    assert_eq!(applied_versions(&admin).await, [(0, 1), (1, 1), (2, 1)]);
    assert_eq!(tables_rust_never_writes(&admin).await, go_tables);
}

#[tokio::test(flavor = "multi_thread")]
async fn go_sends_nothing_from_the_rust_era() {
    let Some(go) = go_v1_binary() else {
        return;
    };
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate_due_tomorrow(&fake);
    let store = Store::new(&Secret::new(database.url.clone())).expect("store");
    let monitor = rust_monitor(&fake, &store);
    bounded(monitor.prepare()).await.expect("prepare");
    rust_cycle(&monitor).await;
    change_like_the_next_cycle(&fake);
    rust_cycle(&monitor).await;
    let admin = admin(&database.url).await;
    let rust_era = events(&admin).await;
    assert_eq!(
        kinds(&rust_era),
        [
            "assignment_changed",
            "return_due_soon",
            "process_entered",
            "process_left",
            "activity_added",
            "document_added"
        ]
    );
    fake.add_process(UNIT_NUPROC, process(9005, 5, "Memorando"));
    let webhook = Webhook::start();

    let mut release = start_go(&go, &go_env(&fake, &database.url, &webhook));
    wait_for(
        &mut release,
        "the Go program records the webhook delivery of process 9005's process_entered as sent",
        || barrier_sent(&admin),
    )
    .await;
    let output = release.stop();

    let bodies = webhook.bodies();
    let [body] = &bodies[..] else {
        panic!("expected one webhook request, got {bodies:#?}\n{output}");
    };
    let sent = body["events"].as_array().expect("an events list");
    let [only] = &sent[..] else {
        panic!("expected one event in the webhook body, got {body:#}\n{output}");
    };
    assert_eq!(only["kind"], "process_entered", "{body:#}");
    assert_eq!(only["procedure_id"], 9005, "{body:#}");
    assert_eq!(
        deliveries(&admin).await,
        [(
            "process_entered".to_owned(),
            9005,
            "webhook".to_owned(),
            "sent".to_owned(),
            1
        )]
    );
    assert_eq!(events(&admin).await.len(), rust_era.len() + 1);
    assert_eq!(account_ids(&admin).await.len(), 1);
    let newest: i64 = admin
        .query_one("SELECT max(version_id) FROM goose_db_version", &[])
        .await
        .expect("goose_db_version")
        .get(0);
    assert_eq!(newest, 2);
}
