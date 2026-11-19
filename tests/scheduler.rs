mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use monitor_hub_plugin::Signals;
use sei_monitor::event::Kind;
use sei_monitor::monitor::{Clock, Monitor, MonitorError, State, TriggerError};
use sei_monitor::password::{PasswordCommandError, PasswordSource};
use sei_monitor::secret::Secret;
use sei_monitor::store::{AccountKey, CycleRun, Store, Trigger};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_postgres::NoTls;

use support::clock::PendingSleep;
use support::fake_sei::{
    FakeSei, ORG_ID, PASSWORD, ROUTE_LOGIN, ROUTE_LOGIN_PAGE, ROUTE_PROCESSES, ROUTE_VERSION,
    UNIT_NUPROC, USERNAME,
};
use support::harness::{
    FIFTEEN_MINUTES, Harness, bounded, config, database_store, no_jitter, populate, process,
};
use support::logs::{LoggedEvent, captured, logged};
use support::{next_signal, pending_signal};

const SETTLE: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(10);
const EVENTUALLY: Duration = Duration::from_secs(10);
const CHANGED_PASSWORD: &str = "changed-by-the-user";
const REJECTED: &str = "Usuário ou Senha Inválida.";
const RECORDED_REASON: &str = "SEI rejected the login (Usuário ou Senha Inválida.)";
const PAUSE_WRITE: &str = "UPDATE accounts SET paused_reason%";
const PROCESSES_READ: &str = "SELECT * FROM processes%";
const EARLIER_REASON: &str = "SEI rejected the login (in an earlier run)";
const SIGNALS_TO_READ: usize = 10;
const REFUSE_EVENTS_AT_COMMIT: &str = "CREATE FUNCTION refuse_events() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$;
     CREATE CONSTRAINT TRIGGER refuse_at_commit AFTER INSERT ON events DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_events();";

fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

struct Running(JoinHandle<()>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn spawn_run(monitor: &Arc<Monitor>) -> Running {
    Running(tokio::spawn(Arc::clone(monitor).run()))
}

async fn eventually<F: Future<Output = bool>>(mut check: impl FnMut() -> F) -> bool {
    let polled = async {
        while !check().await {
            tokio::time::sleep(POLL).await;
        }
    };
    tokio::time::timeout(EVENTUALLY, polled).await.is_ok()
}

async fn observed_waits(h: &Harness, count: usize) -> Vec<Duration> {
    let mut waits = Vec::new();
    for _ in 0..count {
        let sleep = h.clock.next_sleep().await;
        let expected = h.clock.now() + chrono::Duration::from_std(sleep.duration).expect("a wait");
        assert_eq!(h.monitor.state().next_cycle_at, Some(expected));
        waits.push(sleep.duration);
        h.clock.advance(sleep.duration);
        sleep.fire.send(()).expect("the monitor is waiting");
    }
    waits
}

async fn last_run(h: &Harness) -> CycleRun {
    h.store
        .last_cycle_run(h.account_id())
        .await
        .expect("last cycle run")
        .expect("a cycle ran")
}

fn account_key(h: &Harness) -> AccountKey {
    AccountKey {
        sei_base_url: h.fake.url(),
        username: USERNAME.to_owned(),
        org_id: ORG_ID,
    }
}

async fn stored_reason(h: &Harness) -> Option<String> {
    let account = h.store.ensure_account(&account_key(h), h.clock.now()).await;
    account.ok().map(|account| account.paused_reason)
}

async fn pause_recorded(h: &Harness) -> bool {
    eventually(|| async {
        let reason_stored = stored_reason(h)
            .await
            .is_some_and(|reason| !reason.is_empty());
        h.monitor.state().paused && reason_stored && !h.events().await.is_empty()
    })
    .await
}

async fn fire_pending_sleeps(h: &Harness, limit: usize) -> usize {
    let mut fired = 0;
    while fired < limit
        && let Ok(sleep) = tokio::time::timeout(SETTLE, h.clock.next_sleep()).await
    {
        fired += 1;
        h.clock.advance(sleep.duration);
        sleep.fire.send(()).expect("the monitor is waiting");
    }
    fired
}

async fn record_rejection(h: &Harness) -> String {
    record_reason(h, RECORDED_REASON).await;
    RECORDED_REASON.to_owned()
}

async fn record_reason(h: &Harness, reason: &str) {
    let account = h
        .store
        .ensure_account(&account_key(h), h.clock.now())
        .await
        .expect("account");
    h.store
        .set_paused_reason(account.id, reason)
        .await
        .expect("record the rejection");
}

fn another_monitor(h: &Harness) -> Arc<Monitor> {
    Monitor::new(
        config(&h.fake, PASSWORD),
        h.store.clone(),
        h.clock.clone(),
        no_jitter(),
    )
}

async fn held(monitor: &Monitor) -> bool {
    eventually(|| async {
        let state = monitor.state();
        state.paused && state.held
    })
    .await
}

fn store_that_never_connects() -> Store {
    Store::new(&Secret::new("postgres://sei@127.0.0.1:1/sei".to_owned()))
        .expect("a store that never connects")
}

fn messages<'a>(logs: &'a [LoggedEvent], message: &str) -> Vec<&'a LoggedEvent> {
    logs.iter()
        .filter(|event| {
            event
                .iter()
                .any(|(name, value)| name == "message" && value == message)
        })
        .collect()
}

#[tokio::test]
async fn run_backs_off_after_failures_and_then_keeps_the_interval() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.fail_next(ROUTE_PROCESSES, 502, 2);

    let (waits, logs) = captured(async {
        let _run = spawn_run(&h.monitor);
        let waits = observed_waits(&h, 3).await;
        h.clock.next_sleep().await;
        waits
    })
    .await;

    assert_eq!(
        waits,
        [Duration::from_secs(30), minutes(1), FIFTEEN_MINUTES]
    );
    let run = last_run(&h).await;
    assert_eq!((run.error.as_str(), run.processes_seen), ("", 2));
    let failure = |retry_in: &str| {
        logged(&[
            ("message", "cycle failed"),
            ("level", "WARN"),
            (
                "error",
                "sei: /processo/listar: answer outside the API format (HTTP 502)",
            ),
            ("retry_in", retry_in),
        ])
    };
    assert_eq!(
        messages(&logs, "cycle failed"),
        [&failure("30s"), &failure("1m0s")]
    );
}

#[tokio::test]
async fn backoff_is_capped_by_the_poll_interval() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.fail_next(ROUTE_PROCESSES, 502, 7);
    let _run = spawn_run(&h.monitor);

    let waits = observed_waits(&h, 7).await;

    assert_eq!(
        waits,
        [
            Duration::from_secs(30),
            minutes(1),
            minutes(2),
            minutes(4),
            minutes(8),
            FIFTEEN_MINUTES,
            FIFTEEN_MINUTES,
        ]
    );
}

#[tokio::test]
async fn rejected_login_pauses_without_trying_again() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let _run = spawn_run(&h.monitor);
    let scheduled = h.clock.next_sleep().await;
    h.fake.set_password(CHANGED_PASSWORD);
    h.clock.advance(scheduled.duration);
    scheduled.fire.send(()).expect("the monitor is waiting");

    let recorded = pause_recorded(&h).await;
    let retries = fire_pending_sleeps(&h, 3).await;
    h.clock.advance(Duration::from_secs(3600));
    let triggered = h.monitor.trigger_cycle();
    tokio::time::sleep(SETTLE).await;

    assert_eq!(h.fake.failed_logins(), 1, "after {retries} retries");
    assert_eq!(retries, 0, "the paused monitor scheduled another cycle");
    assert!(recorded, "the pause was never recorded");
    assert_eq!(triggered, Err(TriggerError::Paused));
    let state = h.monitor.state();
    assert!(state.paused_reason.contains(REJECTED), "{state:?}");
    assert!(!state.held);
    assert_eq!(state.next_cycle_at, None);
    assert_eq!(stored_reason(&h).await, Some(state.paused_reason.clone()));
    let events = h.events().await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, Kind::AccountLoginFailed);
    assert!(events[0].reference.starts_with("login:"), "{events:?}");
    assert_eq!(events[0].payload.reason, state.paused_reason);
    assert!(last_run(&h).await.error.contains(REJECTED));
}

#[tokio::test]
async fn manual_cycle_waits_for_the_minimum_interval() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let _run = spawn_run(&h.monitor);
    let _scheduled = h.clock.next_sleep().await;

    h.clock.advance(minutes(1));
    let early = h.monitor.trigger_cycle();
    h.clock.advance(minutes(4));
    let accepted = h.monitor.trigger_cycle();
    let queued_twice = h.monitor.trigger_cycle();
    h.clock.next_sleep().await;

    assert_eq!(
        early,
        Err(TriggerError::TooSoon {
            retry_after: minutes(4)
        })
    );
    assert_eq!(accepted, Ok(()));
    assert_eq!(queued_twice, Err(TriggerError::Running));
    let run = last_run(&h).await;
    assert_eq!((run.trigger, run.error.as_str()), (Trigger::Manual, ""));
}

#[tokio::test]
async fn run_waits_for_a_disabled_module() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.set_module_inactive(true);
    let _run = spawn_run(&h.monitor);

    let retry = h.clock.next_sleep().await;
    let waiting = h.monitor.state();
    h.fake.set_module_inactive(false);
    h.clock.advance(retry.duration);
    retry.fire.send(()).expect("the monitor is waiting");
    h.clock.next_sleep().await;

    assert_eq!(retry.duration, Duration::from_secs(30));
    assert_eq!(
        waiting.prepare_error.as_deref(),
        Some("sei: /versao: Módulo inativo.")
    );
    assert_eq!(h.monitor.state().prepare_error, None);
    let run = last_run(&h).await;
    assert_eq!((run.error.as_str(), run.processes_seen), ("", 2));
}

#[tokio::test]
async fn an_unknown_agency_fails_and_is_tried_again_after_each_back_off() {
    let fake = FakeSei::start();
    let mut monitor_config = config(&fake, PASSWORD);
    monitor_config.target.org = "NOPE".to_owned();
    let h = Harness::with(fake, store_that_never_connects(), monitor_config);

    let (attempts, logs) = captured(async {
        let _run = spawn_run(&h.monitor);
        let mut attempts = Vec::new();
        for _ in 0..3 {
            let retry = h.clock.next_sleep().await;
            let state = h.monitor.state();
            attempts.push((retry.duration, h.fake.calls(ROUTE_LOGIN_PAGE), state));
            h.clock.advance(retry.duration);
            retry.fire.send(()).expect("the monitor is waiting");
        }
        h.clock.next_sleep().await;
        attempts
    })
    .await;

    let waits: Vec<(Duration, usize)> = attempts
        .iter()
        .map(|(wait, pages, _)| (*wait, *pages))
        .collect();
    assert_eq!(
        waits,
        [
            (Duration::from_secs(30), 1),
            (minutes(1), 2),
            (minutes(2), 3)
        ]
    );
    for (_, _, state) in &attempts {
        let fatal = state.fatal.as_deref().expect("a fatal error");
        assert!(
            fatal.starts_with(r#"agency not found on the SEI login page: "NOPE" ("#),
            "{fatal}"
        );
        assert_eq!(state.account_id, 0);
    }
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (0, 0));
    let reported = messages(
        &logs,
        "sei-monitor cannot monitor SEI with this configuration; trying again with a back-off",
    );
    assert_eq!(reported.len(), 1, "{logs:?}");
}

#[tokio::test]
async fn a_maintenance_page_while_preparing_is_tried_again_until_sei_answers() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.respond_next(
        ROUTE_VERSION,
        503,
        &[("content-type", "text/html")],
        b"<html><body>Sistema em manutencao</body></html>".to_vec(),
    );
    let _run = spawn_run(&h.monitor);

    let retry = h.clock.next_sleep().await;
    let failing = h.monitor.state();
    let logins_while_failing = (h.fake.logins(), h.fake.failed_logins());
    h.clock.advance(retry.duration);
    retry.fire.send(()).expect("the monitor is waiting");
    let scheduled = h.clock.next_sleep().await;

    assert_eq!(retry.duration, Duration::from_secs(30));
    let fatal = failing.fatal.as_deref().expect("a fatal error");
    assert!(
        fatal.starts_with("the mod-wssei module was not found on this SEI: tried "),
        "{fatal}"
    );
    assert_eq!(logins_while_failing, (0, 0));
    assert_eq!(scheduled.duration, FIFTEEN_MINUTES);
    let state = h.monitor.state();
    assert_eq!((state.fatal, state.prepare_error), (None, None));
    let run = last_run(&h).await;
    assert_eq!((run.error.as_str(), run.processes_seen), ("", 2));
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (1, 0));
}

#[tokio::test]
async fn trigger_cycle_checks_in_order() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);

    let not_ready = h.monitor.trigger_cycle();
    h.fake.stall(ROUTE_PROCESSES);
    let run = spawn_run(&h.monitor);
    let stalled = eventually(|| async { h.monitor.state().running }).await;
    let running = h.monitor.trigger_cycle();
    drop(run);
    let released = eventually(|| async { !h.monitor.state().running }).await;

    assert_eq!(not_ready, Err(TriggerError::NotReady));
    assert!(stalled, "the cycle never started");
    assert_eq!(running, Err(TriggerError::Running));
    assert!(released, "an aborted run kept the monitor busy");
}

#[tokio::test]
async fn a_paused_monitor_refuses_cycles_and_new_runs() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.set_password(CHANGED_PASSWORD);
    let first = spawn_run(&h.monitor);
    assert!(pause_recorded(&h).await, "the pause was never recorded");

    let direct = bounded(h.monitor.run_once(Trigger::Manual)).await;
    drop(first);
    let _respawned = spawn_run(&h.monitor);
    let retries = fire_pending_sleeps(&h, 3).await;
    h.clock.advance(Duration::from_secs(3600));
    let triggered = h.monitor.trigger_cycle();
    tokio::time::sleep(SETTLE).await;

    assert_eq!(h.fake.failed_logins(), 1, "after {retries} retries");
    assert!(matches!(direct, Err(MonitorError::Paused)), "{direct:?}");
    assert_eq!(retries, 0, "the respawned run scheduled a cycle");
    assert_eq!(triggered, Err(TriggerError::Paused));
    let state = h.monitor.state();
    assert!(state.paused && !state.held, "{state:?}");
}

#[tokio::test]
async fn a_second_run_returns_while_the_first_is_alive() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let _first = spawn_run(&h.monitor);
    let scheduled = h.clock.next_sleep().await;
    let logins = h.fake.logins();

    bounded(tokio::spawn(Arc::clone(&h.monitor).run()))
        .await
        .expect("join the second run");
    h.clock.advance(scheduled.duration);
    scheduled.fire.send(()).expect("the first run is waiting");
    h.clock.next_sleep().await;

    assert_eq!(h.fake.logins(), logins);
    let run = last_run(&h).await;
    assert_eq!((run.error.as_str(), run.processes_seen), ("", 2));
}

#[tokio::test]
async fn a_recorded_rejection_holds_every_start_until_a_check_is_requested() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.set_password(CHANGED_PASSWORD);
    let reason = record_rejection(&h).await;

    let mut starts = Vec::new();
    let mut runs = Vec::new();
    let mut monitor = another_monitor(&h);
    for start in 0..5 {
        if start > 0 {
            h.clock.advance(Duration::from_secs(3600));
            monitor = another_monitor(&h);
        }
        runs.push(spawn_run(&monitor));
        starts.push((held(&monitor).await, fire_pending_sleeps(&h, 3).await));
        if start < 4 {
            runs.pop();
        }
    }
    let held_state = monitor.state();
    let logins = (h.fake.logins(), h.fake.failed_logins());
    let events = store
        .events_after(held_state.account_id, 0, 500)
        .await
        .expect("events");
    let checked = monitor.trigger_cycle();
    let rejected = eventually(|| async {
        let state = monitor.state();
        h.fake.failed_logins() > 0 && state.paused && !state.held
    })
    .await;
    let again = monitor.trigger_cycle();

    assert_eq!(logins, (0, 0));
    assert_eq!(starts, [(true, 0); 5]);
    let expected = State {
        paused: true,
        held: true,
        paused_reason: reason.clone(),
        next_cycle_at: None,
        ..held_state.clone()
    };
    assert_eq!(held_state, expected);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, Kind::AccountLoginFailed);
    assert_eq!(events[0].reference, "login-held:2026-11-04");
    assert_eq!(
        events[0].payload.reason,
        format!(
            "{reason}; still paused after a restart: request a check once the configuration is fixed"
        )
    );
    assert_eq!(checked, Ok(()));
    assert!(rejected, "the requested check never ran");
    assert_eq!(h.fake.failed_logins(), 1);
    assert_eq!(again, Err(TriggerError::Paused));
}

#[tokio::test]
async fn a_check_with_the_password_fixed_resumes_monitoring() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    record_rejection(&h).await;
    let _run = spawn_run(&h.monitor);
    let was_held = held(&h.monitor).await;

    let checked = h.monitor.trigger_cycle();
    let next = h.clock.next_sleep().await;

    assert!(was_held, "the monitor did not start held");
    assert_eq!(checked, Ok(()));
    assert_eq!(next.duration, FIFTEEN_MINUTES);
    let state = h.monitor.state();
    assert_eq!(
        (state.paused, state.held, state.paused_reason.as_str()),
        (false, false, "")
    );
    assert_eq!(stored_reason(&h).await.as_deref(), Some(""));
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (1, 0));
    let run = last_run(&h).await;
    assert_eq!((run.trigger, run.error.as_str()), (Trigger::Manual, ""));
}

#[tokio::test]
async fn a_login_accepted_by_a_check_clears_the_rejection_even_when_the_cycle_fails() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    record_rejection(&h).await;
    h.fake.fail_next(ROUTE_PROCESSES, 502, 1);
    let _run = spawn_run(&h.monitor);
    let was_held = held(&h.monitor).await;

    let checked = h.monitor.trigger_cycle();
    let retry = h.clock.next_sleep().await;
    let failed = (last_run(&h).await, h.monitor.state());
    let reason_after_the_failure = stored_reason(&h).await;
    let logins_after_the_failure = (h.fake.logins(), h.fake.failed_logins());
    h.clock.advance(retry.duration);
    retry.fire.send(()).expect("the monitor is waiting");
    let scheduled = h.clock.next_sleep().await;

    assert!(was_held, "the monitor did not start held");
    assert_eq!(checked, Ok(()));
    let (failed_run, failed_state) = failed;
    assert_eq!(failed_run.trigger, Trigger::Manual);
    assert!(failed_run.error.contains("HTTP 502"), "{failed_run:?}");
    assert_eq!(
        (
            failed_state.paused,
            failed_state.held,
            failed_state.paused_reason.as_str()
        ),
        (false, false, "")
    );
    assert_eq!(reason_after_the_failure.as_deref(), Some(""));
    assert_eq!(logins_after_the_failure, (1, 0));
    assert_eq!(retry.duration, Duration::from_secs(30));
    assert_eq!(scheduled.duration, FIFTEEN_MINUTES);
    let run = last_run(&h).await;
    assert_eq!((run.trigger, run.error.as_str()), (Trigger::Scheduled, ""));
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (1, 0));
}

#[tokio::test]
async fn a_check_whose_login_gets_no_verdict_keeps_the_rejection_on_hold() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let reason = record_rejection(&h).await;
    h.fake.fail_next(ROUTE_LOGIN, 502, 1);
    let _run = spawn_run(&h.monitor);
    let was_held = held(&h.monitor).await;

    let checked = h.monitor.trigger_cycle();
    let finished = eventually(|| async {
        let run = h.store.last_cycle_run(h.account_id()).await;
        run.is_ok_and(|run| run.is_some_and(|run| run.finished_at.is_some()))
    })
    .await;
    let fired = fire_pending_sleeps(&h, 3).await;
    let state = h.monitor.state();
    let stored = stored_reason(&h).await;
    let logins = (
        h.fake.calls(ROUTE_LOGIN),
        h.fake.logins(),
        h.fake.failed_logins(),
    );
    let too_soon = h.monitor.trigger_cycle();
    h.clock.advance(minutes(5));
    let again = h.monitor.trigger_cycle();

    assert!(was_held, "the monitor did not start held");
    assert_eq!(checked, Ok(()));
    assert!(finished, "the requested check never finished");
    assert_eq!(fired, 0, "the held monitor scheduled a cycle");
    assert_eq!(
        (state.paused, state.held, state.paused_reason.as_str()),
        (true, true, reason.as_str())
    );
    assert_eq!(stored, Some(reason));
    assert_eq!(logins, (1, 0, 0));
    assert_eq!(
        too_soon,
        Err(TriggerError::TooSoon {
            retry_after: minutes(5)
        })
    );
    assert_eq!(again, Ok(()));
}

async fn admin(url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("connect as the test superuser");
    tokio::spawn(connection);
    client
}

async fn reason_in(observer: &tokio_postgres::Client) -> String {
    observer
        .query_one("SELECT paused_reason FROM accounts", &[])
        .await
        .expect("the stored reason")
        .get(0)
}

#[tokio::test]
async fn a_failed_clear_of_the_rejection_is_tried_again_after_the_next_successful_cycle() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    record_rejection(&h).await;
    let _run = spawn_run(&h.monitor);
    let was_held = held(&h.monitor).await;
    let observer = admin(&database.url).await;
    let holder = admin(&database.url).await;
    holder
        .batch_execute("BEGIN; SELECT id FROM accounts FOR NO KEY UPDATE")
        .await
        .expect("hold the account row");

    let checked = h.monitor.trigger_cycle();
    let blocked = eventually(|| waiting_on(&observer, PAUSE_WRITE)).await;
    let gave_up = eventually(|| async { !waiting_on(&observer, PAUSE_WRITE).await }).await;
    let stored_while_held = reason_in(&observer).await;
    holder.batch_execute("ROLLBACK").await.expect("release");
    let scheduled = h.clock.next_sleep().await;

    assert!(was_held, "the monitor did not start held");
    assert_eq!(checked, Ok(()));
    assert!(blocked && gave_up, "the clear never ran into the lock");
    assert_eq!(stored_while_held, RECORDED_REASON);
    assert_eq!(scheduled.duration, FIFTEEN_MINUTES);
    let run = last_run(&h).await;
    assert_eq!((run.trigger, run.error.as_str()), (Trigger::Manual, ""));
    assert_eq!(reason_in(&observer).await, "");
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (1, 0));
}

async fn waiting_on(observer: &tokio_postgres::Client, pattern: &str) -> bool {
    let waiting: i64 = observer
        .query_one(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock' AND query LIKE $1",
            &[&pattern],
        )
        .await
        .expect("pg_stat_activity")
        .get(0);
    waiting > 0
}

#[tokio::test]
async fn a_pause_the_database_did_not_take_is_recorded_once_the_row_is_free() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.set_password(CHANGED_PASSWORD);
    let observer = admin(&database.url).await;
    let runs_holder = admin(&database.url).await;
    runs_holder
        .batch_execute("BEGIN; LOCK TABLE cycle_runs IN SHARE ROW EXCLUSIVE MODE")
        .await
        .expect("hold cycle_runs");
    let first = spawn_run(&h.monitor);
    let reached_the_cycle = eventually(|| waiting_on(&observer, "INSERT INTO cycle_runs%")).await;
    let account_holder = admin(&database.url).await;
    account_holder
        .batch_execute("BEGIN; SELECT id FROM accounts FOR NO KEY UPDATE")
        .await
        .expect("hold the account row");
    runs_holder
        .batch_execute("ROLLBACK")
        .await
        .expect("release");
    let write_blocked = eventually(|| waiting_on(&observer, PAUSE_WRITE)).await;
    let write_failed = eventually(|| async { !waiting_on(&observer, PAUSE_WRITE).await }).await;

    let retry = h.clock.next_sleep().await;
    let reason_while_held = reason_in(&observer).await;
    account_holder
        .batch_execute("ROLLBACK")
        .await
        .expect("release");
    h.clock.advance(retry.duration);
    retry.fire.send(()).expect("the monitor is waiting");
    let landed = eventually(|| async { !reason_in(&observer).await.is_empty() }).await;
    let more_retries = fire_pending_sleeps(&h, 3).await;
    let paused = h.monitor.state();
    drop(first);
    let restarted = another_monitor(&h);
    let _second = spawn_run(&restarted);
    let restarted_held = held(&restarted).await;

    assert!(reached_the_cycle, "the cycle never reached cycle_runs");
    assert!(
        write_blocked && write_failed,
        "the pause never ran into the lock"
    );
    assert_eq!(reason_while_held, "");
    assert_eq!(retry.duration, Duration::from_secs(30));
    assert!(landed, "the pause reason never landed");
    assert_eq!(more_retries, 0);
    assert!(paused.paused && !paused.held, "{paused:?}");
    assert_eq!(reason_in(&observer).await, paused.paused_reason);
    assert!(restarted_held, "the restarted monitor did not hold");
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (0, 1));
}

struct CancelledClear {
    first: Running,
    observer: tokio_postgres::Client,
    was_held: bool,
    checked: Result<(), TriggerError>,
    both_waiting: bool,
    gave_up: bool,
    stored: String,
    failed: State,
    retry: PendingSleep,
}

struct PasswordOnRelease(watch::Receiver<bool>);

impl PasswordSource for PasswordOnRelease {
    fn password(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, PasswordCommandError>> + Send + '_>> {
        let mut released = self.0.clone();
        Box::pin(async move {
            let _ = released.wait_for(|released| *released).await;
            Ok(Secret::new(PASSWORD.to_owned()))
        })
    }
}

fn harness_with_a_held_login(store: &Store) -> (Harness, watch::Sender<bool>) {
    let (release, released) = watch::channel(false);
    let h = Harness::new(store, |config| {
        config.target.password = Arc::new(PasswordOnRelease(released));
    });
    (h, release)
}

// The check's login waits until the processes read is blocked, so that read's lock timeout
// ends the cycle before the clear's own does, however fast the pool hands out connections.
async fn a_check_whose_clear_is_cancelled(
    h: &Harness,
    url: &str,
    reason: &str,
    release_login: &watch::Sender<bool>,
) -> CancelledClear {
    record_reason(h, reason).await;
    let first = spawn_run(&h.monitor);
    let was_held = held(&h.monitor).await;
    let observer = admin(url).await;
    let holder = admin(url).await;
    holder
        .batch_execute(
            "BEGIN; SELECT id FROM accounts FOR NO KEY UPDATE; LOCK TABLE processes IN ACCESS EXCLUSIVE MODE",
        )
        .await
        .expect("hold the account row and the processes table");

    let checked = h.monitor.trigger_cycle();
    let read_waiting = eventually(|| waiting_on(&observer, PROCESSES_READ)).await;
    release_login.send_replace(true);
    let both_waiting = read_waiting
        && eventually(|| async {
            waiting_on(&observer, PAUSE_WRITE).await && waiting_on(&observer, PROCESSES_READ).await
        })
        .await;
    let retry = h.clock.next_sleep().await;
    let failed = h.monitor.state();
    let gave_up = eventually(|| async { !waiting_on(&observer, PAUSE_WRITE).await }).await;
    let stored = reason_in(&observer).await;
    holder.batch_execute("ROLLBACK").await.expect("release");
    CancelledClear {
        first,
        observer,
        was_held,
        checked,
        both_waiting,
        gave_up,
        stored,
        failed,
        retry,
    }
}

fn assert_cancelled(cancelled: &CancelledClear, logs: &[LoggedEvent], reason: &str) {
    assert!(cancelled.was_held, "the monitor did not start held");
    assert_eq!(cancelled.checked, Ok(()));
    assert!(
        cancelled.both_waiting && cancelled.gave_up,
        "the clear and the processes read never ran into the locks"
    );
    let failed = &cancelled.failed;
    assert!(
        failed
            .cycle_error
            .as_deref()
            .is_some_and(|error| error.contains("lock timeout")),
        "{failed:?}"
    );
    assert_eq!(
        (failed.paused, failed.held, failed.paused_reason.as_str()),
        (false, false, "")
    );
    assert_eq!(cancelled.retry.duration, Duration::from_secs(30));
    assert_eq!(cancelled.stored, reason);
    assert!(
        messages(logs, "could not clear the pause reason").is_empty(),
        "the clear failed instead of being cancelled"
    );
}

#[tokio::test]
async fn a_cancelled_clear_of_the_rejection_is_tried_again_after_the_next_successful_cycle() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let (h, release_login) = harness_with_a_held_login(&store);
    populate(&h.fake);
    let (cancelled, logs) = captured(a_check_whose_clear_is_cancelled(
        &h,
        &database.url,
        RECORDED_REASON,
        &release_login,
    ))
    .await;
    assert_cancelled(&cancelled, &logs, RECORDED_REASON);
    let CancelledClear {
        first,
        observer,
        retry,
        ..
    } = cancelled;

    h.clock.advance(retry.duration);
    retry.fire.send(()).expect("the monitor is waiting");
    let scheduled = h.clock.next_sleep().await;
    let stored_after_success = reason_in(&observer).await;
    let logins = (h.fake.logins(), h.fake.failed_logins());
    let run = last_run(&h).await;
    drop(first);
    let restarted = another_monitor(&h);
    let _second = spawn_run(&restarted);
    let restarted_ran = eventually(|| async {
        let state = restarted.state();
        state.next_cycle_at.is_some() && !state.paused && !state.held
    })
    .await;

    assert_eq!(scheduled.duration, FIFTEEN_MINUTES);
    assert_eq!((run.trigger, run.error.as_str()), (Trigger::Scheduled, ""));
    assert_eq!(stored_after_success, "");
    assert_eq!(logins, (1, 0));
    assert!(restarted_ran, "{:?}", restarted.state());
    assert_eq!(reason_in(&observer).await, "");
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (2, 0));
}

#[tokio::test]
async fn a_rejection_recorded_after_a_cancelled_clear_stays_on_record() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let (h, release_login) = harness_with_a_held_login(&store);
    populate(&h.fake);
    let (cancelled, logs) = captured(a_check_whose_clear_is_cancelled(
        &h,
        &database.url,
        EARLIER_REASON,
        &release_login,
    ))
    .await;
    assert_cancelled(&cancelled, &logs, EARLIER_REASON);
    let CancelledClear {
        first,
        observer,
        retry,
        ..
    } = cancelled;

    h.fake.set_password(CHANGED_PASSWORD);
    h.clock.advance(retry.duration);
    retry.fire.send(()).expect("the monitor is waiting");
    let recorded = eventually(|| async {
        let state = h.monitor.state();
        state.paused && !state.held && reason_in(&observer).await == RECORDED_REASON
    })
    .await;
    let retries = fire_pending_sleeps(&h, 3).await;
    h.clock.advance(Duration::from_secs(3600));
    let triggered = h.monitor.trigger_cycle();
    tokio::time::sleep(SETTLE).await;
    let stored_after_the_trigger = reason_in(&observer).await;
    let events = h.events().await;
    drop(first);
    let restarted = another_monitor(&h);
    let _second = spawn_run(&restarted);
    let restarted_held = held(&restarted).await;

    assert!(recorded, "{:?}", h.monitor.state());
    assert_eq!(retries, 0, "the paused monitor scheduled another cycle");
    assert_eq!(triggered, Err(TriggerError::Paused));
    assert_eq!(stored_after_the_trigger, RECORDED_REASON);
    assert!(
        events
            .iter()
            .any(|event| event.kind == Kind::AccountLoginFailed
                && event.reference.starts_with("login:")
                && event.payload.reason == RECORDED_REASON),
        "{events:?}"
    );
    assert!(restarted_held, "the restarted monitor did not hold");
    assert_eq!(restarted.state().paused_reason, RECORDED_REASON);
    assert_eq!(reason_in(&observer).await, RECORDED_REASON);
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (1, 1));
}

async fn signal_until(signals: &mut Signals, mut awaited: impl FnMut(i64) -> bool) -> i64 {
    for _ in 0..SIGNALS_TO_READ {
        let seq = next_signal(signals).await;
        if awaited(seq) {
            return seq;
        }
    }
    panic!("none of {SIGNALS_TO_READ} signals was the one awaited");
}

async fn with_a_baseline(store: &Store) -> Harness {
    let h = Harness::new(store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(FIFTEEN_MINUTES);
    h.fake.add_process(UNIT_NUPROC, process(9003, 3, "Ofício"));
    h
}

#[tokio::test]
async fn a_committed_cycle_signals_the_newest_event_id() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_a_baseline(&store).await;
    let mut signals = h.monitor.notifier().subscribe();
    signals.latest_seq();

    h.run_once().await;
    let signalled = pending_signal(&mut signals).await;

    let newest = h
        .events()
        .await
        .iter()
        .map(|event| event.id)
        .max()
        .expect("the events of the second cycle");
    assert_eq!(signalled, Some(newest));
}

#[tokio::test]
async fn a_cycle_whose_commit_fails_signals_no_new_id() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = with_a_baseline(&store).await;
    admin(&database.url)
        .await
        .batch_execute(REFUSE_EVENTS_AT_COMMIT)
        .await
        .expect("a trigger that refuses every event at commit");
    let before = h.events().await;
    let mut signals = h.monitor.notifier().subscribe();
    let seen = signals.latest_seq();

    let outcome = bounded(h.monitor.run_once(Trigger::Scheduled)).await;

    assert!(
        matches!(outcome, Err(MonitorError::Store(_))),
        "{outcome:?}"
    );
    assert_eq!(signals.latest_seq(), seen);
    assert_eq!(h.events().await, before);
}

#[tokio::test]
async fn a_cycle_signals_when_it_starts_and_when_it_ends() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.fake.stall(ROUTE_PROCESSES);
    let mut signals = h.monitor.notifier().subscribe();
    signals.latest_seq();

    let monitor = Arc::clone(&h.monitor);
    let cycle = tokio::spawn(async move { monitor.run_once(Trigger::Scheduled).await });
    next_signal(&mut signals).await;
    let started = h.monitor.state();
    cycle.abort();
    let cancelled = cycle.await;
    next_signal(&mut signals).await;
    let ended = h.monitor.state();

    assert!(started.running, "{started:?}");
    assert!(
        cancelled
            .as_ref()
            .is_err_and(|failure| failure.is_cancelled()),
        "{cancelled:?}"
    );
    assert!(!ended.running, "{ended:?}");
}

#[tokio::test]
async fn a_rejected_login_signals_the_pause_and_its_event() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.set_password(CHANGED_PASSWORD);
    let mut signals = h.monitor.notifier().subscribe();
    signals.latest_seq();

    let _run = spawn_run(&h.monitor);
    let announced = signal_until(&mut signals, |seq| seq > 0).await;
    let state = h.monitor.state();
    let events = h.events().await;
    let retries = fire_pending_sleeps(&h, 3).await;

    let [failed] = &events[..] else {
        panic!("expected one event: {events:?}");
    };
    assert_eq!(failed.kind, Kind::AccountLoginFailed);
    assert!(failed.reference.starts_with("login:"), "{failed:?}");
    assert_eq!(announced, failed.id);
    assert!(state.paused && !state.held, "{state:?}");
    assert_eq!(h.fake.calls(ROUTE_LOGIN), 1);
    assert_eq!((h.fake.logins(), h.fake.failed_logins()), (0, 1));
    assert_eq!(retries, 0, "the paused monitor scheduled another cycle");
}

async fn held_harness(store: &Store) -> (Harness, String) {
    let h = Harness::new(store, |_| {});
    populate(&h.fake);
    h.fake.set_password(CHANGED_PASSWORD);
    let reason = record_rejection(&h).await;
    (h, reason)
}

#[tokio::test]
async fn starting_held_signals_the_hold_and_its_event() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let (h, reason) = held_harness(&store).await;
    let mut signals = h.monitor.notifier().subscribe();
    signals.latest_seq();

    let _run = spawn_run(&h.monitor);
    let announced = signal_until(&mut signals, |seq| seq > 0).await;
    let state = h.monitor.state();
    let events = h.events().await;

    let [held] = &events[..] else {
        panic!("expected one event: {events:?}");
    };
    assert_eq!(held.kind, Kind::AccountLoginFailed);
    assert_eq!(held.reference, "login-held:2026-11-04");
    assert_eq!(announced, held.id);
    assert!(state.paused && state.held, "{state:?}");
    assert_eq!(h.fake.calls(ROUTE_LOGIN), 0);
    assert_eq!(stored_reason(&h).await, Some(reason));
}

#[tokio::test]
async fn a_second_hold_on_the_same_day_signals_the_hold_without_a_new_id() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let (h, reason) = held_harness(&store).await;
    let mut first_signals = h.monitor.notifier().subscribe();
    first_signals.latest_seq();
    let first = spawn_run(&h.monitor);
    signal_until(&mut first_signals, |seq| seq > 0).await;
    drop(first);
    let restarted = another_monitor(&h);
    let mut signals = restarted.notifier().subscribe();
    signals.latest_seq();

    let mut second = spawn_run(&restarted);
    signal_until(&mut signals, |_| restarted.state().held).await;
    restarted.shut_down();
    bounded(&mut second.0)
        .await
        .expect("the held run ends on shut_down");
    let latest = signals.latest_seq();

    assert_eq!(latest, 0);
    assert_eq!(h.fake.calls(ROUTE_LOGIN), 0);
    let events = h.events().await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(stored_reason(&h).await, Some(reason));
}

#[tokio::test]
async fn a_failed_preparation_signals_its_error() {
    let h = Harness::new(&store_that_never_connects(), |_| {});
    h.fake.set_module_inactive(true);
    let mut signals = h.monitor.notifier().subscribe();
    signals.latest_seq();

    let _run = spawn_run(&h.monitor);
    next_signal(&mut signals).await;
    let state = h.monitor.state();

    assert_eq!(
        state.prepare_error.as_deref(),
        Some("sei: /versao: Módulo inativo.")
    );
    assert_eq!(state.account_id, 0);
    assert_eq!(h.fake.calls(ROUTE_LOGIN), 0);
}
