mod support;

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, TimeZone, Utc};
use monitor_hub_sdk::Monitor as _;
use monitor_hub_sdk::proto::{
    CheckRequest, CheckResponse, DescribeRequest, Event, EventKind, EventsRequest, Severity, State,
    StatusRequest, StatusResponse,
};
use prost_types::value::Kind as ValueKind;
use prost_types::{Struct, Timestamp};
use serde_json::{Map, Number, Value};
use tokio::task::JoinHandle;
use tokio_postgres::NoTls;
use tonic::{Code, Request};

use sei_monitor::event::Kind;
use sei_monitor::monitor::{Clock, Monitor};
use sei_monitor::secret::Secret;
use sei_monitor::server::SeiMonitor;
use sei_monitor::store::{AccountKey, Store, Trigger};

use support::fake_sei::{
    FakeSei, ORG_ID, PASSWORD, ROUTE_LOGIN, ROUTE_PROCESSES, UNIT_NUPROC, USERNAME, process_number,
};
use support::harness::{
    FIFTEEN_MINUTES, Harness, activity, bounded, config, database_store, document, no_jitter,
    populate, process, start, store_as, user,
};
use support::{database_url_as, login_role, pending_signal};

const POLL: Duration = Duration::from_millis(10);
const EVENTUALLY: Duration = Duration::from_secs(10);
const REJECTED: &str = "SEI rejected the login (Usuário ou Senha Inválida.)";
const GO_PAUSE: &str = "monitoring is paused because SEI rejected the login; restart sei-monitor after fixing the configuration";
const HELD: &str = "nothing is logging in: request a check once the configuration is fixed";
const PAUSED_HERE: &str = "restart the monitor with the configuration fixed, then request a check";
const NOT_REACHED: &str = "sei-monitor has not reached SEI yet";
const UNAVAILABLE: &str = "the database is unavailable";
const HUB_AHEAD: &str =
    "the hub is ahead of this database: reset the hub's position for the sei monitor";
const DB_ROLE: &str = "sei_server_locked_out";
const DB_PASSWORD: &str = "db-secret-7f3a";
const FLAPPING_ROLE: &str = "sei_events_flapping";

struct Running(JoinHandle<()>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn spawn_run(monitor: &Arc<Monitor>) -> Running {
    Running(tokio::spawn(Arc::clone(monitor).run()))
}

async fn eventually<F: Future<Output = bool>>(what: &str, mut check: impl FnMut() -> F) {
    let polled = async {
        while !check().await {
            tokio::time::sleep(POLL).await;
        }
    };
    tokio::time::timeout(EVENTUALLY, polled)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}"));
}

fn served(h: &Harness) -> SeiMonitor {
    SeiMonitor::new(Arc::clone(&h.monitor), false)
}

fn timestamp(at: DateTime<Utc>) -> Timestamp {
    Timestamp::from(SystemTime::from(at))
}

async fn events(served: &SeiMonitor, after: i64, limit: i32) -> Result<Vec<Event>, tonic::Status> {
    served
        .events(Request::new(EventsRequest { after, limit }))
        .await
        .map(|response| response.into_inner().events)
}

async fn page(served: &SeiMonitor, after: i64, limit: i32) -> Vec<Event> {
    events(served, after, limit).await.expect("events")
}

async fn status(served: &SeiMonitor) -> StatusResponse {
    served
        .status(Request::new(StatusRequest {}))
        .await
        .expect("status")
        .into_inner()
}

async fn check(served: &SeiMonitor) -> CheckResponse {
    served
        .check(Request::new(CheckRequest {}))
        .await
        .expect("check")
        .into_inner()
}

fn seqs(events: &[Event]) -> Vec<i64> {
    events.iter().map(|event| event.seq).collect()
}

fn kinds(events: &[Event]) -> Vec<&str> {
    events.iter().map(|event| event.kind.as_str()).collect()
}

fn json_of(payload: &Struct) -> Value {
    Value::Object(
        payload
            .fields
            .iter()
            .map(|(name, value)| (name.clone(), value_json(value)))
            .collect::<Map<String, Value>>(),
    )
}

fn value_json(value: &prost_types::Value) -> Value {
    match value.kind.as_ref() {
        None | Some(ValueKind::NullValue(_)) => Value::Null,
        Some(ValueKind::BoolValue(flag)) => Value::Bool(*flag),
        Some(ValueKind::NumberValue(number)) => {
            Number::from_f64(*number).map_or(Value::Null, Value::Number)
        }
        Some(ValueKind::StringValue(text)) => Value::String(text.clone()),
        Some(ValueKind::ListValue(list)) => {
            Value::Array(list.values.iter().map(value_json).collect())
        }
        Some(ValueKind::StructValue(inner)) => json_of(inner),
    }
}

async fn admin(url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("connect as the test superuser");
    tokio::spawn(connection);
    client
}

async fn last_issued(h: &Harness) -> i64 {
    h.store
        .last_issued_seq()
        .await
        .expect("the last issued seq")
}

async fn with_history(store: &Store) -> Harness {
    let h = Harness::new(store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(FIFTEEN_MINUTES);
    h.fake.remove_process(UNIT_NUPROC, 9002);
    h.fake.add_process(UNIT_NUPROC, process(9003, 3, "Ofício"));
    h.fake.add_activities(
        9003,
        vec![
            activity(70001, start(), "Processo gerado"),
            activity(70002, start(), "Processo remetido"),
        ],
    );
    h.fake
        .add_documents(9003, vec![document(90001, "Ofício 9")]);
    h.fake.update_process(UNIT_NUPROC, 9001, |process| {
        process.assignee = Some(user(202, "beltrano.silva", "Beltrano da Silva"));
    });
    h.fake.add_activities(
        9001,
        vec![activity(
            50004,
            start() + chrono::Duration::minutes(10),
            "Documento assinado",
        )],
    );
    h.fake
        .add_documents(9001, vec![document(80004, "Ofício 4")]);
    h.run_once().await;
    h
}

async fn add_one_event(h: &Harness) {
    h.clock.advance(FIFTEEN_MINUTES);
    h.fake.add_process(UNIT_NUPROC, process(9004, 4, "Ofício"));
    assert!(h.run_once().await.events_created > 0);
}

async fn insert_again(url: &str, id: i64) {
    admin(url)
        .await
        .execute(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             SELECT account_id, kind, procedure_id, ref, occurred_at, detected_at, payload
             FROM events WHERE id = $1
             ON CONFLICT (account_id, kind, ref) DO NOTHING",
            &[&id],
        )
        .await
        .expect("insert an event that exists");
}

async fn add_one_event_through_another_monitor(h: &Harness) {
    let other = Monitor::new(
        config(&h.fake, PASSWORD),
        h.store.clone(),
        h.clock.clone(),
        no_jitter(),
    );
    bounded(other.prepare()).await.expect("prepare");
    h.clock.advance(FIFTEEN_MINUTES);
    h.fake.add_process(UNIT_NUPROC, process(9004, 4, "Ofício"));
    let cycle = bounded(other.run_once(Trigger::Scheduled))
        .await
        .expect("a cycle of the other monitor");
    assert!(cycle.events_created > 0);
}

fn rejected_harness(store: &Store) -> Harness {
    let fake = FakeSei::start();
    populate(&fake);
    fake.set_password("changed-by-the-user");
    let monitor_config = config(&fake, PASSWORD);
    Harness::with(fake, store.clone(), monitor_config)
}

fn ids(h_events: &[sei_monitor::event::Event]) -> Vec<i64> {
    h_events.iter().map(|event| event.id).collect()
}

#[tokio::test]
async fn describe_names_the_monitor_and_its_kinds() {
    let store = Store::new(&Secret::new(
        "postgres://sei:unused@127.0.0.1:1/sei".to_owned(),
    ))
    .expect("a store that never connects");
    let h = Harness::new(&store, |_| {});

    let described = served(&h)
        .describe(Request::new(DescribeRequest {}))
        .await
        .expect("describe")
        .into_inner();

    assert_eq!(described.name, "sei");
    assert_eq!(described.display_name, "SEI");
    assert_eq!(described.version, "2.2.0");
    let expected: Vec<EventKind> = Kind::ALL
        .iter()
        .map(|kind| EventKind {
            id: kind.as_str().to_owned(),
            description: kind.description().to_owned(),
        })
        .collect();
    assert_eq!(described.kinds, expected);
    assert_eq!(described.kinds.len(), 8);
}

#[tokio::test]
async fn events_are_unavailable_until_the_account_is_known() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let history = with_history(&store).await;
    let confirmed = history.events().await[1].id;
    let h = Harness::new(&store, |_| {});
    let served = served(&h);

    let failure = events(&served, confirmed, 0)
        .await
        .expect_err("events before the account is known");

    assert_eq!(failure.code(), Code::Unavailable);
    assert_eq!(failure.message(), NOT_REACHED);
    assert_eq!(h.monitor.acked_seq(), confirmed);
}

#[tokio::test]
async fn events_map_the_stored_events() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let served = served(&h);

    let all = page(&served, 0, 0).await;

    assert_eq!(
        kinds(&all),
        [
            "assignment_changed",
            "return_due_soon",
            "process_entered",
            "process_left",
            "activity_added",
            "document_added",
        ]
    );
    assert!(
        all.windows(2).all(|pair| pair[0].seq < pair[1].seq),
        "{all:?}"
    );
    let stored = h.events().await;
    for (event, stored) in all.iter().zip(&stored) {
        assert_eq!(event.seq, stored.id);
        assert_eq!(event.id, stored.id.to_string());
        assert_eq!(event.severity, i32::from(Severity::Info));
        assert_eq!(event.body, "");
        assert!(!event.title.is_empty(), "{event:?}");
        assert_eq!(
            event.occurred_at,
            Some(timestamp(stored.occurred_at.expect("occurred_at")))
        );
    }
    assert_eq!(
        all[0].url,
        format!(
            "{}/sei/controlador.php?acao=procedimento_trabalhar&id_procedimento=9001",
            h.fake.url()
        )
    );
    let entered = json_of(all[2].payload.as_ref().expect("a payload"));
    assert_eq!(entered["procedure_id"], 9003.0);
    assert_eq!(entered["process_number"], process_number(3).as_str());
    assert_eq!(entered["unit"], "SEDEMO/NUPROC");
    assert!(entered["detail"].is_object(), "{entered}");
    for event in &all {
        let payload = json_of(event.payload.as_ref().expect("a payload")).to_string();
        assert!(!payload.contains(USERNAME), "{payload}");
    }
}

#[tokio::test]
async fn events_follow_after_and_limit() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let served = served(&h);
    let all = seqs(&page(&served, 0, 0).await);

    assert_eq!(seqs(&page(&served, all[1], 0).await), all[2..]);
    assert_eq!(h.monitor.acked_seq(), all[1]);
    assert_eq!(seqs(&page(&served, 0, 2).await), all[..2]);
    assert_eq!(h.monitor.acked_seq(), all[1]);
}

#[tokio::test]
async fn events_clamp_the_limit() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    admin(&database.url)
        .await
        .execute(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             SELECT $1, 'process_left', 9001, 'r' || n, $2, $2, '{}'
             FROM generate_series(1, 600) AS n",
            &[&h.account_id(), &start()],
        )
        .await
        .expect("insert events");
    let served = served(&h);

    let default = page(&served, 0, 0).await;
    assert_eq!(default.len(), 100);
    assert_eq!(page(&served, 0, -3).await, default);
    assert_eq!(seqs(&page(&served, 0, 7).await), seqs(&default)[..7]);
    let full = page(&served, 0, i32::MAX).await;
    assert_eq!(full.len(), 500);
    assert_eq!(seqs(&full)[..100], seqs(&default));
    assert_eq!(page(&served, full[499].seq, 500).await.len(), 100);
}

#[tokio::test]
async fn full_detail_puts_the_details_in_the_body() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let served = SeiMonitor::new(Arc::clone(&h.monitor), true);

    let all = page(&served, 0, 0).await;

    let added = all
        .iter()
        .find(|event| event.kind == "document_added")
        .expect("a document_added event");
    assert!(added.body.starts_with("When: "), "{:?}", added.body);
}

#[tokio::test]
async fn login_failure_is_urgent_and_explains_itself() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = rejected_harness(&store);
    let served = served(&h);
    let _run = spawn_run(&h.monitor);
    eventually("the login failure is recorded", || async {
        h.monitor.state().paused && !h.events().await.is_empty()
    })
    .await;

    let all = page(&served, 0, 0).await;

    let [failed] = &all[..] else {
        panic!("expected one event: {all:?}");
    };
    assert_eq!(failed.kind, "account_login_failed");
    assert_eq!(failed.severity, i32::from(Severity::Urgent));
    assert_eq!(failed.url, "");
    let lines: Vec<&str> = failed.body.lines().collect();
    assert_eq!(lines.len(), 2, "{:?}", failed.body);
    assert!(
        lines[0].starts_with(&format!("Reason: {REJECTED}")),
        "{:?}",
        failed.body
    );
}

#[tokio::test]
async fn events_of_another_account_are_left_out() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let other = store
        .ensure_account(
            &AccountKey {
                sei_base_url: "https://sei.example".to_owned(),
                username: "outro".to_owned(),
                org_id: 1,
            },
            start(),
        )
        .await
        .expect("another account");
    let client = admin(&database.url).await;
    for n in 0..3 {
        client
            .execute(
                "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
                 VALUES ($1, 'process_left', 1, $2, $3, $3, '{}')",
                &[&other.id, &format!("other-{n}"), &start()],
            )
            .await
            .expect("insert an event of another account");
    }
    add_one_event(&h).await;
    let ours = ids(&h.events().await);

    assert_eq!(seqs(&page(&served(&h), 0, 0).await), ours);
}

#[tokio::test]
async fn status_follows_the_monitor() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let served = served(&h);

    let before = status(&served).await;
    assert_eq!(before.state, i32::from(State::Failing));
    assert!(before.message.starts_with(NOT_REACHED), "{before:?}");
    assert_eq!(
        (
            before.last_check_started_at,
            before.last_success_at,
            before.next_check_at
        ),
        (None, None, None)
    );

    let _run = spawn_run(&h.monitor);
    let scheduled = h.clock.next_sleep().await;
    let finished = h
        .store
        .last_successful_cycle_run(h.account_id())
        .await
        .expect("last run")
        .expect("a successful run")
        .finished_at
        .expect("finished");
    assert_eq!(
        status(&served).await,
        StatusResponse {
            state: State::Idle.into(),
            message: String::new(),
            last_check_started_at: Some(timestamp(start())),
            last_success_at: Some(timestamp(finished)),
            next_check_at: Some(timestamp(start() + chrono::Duration::minutes(15))),
        }
    );

    h.fake.set_password("changed-by-the-user");
    h.clock.advance(scheduled.duration);
    scheduled.fire.send(()).expect("the monitor is waiting");
    eventually("the monitor pauses", || async { h.monitor.state().paused }).await;
    let paused = status(&served).await;
    assert_eq!(paused.state, i32::from(State::Paused));
    assert_eq!(paused.message, format!("{REJECTED}; {PAUSED_HERE}"));
    assert_eq!(paused.next_check_at, None);
    assert_eq!(paused.last_success_at, Some(timestamp(finished)));
}

#[tokio::test]
async fn status_shows_a_failing_cycle_until_one_succeeds() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.fail_next(ROUTE_PROCESSES, 500, 1);
    let served = served(&h);
    let _run = spawn_run(&h.monitor);

    let retry = h.clock.next_sleep().await;
    let failing = status(&served).await;
    h.clock.advance(retry.duration);
    retry.fire.send(()).expect("the monitor is waiting");
    h.clock.next_sleep().await;
    let recovered = status(&served).await;

    assert_eq!(failing.state, i32::from(State::Failing));
    assert_eq!(
        failing.message,
        "sei: /processo/listar: answer outside the API format (HTTP 500)"
    );
    assert_eq!(failing.last_success_at, None);
    assert_eq!(
        failing.next_check_at,
        Some(timestamp(start() + chrono::Duration::seconds(30)))
    );
    assert_eq!(recovered.state, i32::from(State::Idle));
    assert_eq!(recovered.message, "");
    assert!(recovered.last_success_at.is_some(), "{recovered:?}");
}

#[tokio::test]
async fn status_shows_events_that_cannot_be_read() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let blind = store_as(&database, "sei_events_denied", "SELECT ON events").await;
    let monitor = Monitor::new(
        config(&h.fake, PASSWORD),
        blind,
        h.clock.clone(),
        no_jitter(),
    );
    bounded(monitor.prepare()).await.expect("prepare");
    let served = SeiMonitor::new(Arc::clone(&monitor), false);
    let readable = status(&served).await;

    let failure = events(&served, 0, 0)
        .await
        .expect_err("events the role cannot read");
    let failing = status(&served).await;

    assert_eq!(readable.state, i32::from(State::Idle), "{readable:?}");
    assert_eq!(failure.code(), Code::Unavailable);
    assert_eq!(failure.message(), UNAVAILABLE);
    assert_eq!(failing.state, i32::from(State::Failing));
    assert_eq!(failing.message, UNAVAILABLE);
}

#[tokio::test]
async fn status_shows_a_cycle_in_progress() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.stall(ROUTE_PROCESSES);
    let served = served(&h);
    let _run = spawn_run(&h.monitor);
    eventually("the cycle asks for the processes", || async {
        h.fake.calls(ROUTE_PROCESSES) > 0
    })
    .await;

    let checking = status(&served).await;

    assert_eq!(
        checking,
        StatusResponse {
            state: State::Checking.into(),
            message: String::new(),
            last_check_started_at: Some(timestamp(start())),
            last_success_at: None,
            next_check_at: None,
        }
    );
}

#[tokio::test]
async fn status_explains_why_sei_is_not_reached() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let waiting = Harness::new(&store, |_| {});
    waiting.fake.set_module_inactive(true);
    let unknown = Harness::new(&store, |config| config.target.org = "NOPE".to_owned());
    let _waiting = spawn_run(&waiting.monitor);
    let _unknown = spawn_run(&unknown.monitor);
    eventually("both monitors fail to prepare", || async {
        waiting.monitor.state().prepare_error.is_some() && unknown.monitor.state().fatal.is_some()
    })
    .await;

    let retrying = status(&served(&waiting)).await;
    let stopped = status(&served(&unknown)).await;

    assert_eq!(retrying.state, i32::from(State::Failing));
    assert_eq!(
        retrying.message,
        format!("{NOT_REACHED}: sei: /versao: Módulo inativo.")
    );
    assert_eq!(stopped.state, i32::from(State::Failing));
    assert!(
        stopped
            .message
            .starts_with("agency not found on the SEI login page: \"NOPE\" ("),
        "{stopped:?}"
    );
}

#[tokio::test]
async fn check_maps_the_trigger_errors() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let served = served(&h);

    assert_eq!(
        check(&served).await,
        CheckResponse {
            accepted: false,
            message: NOT_REACHED.to_owned(),
        }
    );
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(Duration::from_secs(60));
    assert_eq!(
        check(&served).await,
        CheckResponse {
            accepted: false,
            message: "the last cycle started less than 5m0s ago; try again in 4m0s".to_owned(),
        }
    );
    h.clock.advance(Duration::from_secs(4 * 60));
    assert_eq!(
        check(&served).await,
        CheckResponse {
            accepted: true,
            message: String::new(),
        }
    );

    let rejected = rejected_harness(&store);
    let _run = spawn_run(&rejected.monitor);
    eventually("the monitor pauses", || async {
        rejected.monitor.state().paused
    })
    .await;
    assert_eq!(
        check(&SeiMonitor::new(Arc::clone(&rejected.monitor), false)).await,
        CheckResponse {
            accepted: false,
            message: format!("{GO_PAUSE}, then request a check"),
        }
    );
}

#[tokio::test]
async fn a_held_monitor_says_how_to_resume_and_accepts_one_check() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = rejected_harness(&store);
    let account = store
        .ensure_account(
            &AccountKey {
                sei_base_url: h.monitor.config().target.sei_url.clone(),
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
    let served = served(&h);
    let _run = spawn_run(&h.monitor);
    eventually("the monitor holds", || async { h.monitor.state().held }).await;

    let held = status(&served).await;
    assert_eq!(held.state, i32::from(State::Paused));
    assert_eq!(held.message, format!("{REJECTED}; {HELD}"));
    assert_eq!(held.next_check_at, None);
    assert_eq!(h.fake.failed_logins(), 0);

    assert_eq!(
        check(&served).await,
        CheckResponse {
            accepted: true,
            message: String::new(),
        }
    );
    eventually("the check is rejected by SEI", || async {
        let state = h.monitor.state();
        state.paused && !state.held
    })
    .await;
    assert_eq!(h.fake.failed_logins(), 1);
    let paused = status(&served).await;
    assert_eq!(paused.state, i32::from(State::Paused));
    assert_eq!(paused.message, format!("{REJECTED}; {PAUSED_HERE}"));
}

#[tokio::test]
async fn database_failures_never_show_the_password() {
    let Some((database, _store)) = database_store().await else {
        return;
    };
    let client = admin(&database.url).await;
    client
        .batch_execute(&format!(
            "{}
             GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {DB_ROLE};
             GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO {DB_ROLE};",
            login_role(DB_ROLE, DB_PASSWORD)
        ))
        .await
        .expect("create the role");
    let url = database_url_as(&database.url, DB_ROLE, DB_PASSWORD);
    let store = Store::new(&Secret::new(url)).expect("store");
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    let served = served(&h);
    assert_eq!(status(&served).await.state, i32::from(State::Idle));

    client
        .batch_execute(&format!(
            "ALTER ROLE {DB_ROLE} NOLOGIN;
             SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = '{DB_ROLE}';"
        ))
        .await
        .expect("lock the role out");

    let failure = events(&served, 0, 0)
        .await
        .expect_err("events from a database that refuses the role");
    assert_eq!(failure.code(), Code::Unavailable);
    assert_eq!(failure.message(), UNAVAILABLE);
    let failing = status(&served).await;
    assert_eq!(failing.state, i32::from(State::Failing));
    assert_eq!(failing.message, UNAVAILABLE);
    for shown in [format!("{failure:?}"), format!("{failing:?}")] {
        assert!(!shown.contains(DB_PASSWORD), "{shown}");
    }
}

#[tokio::test]
async fn the_first_read_holds_only_the_events_from_before_the_start() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let history = ids(&h.events().await);
    let served = served(&h).with_first_read_boundary(last_issued(&h).await);
    add_one_event(&h).await;
    let all = seqs(&page(&SeiMonitor::new(Arc::clone(&h.monitor), false), 0, 500).await);
    assert_eq!(all.len(), history.len() + 1);

    assert_eq!(seqs(&page(&served, 0, 500).await), history);
    assert_eq!(
        seqs(&page(&served, *history.last().expect("history"), 500).await),
        all[history.len()..]
    );
}

#[tokio::test]
async fn the_first_read_ends_with_its_first_short_page() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let served = served(&h).with_first_read_boundary(last_issued(&h).await);
    add_one_event(&h).await;
    let all = seqs(&page(&SeiMonitor::new(Arc::clone(&h.monitor), false), 0, 500).await);
    assert_eq!(all.len(), 7);

    assert_eq!(seqs(&page(&served, 0, 2).await), all[..2]);
    assert_eq!(seqs(&page(&served, all[1], 2).await), all[2..4]);
    assert_eq!(seqs(&page(&served, all[3], 2).await), all[4..6]);
    assert_eq!(seqs(&page(&served, all[5], 2).await), Vec::<i64>::new());
    assert_eq!(seqs(&page(&served, all[5], 2).await), all[6..]);
}

#[tokio::test]
async fn on_a_new_database_the_first_read_is_empty() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let served = served(&h).with_first_read_boundary(0);
    h.prepare().await;
    h.run_once().await;
    add_one_event(&h).await;

    let stored = ids(&h.events().await);
    assert!(!stored.is_empty());

    assert_eq!(page(&served, 0, 0).await, []);
    assert_eq!(seqs(&page(&served, 0, 0).await), stored);
}

#[tokio::test]
async fn a_hub_ahead_of_the_database_gets_nothing_and_confirms_nothing() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let newest = last_issued(&h).await;
    let served = served(&h);
    assert_eq!(status(&served).await.state, i32::from(State::Idle));

    assert_eq!(page(&served, newest + 50, 0).await, []);
    assert_eq!(h.monitor.acked_seq(), 0);
    let ahead = status(&served).await;
    assert_eq!(ahead.state, i32::from(State::Failing));
    assert_eq!(ahead.message, HUB_AHEAD);

    assert_eq!(page(&served, newest, 0).await, []);
    assert_eq!(h.monitor.acked_seq(), newest);
    let in_line = status(&served).await;
    assert_eq!(in_line.state, i32::from(State::Idle));
    assert_eq!(in_line.message, "");
}

#[tokio::test]
async fn an_activity_time_outside_the_years_1_to_9999_is_announced_at_detection() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(FIFTEEN_MINUTES);
    let past_9999 = Utc
        .with_ymd_and_hms(10000, 1, 1, 3, 30, 0)
        .single()
        .expect("an instant in year 10000");
    h.fake
        .add_activities(9001, vec![activity(50009, past_9999, "Processo futuro")]);
    h.run_once().await;
    let detected = h.clock.now();
    admin(&database.url)
        .await
        .execute(
            "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
             VALUES ($1, 'activity_added', 9001, 'stored-by-go', $2, $3, '{}')",
            &[&h.account_id(), &past_9999, &detected],
        )
        .await
        .expect("insert an event stored with a time past year 9999");

    let all = page(&served(&h), 0, 0).await;

    let stored: Vec<_> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.reference == "50009")
        .map(|event| event.occurred_at)
        .collect();
    assert_eq!(stored, [Some(detected)]);
    let added: Vec<&Event> = all
        .iter()
        .filter(|event| event.kind == "activity_added")
        .collect();
    assert_eq!(added.len(), 2, "{all:?}");
    for event in added {
        assert_eq!(event.occurred_at, Some(timestamp(detected)), "{event:?}");
    }
}

#[tokio::test]
async fn long_sei_text_is_cut_where_it_enters() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(FIFTEEN_MINUTES);
    let long = "Ação ".repeat(400);
    h.fake.add_process(
        UNIT_NUPROC,
        support::fake_sei::FakeProcess {
            description: long.clone(),
            ..process(9005, 5, &long)
        },
    );
    h.run_once().await;

    let row = h
        .store
        .processes(h.account_id())
        .await
        .expect("processes")
        .into_iter()
        .find(|row| row.procedure_id == 9005)
        .expect("the new process");
    assert_eq!(row.description.chars().count(), 500);
    assert_eq!(row.process_type.chars().count(), 500);
    assert!(long.starts_with(&row.description));
    let entered = h
        .events()
        .await
        .into_iter()
        .find(|event| event.procedure_id == 9005)
        .expect("the process_entered event");
    assert_eq!(entered.payload.description, row.description);
    assert_eq!(entered.payload.process_type, row.process_type);
}

#[tokio::test]
async fn a_long_rejection_from_sei_is_cut_in_the_pause_and_in_status() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let message = format!("Usuário ou Senha Inválida. {}", "ç".repeat(2000));
    let body = serde_json::json!({ "sucesso": false, "mensagem": message }).to_string();
    h.fake.respond_next(
        ROUTE_LOGIN,
        401,
        &[("Content-Type", "application/json")],
        body.into_bytes(),
    );
    let served = served(&h);
    let _run = spawn_run(&h.monitor);
    eventually("the monitor pauses", || async { h.monitor.state().paused }).await;

    let reason = h.monitor.state().paused_reason;
    let shown = status(&served).await.message;

    let cut: String = message.chars().take(500).collect();
    assert_eq!(reason, format!("SEI rejected the login ({cut})"));
    assert_eq!(shown.chars().count(), 500);
    assert!(reason.starts_with(&shown), "{shown}");
}

#[tokio::test]
async fn the_end_of_the_first_read_signals_the_events_it_held() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let boundary = last_issued(&h).await;
    let served = served(&h).with_first_read_boundary(boundary);
    add_one_event_through_another_monitor(&h).await;
    let all = seqs(&page(&SeiMonitor::new(Arc::clone(&h.monitor), false), 0, 500).await);
    assert_eq!(all.len(), 7);
    let newest = all[6];
    insert_again(&database.url, newest).await;
    assert_eq!(last_issued(&h).await, newest + 1);
    let logins = h.fake.calls(ROUTE_LOGIN);
    let mut signals = h.monitor.notifier().subscribe();
    let known = signals.latest_seq();

    let full = page(&served, 0, 2).await;
    let after_a_full_page = pending_signal(&mut signals).await;
    let short = page(&served, all[1], 500).await;
    let after_the_short_page = pending_signal(&mut signals).await;
    let later = page(&served, all[5], 500).await;
    let after_a_later_read = pending_signal(&mut signals).await;

    assert_eq!(seqs(&full), all[..2]);
    assert_eq!(after_a_full_page, None);
    assert_eq!(seqs(&short), all[2..6]);
    assert!(newest > boundary, "{newest} {boundary}");
    assert!(known < newest, "{known} {newest}");
    assert_eq!(after_the_short_page, Some(newest));
    assert_eq!(seqs(&later), all[6..]);
    assert_eq!(after_a_later_read, None);
    assert_eq!(h.fake.calls(ROUTE_LOGIN), logins);
}

#[tokio::test]
async fn the_hub_falling_ahead_and_back_in_line_signals_the_status() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let newest = last_issued(&h).await;
    let served = served(&h);
    h.clock.advance(FIFTEEN_MINUTES);
    let logins = h.fake.calls(ROUTE_LOGIN);
    let mut signals = h.monitor.notifier().subscribe();
    signals.latest_seq();

    assert_eq!(page(&served, newest + 50, 0).await, []);
    let fell_ahead = pending_signal(&mut signals).await;
    let ahead = status(&served).await;
    assert_eq!(page(&served, newest + 50, 0).await, []);
    let still_ahead = pending_signal(&mut signals).await;
    assert_eq!(page(&served, newest, 0).await, []);
    let back_in_line = pending_signal(&mut signals).await;
    let in_line = status(&served).await;

    assert!(fell_ahead.is_some());
    assert_eq!(ahead.state, i32::from(State::Failing));
    assert_eq!(ahead.message, HUB_AHEAD);
    assert_eq!(still_ahead, None);
    assert!(back_in_line.is_some());
    assert_eq!(in_line.state, i32::from(State::Idle));
    assert_eq!(in_line.message, "");
    assert_eq!(h.monitor.trigger_cycle(), Ok(()));
    assert_eq!(h.fake.calls(ROUTE_LOGIN), logins);
}

#[tokio::test]
async fn a_failing_read_of_the_events_and_its_recovery_signal_the_status() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let blind = store_as(&database, FLAPPING_ROLE, "SELECT ON events").await;
    let superuser = admin(&database.url).await;
    superuser
        .batch_execute(&format!("GRANT SELECT ON events_id_seq TO {FLAPPING_ROLE}"))
        .await
        .expect("let the role read the event counter");
    let monitor = Monitor::new(
        config(&h.fake, PASSWORD),
        blind,
        h.clock.clone(),
        no_jitter(),
    );
    bounded(monitor.prepare()).await.expect("prepare");
    let served = SeiMonitor::new(Arc::clone(&monitor), false);
    let logins = h.fake.calls(ROUTE_LOGIN);
    let mut signals = monitor.notifier().subscribe();
    signals.latest_seq();

    let failure = events(&served, 0, 0)
        .await
        .expect_err("events the role cannot read");
    let failed = pending_signal(&mut signals).await;
    let failing = status(&served).await;
    events(&served, 0, 0)
        .await
        .expect_err("events the role still cannot read");
    let failed_again = pending_signal(&mut signals).await;
    superuser
        .batch_execute(&format!("GRANT SELECT ON events TO {FLAPPING_ROLE}"))
        .await
        .expect("give the role the events back");
    let recovered_page = page(&served, 0, 0).await;
    let recovered = pending_signal(&mut signals).await;
    let readable = status(&served).await;

    assert_eq!(failure.code(), Code::Unavailable);
    assert_eq!(failure.message(), UNAVAILABLE);
    assert!(failed.is_some());
    assert_eq!(failing.state, i32::from(State::Failing));
    assert_eq!(failing.message, UNAVAILABLE);
    assert_eq!(failed_again, None);
    assert_eq!(seqs(&recovered_page), ids(&h.events().await));
    assert!(recovered.is_some());
    assert_eq!(readable.state, i32::from(State::Idle), "{readable:?}");
    assert_eq!(monitor.trigger_cycle(), Ok(()));
    assert_eq!(h.fake.calls(ROUTE_LOGIN), logins);
}

#[tokio::test]
async fn the_end_of_the_first_read_starts_no_cycle() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_history(&store).await;
    let served = served(&h).with_first_read_boundary(last_issued(&h).await);
    h.clock.advance(FIFTEEN_MINUTES);
    let logins = h.fake.calls(ROUTE_LOGIN);

    let short = page(&served, 0, 500).await;

    assert!(!short.is_empty());
    assert_eq!(h.monitor.trigger_cycle(), Ok(()));
    assert_eq!(h.fake.calls(ROUTE_LOGIN), logins);
}
