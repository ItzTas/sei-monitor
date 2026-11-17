mod support;

use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use monitor_hub_plugin::NOT_LAUNCHED_BY_HUB;
use monitor_hub_plugin::proto::{State, StatusResponse};
use monitor_hub_plugin_testkit::{Database, database};
use serde_json::Value;
use tokio_postgres::{Client, NoTls};

use sei_monitor::secret::Secret;
use sei_monitor::store::{AccountKey, Store, WRITER_LOCK_ID, acquire_writer_lock};

use support::fake_sei::{
    FakeSei, ORG_ACRONYM, ORG_ID, PASSWORD, ROUTE_LOGIN, ROUTE_LOGIN_PAGE, ROUTE_PROCESSES,
    ROUTE_VERSION, TIME_ZONE, UNIT_NUPROC, USERNAME,
};
use support::harness::{Harness, populate, process, start};
use support::process::{
    COOKIE, PluginClient, RunningPlugin, error_lines, plugin_client, run_to_exit, spawn_plugin,
    terminate,
};
use support::{database_url_as, migrated_database};

const EXIT_LIMIT: Duration = Duration::from_secs(30);
const STOP_LIMIT: Duration = Duration::from_secs(2);
const PATIENCE: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(20);
const STAY_UP: Duration = Duration::from_secs(3);
const CLOSE_LIMIT: Duration = Duration::from_millis(700);
const FIRST_RETRY: Duration = Duration::from_secs(30);
const PREPARATION_RETRY_LIMIT: Duration = Duration::from_secs(45);
const REJECTED: &str = "SEI rejected the login (Usuário ou Senha Inválida.)";
const GO_PAUSE: &str = "monitoring is paused because SEI rejected the login; restart sei-monitor after fixing the configuration";
const HELD: &str = "nothing is logging in: request a check once the configuration is fixed";
const PAUSED_HERE: &str = "restart the plugin with the configuration fixed, then request a check";
const DB_ROLE: &str = "sei_test";
const DB_PASSWORD: &str = "db-secret-7f3a";

async fn admin(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("connect as the test superuser");
    tokio::spawn(connection);
    client
}

fn sei_env(fake: &FakeSei, database_url: &str, password: &str) -> Vec<(&'static str, String)> {
    vec![
        ("SEI_URL", fake.url()),
        ("SEI_USERNAME", USERNAME.to_owned()),
        ("SEI_PASSWORD", password.to_owned()),
        ("SEI_ORG", ORG_ACRONYM.to_owned()),
        ("SEI_TIMEZONE", TIME_ZONE.to_owned()),
        ("DATABASE_URL", database_url.to_owned()),
    ]
}

fn with(env: &mut Vec<(&'static str, String)>, name: &'static str, value: &str) {
    env.retain(|(known, _)| *known != name);
    env.push((name, value.to_owned()));
}

fn borrowed<'a>(env: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    env.iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

fn start_plugin(env: &[(&'static str, String)]) -> RunningPlugin {
    spawn_plugin(&borrowed(env))
}

async fn until<T, F: Future<Output = Option<T>>>(
    plugin: &mut RunningPlugin,
    what: &str,
    mut check: impl FnMut() -> F,
) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(found) = check().await {
            return found;
        }
        if Instant::now() >= deadline {
            plugin.fail(&format!("timed out waiting until {what}"));
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn status_where(
    plugin: &mut RunningPlugin,
    client: &mut PluginClient,
    what: &str,
    accept: impl Fn(&StatusResponse) -> bool,
) -> StatusResponse {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let status = client.status().await.expect("status");
        if accept(&status) {
            return status;
        }
        if Instant::now() >= deadline {
            plugin.fail(&format!("timed out waiting until {what}; last {status:?}"));
        }
        tokio::time::sleep(POLL).await;
    }
}

fn single_error(stderr: &str) -> Value {
    let errors = error_lines(stderr);
    let [error] = &errors[..] else {
        panic!("expected one error line:\n{stderr}");
    };
    error.clone()
}

async fn stop_cleanly(plugin: &mut RunningPlugin) {
    terminate(&mut plugin.child);
    let status = plugin.exit_within(STOP_LIMIT);
    assert_eq!(status.code(), Some(0), "{}", plugin.stderr_text());
}

async fn record_rejection(database: &Database, fake: &FakeSei) {
    let store = Store::new(&Secret::new(database.url.clone())).expect("store");
    let account = store
        .ensure_account(
            &AccountKey {
                sei_base_url: fake.url(),
                username: USERNAME.to_owned(),
                org_id: ORG_ID,
            },
            start(),
        )
        .await
        .expect("account");
    store
        .set_paused_reason(account.id, REJECTED)
        .await
        .expect("record the rejection");
}

async fn count(client: &Client, query: &str) -> i64 {
    client.query_one(query, &[]).await.expect("count").get(0)
}

#[test]
fn without_the_cookie_explains_and_exits_1() {
    let exited = run_to_exit(&["plugin"], &[], EXIT_LIMIT);

    assert_eq!(exited.status.code(), Some(1), "{}", exited.stderr);
    assert_eq!(exited.stdout, "");
    assert_eq!(exited.stderr, format!("{NOT_LAUNCHED_BY_HUB}\n"));
}

#[test]
fn without_a_command_explains_and_exits_1() {
    let exited = run_to_exit(&[], &[COOKIE], EXIT_LIMIT);

    assert_eq!(exited.status.code(), Some(1), "{}", exited.stderr);
    assert_eq!(exited.stdout, "");
    assert_eq!(exited.stderr, format!("{NOT_LAUNCHED_BY_HUB}\n"));
}

#[test]
fn usage_errors_exit_2_and_help_and_version_exit_0() {
    let unknown = run_to_exit(&["plugn"], &[], EXIT_LIMIT);
    assert_eq!(unknown.status.code(), Some(2), "{}", unknown.stderr);
    assert!(
        unknown
            .stderr
            .ends_with("\nRun 'sei-monitor --help' for usage.\n"),
        "{}",
        unknown.stderr
    );
    assert_eq!(unknown.stdout, "");

    let version = run_to_exit(&["--version"], &[], EXIT_LIMIT);
    assert_eq!(version.status.code(), Some(0), "{}", version.stderr);
    assert_eq!(version.stdout, "sei-monitor 2.0.0\n");

    let help = run_to_exit(&["--help"], &[], EXIT_LIMIT);
    assert_eq!(help.status.code(), Some(0), "{}", help.stderr);
    assert!(help.stdout.contains("plugin"), "{}", help.stdout);
}

#[test]
fn config_errors_exit_before_the_handshake() {
    let exited = run_to_exit(&["plugin"], &[COOKIE, ("POLL_INTERVAL", "1m")], EXIT_LIMIT);

    assert_eq!(exited.status.code(), Some(1), "{}", exited.stderr);
    assert_eq!(exited.stdout, "");
    let error = single_error(&exited.stderr);
    let text = error.to_string();
    assert!(text.contains("SEI_URL is required"), "{}", exited.stderr);
    assert!(text.contains("SEI_ORG is required"), "{}", exited.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn second_process_refuses_to_start() {
    let Some(database) = database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    let _held = acquire_writer_lock(&Secret::new(database.url.clone()))
        .await
        .expect("hold the lock");
    let client = admin(&database.url).await;
    let env = sei_env(&fake, &database.url, PASSWORD);
    let mut args = vec![COOKIE];
    args.extend(borrowed(&env));

    let exited = run_to_exit(&["plugin"], &args, EXIT_LIMIT);

    assert_eq!(exited.status.code(), Some(1), "{}", exited.stderr);
    assert_eq!(exited.stdout, "");
    let error = single_error(&exited.stderr);
    assert_eq!(error["@message"], "fatal", "{}", exited.stderr);
    assert!(
        error["error"].as_str().is_some_and(
            |text| text.ends_with("another sei-monitor process holds the database lock")
        ),
        "{}",
        exited.stderr
    );
    assert!(!exited.stderr.contains(&database.url), "{}", exited.stderr);
    let migrated: bool = client
        .query_one("SELECT to_regclass('goose_db_version') IS NOT NULL", &[])
        .await
        .expect("look for the migration table")
        .get(0);
    assert!(!migrated, "the second process migrated the database");
    assert_eq!(fake.calls(ROUTE_LOGIN), 0);
    assert_eq!(fake.calls(ROUTE_VERSION), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_password_pauses_and_stays_up() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    let mut plugin = start_plugin(&sei_env(&fake, &database.url, "errada"));
    let started = Instant::now();
    let mut client = plugin_client(&plugin.socket).await;

    let paused = status_where(&mut plugin, &mut client, "the plugin pauses", |status| {
        status.state == i32::from(State::Paused)
    })
    .await;
    assert_eq!(paused.message, format!("{REJECTED}; {PAUSED_HERE}"));
    let refused = client.check().await.expect("check");
    assert!(!refused.accepted);
    assert_eq!(refused.message, format!("{GO_PAUSE}, then request a check"));
    assert_eq!(client.events(0, 0).await.expect("first read"), []);
    let events = client.events(0, 0).await.expect("second read");
    let kinds: Vec<&str> = events.iter().map(|event| event.kind.as_str()).collect();
    assert_eq!(kinds, ["account_login_failed"]);

    tokio::time::sleep(STAY_UP.saturating_sub(started.elapsed())).await;
    assert!(plugin.still_running(), "{}", plugin.stderr_text());
    assert_eq!(fake.failed_logins(), 1);
    drop(client);
    stop_cleanly(&mut plugin).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recorded_rejection_holds_across_restarts_until_a_check() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    fake.set_password("changed-by-the-user");
    record_rejection(&database, &fake).await;
    let env = sei_env(&fake, &database.url, PASSWORD);

    for life in 0..3 {
        let mut plugin = start_plugin(&env);
        let started = Instant::now();
        let mut client = plugin_client(&plugin.socket).await;
        let held = status_where(&mut plugin, &mut client, "the plugin holds", |status| {
            status.state == i32::from(State::Paused)
        })
        .await;
        tokio::time::sleep(STAY_UP.saturating_sub(started.elapsed())).await;
        assert!(plugin.still_running(), "{}", plugin.stderr_text());
        drop(client);
        stop_cleanly(&mut plugin).await;
        assert_eq!((fake.logins(), fake.failed_logins()), (0, 0), "life {life}");
        assert_eq!(held.message, format!("{REJECTED}; {HELD}"), "life {life}");
    }

    let mut plugin = start_plugin(&env);
    let mut client = plugin_client(&plugin.socket).await;
    status_where(&mut plugin, &mut client, "the plugin holds", |status| {
        status.message.ends_with(HELD)
    })
    .await;
    let accepted = client.check().await.expect("check");
    assert!(accepted.accepted, "{accepted:?}");
    let paused = status_where(
        &mut plugin,
        &mut client,
        "the check is rejected",
        |status| status.message.ends_with(PAUSED_HERE),
    )
    .await;
    assert_eq!(paused.state, i32::from(State::Paused));
    assert_eq!(fake.failed_logins(), 1);
    let client_db = admin(&database.url).await;
    assert_eq!(count(&client_db, "SELECT count(*) FROM accounts").await, 1);
    drop(client);
    stop_cleanly(&mut plugin).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fatal_configuration_keeps_the_plugin_up() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    let mut env = sei_env(&fake, &database.url, PASSWORD);
    with(&mut env, "SEI_ORG", "NOPE");
    let mut plugin = start_plugin(&env);
    let started = Instant::now();
    let mut client = plugin_client(&plugin.socket).await;

    let failing = status_where(
        &mut plugin,
        &mut client,
        "the preparation fails",
        |status| status.message.starts_with("agency not found"),
    )
    .await;
    assert_eq!(failing.state, i32::from(State::Failing));
    let deadline = started + PREPARATION_RETRY_LIMIT;
    while fake.calls(ROUTE_LOGIN_PAGE) < 2 {
        if Instant::now() >= deadline {
            plugin.fail("the preparation was never tried again");
        }
        tokio::time::sleep(POLL).await;
    }
    let retried = client.status().await.expect("status");

    assert!(plugin.still_running(), "{}", plugin.stderr_text());
    assert!(started.elapsed() >= FIRST_RETRY, "{:?}", started.elapsed());
    assert_eq!(retried.state, i32::from(State::Failing));
    assert!(
        retried.message.starts_with("agency not found"),
        "{retried:?}"
    );
    assert_eq!((fake.logins(), fake.failed_logins()), (0, 0));
    drop(client);
    stop_cleanly(&mut plugin).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_cycle_shows_in_status_until_one_succeeds() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    fake.fail_next(ROUTE_PROCESSES, 500, 1);
    let mut plugin = start_plugin(&sei_env(&fake, &database.url, PASSWORD));
    let mut client = plugin_client(&plugin.socket).await;

    let failing = status_where(&mut plugin, &mut client, "the cycle fails", |status| {
        status.state == i32::from(State::Failing) && status.message.starts_with("sei: ")
    })
    .await;
    let deadline = Instant::now() + PREPARATION_RETRY_LIMIT;
    let recovered = loop {
        let status = client.status().await.expect("status");
        if status.last_success_at.is_some() {
            break status;
        }
        if Instant::now() >= deadline {
            plugin.fail(&format!("the retry never succeeded; last {status:?}"));
        }
        tokio::time::sleep(POLL).await;
    };

    assert_eq!(
        failing.message,
        "sei: /processo/listar: answer outside the API format (HTTP 500)"
    );
    assert_eq!(failing.last_success_at, None);
    assert_eq!(recovered.state, i32::from(State::Idle), "{recovered:?}");
    assert_eq!(recovered.message, "");
    drop(client);
    stop_cleanly(&mut plugin).await;
}

async fn last_run(client: &Client) -> Option<(bool, String)> {
    client
        .query_opt(
            "SELECT finished_at IS NOT NULL, error FROM cycle_runs ORDER BY id DESC LIMIT 1",
            &[],
        )
        .await
        .expect("the last cycle run")
        .map(|row| (row.get(0), row.get(1)))
}

#[tokio::test(flavor = "multi_thread")]
async fn sigterm_closes_the_cycle_while_a_call_is_still_held() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    fake.stall(ROUTE_PROCESSES);
    let mut plugin = start_plugin(&sei_env(&fake, &database.url, PASSWORD));
    until(&mut plugin, "the cycle asks for the processes", || async {
        (fake.calls(ROUTE_PROCESSES) > 0).then_some(())
    })
    .await;
    let holder = admin(&database.url).await;
    holder
        .batch_execute("BEGIN; LOCK TABLE events IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("hold the events table");
    let observer = admin(&database.url).await;
    let socket = plugin.socket.clone();
    let held_call = tokio::spawn(async move {
        let mut client = plugin_client(&socket).await;
        client.events(0, 0).await
    });
    until(
        &mut plugin,
        "the Events call waits on the events table",
        || async {
            let waiting: i64 = observer
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity
                 WHERE wait_event_type = 'Lock' AND query LIKE '%events%'",
                    &[],
                )
                .await
                .expect("pg_stat_activity")
                .get(0);
            (waiting > 0).then_some(())
        },
    )
    .await;

    let signalled = Instant::now();
    terminate(&mut plugin.child);
    let closed = loop {
        let run = last_run(&observer).await;
        if run.as_ref().is_some_and(|(finished, _)| *finished) {
            break run;
        }
        if signalled.elapsed() >= CLOSE_LIMIT {
            break run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let closed_after = signalled.elapsed();
    let call_still_held = !held_call.is_finished();
    let running_while_closed = plugin.still_running();
    let status = plugin.exit_within(STOP_LIMIT);
    let exited_after = signalled.elapsed();
    holder
        .batch_execute("ROLLBACK")
        .await
        .expect("release the events table");
    let answered = tokio::time::timeout(PATIENCE, held_call)
        .await
        .expect("the held call ended")
        .expect("join the held call");

    assert_eq!(
        closed,
        Some((true, "context canceled".to_owned())),
        "after {closed_after:?}\n{}",
        plugin.stderr_text()
    );
    assert!(
        call_still_held,
        "the call ended before the cycle was closed"
    );
    assert!(
        running_while_closed,
        "the process ended before the cycle was closed"
    );
    assert!(exited_after < STOP_LIMIT, "{exited_after:?}");
    assert_eq!(status.code(), Some(0), "{}", plugin.stderr_text());
    assert!(answered.is_err(), "{answered:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sigterm_during_a_cycle_exits_within_two_seconds() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    fake.stall(ROUTE_PROCESSES);
    let mut plugin = start_plugin(&sei_env(&fake, &database.url, PASSWORD));
    assert!(plugin.socket.exists(), "{}", plugin.socket.display());
    until(&mut plugin, "the cycle asks for the processes", || async {
        (fake.calls(ROUTE_PROCESSES) > 0).then_some(())
    })
    .await;

    let signalled = Instant::now();
    terminate(&mut plugin.child);
    let status = plugin.exit_within(STOP_LIMIT);

    assert!(
        signalled.elapsed() < STOP_LIMIT,
        "{:?}",
        signalled.elapsed()
    );
    assert_eq!(status.code(), Some(0), "{}", plugin.stderr_text());
    assert!(!plugin.socket.exists(), "{}", plugin.socket.display());
    let run = admin(&database.url)
        .await
        .query_one(
            "SELECT finished_at IS NOT NULL, error FROM cycle_runs ORDER BY id DESC LIMIT 1",
            &[],
        )
        .await
        .expect("the last cycle run");
    let (finished, error): (bool, String) = (run.get(0), run.get(1));
    assert!(finished, "{}", plugin.stderr_text());
    assert_eq!(error, "context canceled");
}

#[tokio::test(flavor = "multi_thread")]
async fn losing_the_database_lock_exits_1() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    let mut plugin = start_plugin(&sei_env(&fake, &database.url, PASSWORD));
    let client = admin(&database.url).await;

    let terminated: Vec<bool> = client
        .query(
            "SELECT pg_terminate_backend(pid) FROM pg_locks
             WHERE locktype = 'advisory'
               AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
               AND objsubid = 1
               AND ((classid::bigint << 32) | objid::bigint) = $1",
            &[&WRITER_LOCK_ID],
        )
        .await
        .expect("terminate the lock's backend")
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(terminated, [true]);
    let status = plugin.exit_within(Duration::from_secs(5));

    assert_eq!(status.code(), Some(1), "{}", plugin.stderr_text());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stderr = plugin.stderr_text();
    let fatal: Vec<Value> = error_lines(&stderr)
        .into_iter()
        .filter(|line| line["@message"] == "fatal")
        .collect();
    let [fatal] = &fatal[..] else {
        panic!("expected one fatal line:\n{stderr}");
    };
    let text = fatal["error"].as_str().expect("an error field");
    assert!(
        text.starts_with("the database lock was lost: ") && text.contains("terminating connection"),
        "{stderr}"
    );
    assert!(!plugin.socket.exists(), "{}", plugin.socket.display());
}

#[tokio::test(flavor = "multi_thread")]
async fn hardened_process_has_no_core_dumps_no_inspection_and_private_files() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    populate(&fake);
    let mut plugin = start_plugin(&sei_env(&fake, &database.url, PASSWORD));
    let proc_dir = format!("/proc/{}", plugin.child.id());

    let limits = std::fs::read_to_string(format!("{proc_dir}/limits")).expect("limits");
    let core: Vec<&str> = limits
        .lines()
        .find_map(|line| line.strip_prefix("Max core file size"))
        .expect("a core file size line")
        .split_whitespace()
        .collect();
    assert_eq!(core[..2], ["0", "0"], "{limits}");
    let status = std::fs::read_to_string(format!("{proc_dir}/status")).expect("status");
    assert!(
        status.lines().any(|line| line == "Umask:\t0077"),
        "{status}"
    );
    let mode = std::fs::metadata(&plugin.socket)
        .expect("socket metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "{mode:o}");
    if unsafe { libc::getuid() } != 0 {
        let environ = std::fs::read(format!("{proc_dir}/environ"))
            .expect_err("the environment of a process that cannot be inspected");
        assert_eq!(environ.kind(), std::io::ErrorKind::PermissionDenied);
    }
    stop_cleanly(&mut plugin).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn secrets_never_reach_logs_or_answers() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let client = admin(&database.url).await;
    client
        .batch_execute(&format!(
            "CREATE ROLE {DB_ROLE} LOGIN PASSWORD '{DB_PASSWORD}';
             GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {DB_ROLE};
             GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO {DB_ROLE};"
        ))
        .await
        .expect("create the role");
    let fake = FakeSei::start();
    populate(&fake);
    fake.fail_next(ROUTE_PROCESSES, 403, 1);
    let url = database_url_as(&database.url, DB_ROLE, DB_PASSWORD);
    let mut env = sei_env(&fake, &url, PASSWORD);
    with(&mut env, "LOG_LEVEL", "debug");
    with(&mut env, "POLL_INTERVAL", "5m");
    let mut plugin = start_plugin(&env);
    let mut plugin_calls = plugin_client(&plugin.socket).await;

    let done = status_where(
        &mut plugin,
        &mut plugin_calls,
        "a cycle succeeds",
        |status| status.last_success_at.is_some(),
    )
    .await;
    let check = plugin_calls.check().await.expect("check");
    let first = plugin_calls.events(0, 0).await.expect("events");
    let second = plugin_calls.events(0, 0).await.expect("events");
    drop(plugin_calls);
    stop_cleanly(&mut plugin).await;
    let payloads: Vec<String> = client
        .query("SELECT payload FROM events", &[])
        .await
        .expect("payloads")
        .iter()
        .map(|row| row.get(0))
        .collect();

    let tokens = fake.issued_tokens();
    assert!(tokens.len() >= 2, "{tokens:?}");
    assert!(!check.accepted);
    let shown = [
        plugin.stderr_text(),
        format!("{done:?}"),
        format!("{check:?}"),
        format!("{first:?}"),
        format!("{second:?}"),
        payloads.join("\n"),
    ];
    for text in &shown {
        assert!(!text.contains(PASSWORD), "{text}");
        assert!(!text.contains(DB_PASSWORD), "{text}");
        for token in &tokens {
            assert!(!text.contains(token.as_str()), "{text}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hubs_first_read_leaves_the_first_cycle_for_the_next_one() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let store = Store::new(&Secret::new(database.url.clone())).expect("store");
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(Duration::from_secs(15 * 60));
    h.fake.add_process(UNIT_NUPROC, process(9003, 3, "Ofício"));
    h.run_once().await;
    let history: Vec<String> = h
        .events()
        .await
        .iter()
        .map(|event| event.id.to_string())
        .collect();
    assert!(!history.is_empty());
    h.fake.add_process(UNIT_NUPROC, process(9004, 4, "Ofício"));
    let client = admin(&database.url).await;
    let mut plugin = start_plugin(&sei_env(&h.fake, &database.url, PASSWORD));
    until(
        &mut plugin,
        "the first cycle records the new process",
        || async {
            let recorded = count(
                &client,
                "SELECT count(*) FROM events WHERE procedure_id = 9004",
            )
            .await;
            (recorded > 0).then_some(())
        },
    )
    .await;
    let mut calls = plugin_client(&plugin.socket).await;

    let first = calls.events(0, 500).await.expect("first read");
    let last = first.last().expect("the history").seq;
    let second = calls.events(last, 500).await.expect("second read");

    let ids = |events: &[monitor_hub_plugin::proto::Event]| -> Vec<String> {
        events.iter().map(|event| event.id.clone()).collect()
    };
    assert_eq!(ids(&first), history);
    assert!(second.iter().all(|event| event.seq > last), "{second:?}");
    assert!(
        second
            .iter()
            .any(|event| event.kind == "process_entered"
                && event.url.ends_with("id_procedimento=9004")),
        "{second:?}"
    );
    drop(calls);
    stop_cleanly(&mut plugin).await;
}
