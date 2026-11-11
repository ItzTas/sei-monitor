mod support;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use sei_monitor::event::Kind;
use sei_monitor::monitor::{Clock, Monitor, MonitorError, TriggerError};
use sei_monitor::secret::Secret;
use sei_monitor::store::{AccountKey, CycleRun, Store, Trigger};
use tokio::task::JoinHandle;

use support::fake_sei::{FakeSei, ORG_ID, PASSWORD, ROUTE_PROCESSES, USERNAME};
use support::harness::{FIFTEEN_MINUTES, Harness, bounded, config, database_store, populate};
use support::logs::{LoggedEvent, captured, logged};

const SETTLE: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(10);
const EVENTUALLY: Duration = Duration::from_secs(10);
const CHANGED_PASSWORD: &str = "changed-by-the-user";
const REJECTED: &str = "Usuário ou Senha Inválida.";

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
    h.fake.set_password(CHANGED_PASSWORD);
    let _run = spawn_run(&h.monitor);

    let paused = eventually(|| async { h.monitor.state().paused }).await;
    let mut retries = 0;
    while retries < 3
        && let Ok(sleep) = tokio::time::timeout(SETTLE, h.clock.next_sleep()).await
    {
        retries += 1;
        h.clock.advance(sleep.duration);
        sleep.fire.send(()).expect("the monitor is waiting");
    }
    h.clock.advance(Duration::from_secs(3600));
    let triggered = h.monitor.trigger_cycle();
    tokio::time::sleep(SETTLE).await;

    assert_eq!(h.fake.failed_logins(), 1, "after {retries} retries");
    assert_eq!(retries, 0, "the paused monitor scheduled another cycle");
    assert!(paused, "the monitor never paused");
    assert_eq!(triggered, Err(TriggerError::Paused));
    let state = h.monitor.state();
    assert!(state.paused_reason.contains(REJECTED), "{state:?}");
    assert_eq!(state.next_cycle_at, None);

    let key = AccountKey {
        sei_base_url: h.fake.url(),
        username: USERNAME.to_owned(),
        org_id: ORG_ID,
    };
    let recorded = eventually(|| async {
        let account = store.ensure_account(&key, h.clock.now()).await;
        let reason_stored = account.is_ok_and(|account| !account.paused_reason.is_empty());
        reason_stored && !h.events().await.is_empty()
    })
    .await;
    assert!(recorded, "the pause was never recorded");
    let account = store
        .ensure_account(&key, h.clock.now())
        .await
        .expect("account");
    assert_eq!(account.paused_reason, state.paused_reason);
    let events = h.events().await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, Kind::AccountLoginFailed);
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
async fn run_stops_preparing_on_an_unknown_agency() {
    let fake = FakeSei::start();
    let mut monitor_config = config(&fake, PASSWORD);
    monitor_config.target.org = "NOPE".to_owned();
    let h = Harness::with(fake, store_that_never_connects(), monitor_config);

    bounded(tokio::spawn(Arc::clone(&h.monitor).run()))
        .await
        .expect("join the run task");

    let state = h.monitor.state();
    let fatal = state.fatal.expect("a fatal error");
    assert!(
        fatal.contains(r#"agency not found on the SEI login page: "NOPE""#),
        "{fatal}"
    );
    assert_eq!(state.account_id, 0);
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
    tokio::time::sleep(SETTLE).await;

    assert_eq!(h.fake.failed_logins(), 1, "after {retries} retries");
    assert!(matches!(direct, Err(MonitorError::Paused)), "{direct:?}");
    assert_eq!(retries, 0, "the respawned run scheduled a cycle");
    assert!(h.monitor.state().paused);
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
