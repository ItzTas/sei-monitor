mod support;

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use sei_monitor::event::{Event, Kind, Payload};
use sei_monitor::monitor::diff::CurrentUser;
use sei_monitor::monitor::{Clock, CycleResult, MonitorError, login_failure};
use sei_monitor::password::StaticPassword;
use sei_monitor::secret::Secret;
use sei_monitor::sei::{ApiError, Client, Credentials, ErrorKind, SeiError};
use sei_monitor::store::{AccountInfo, AccountKey, Store, Trigger};
use sei_monitor::timezone::load_zone;
use support::clock::FakeClock;
use support::fake_sei::{
    FakeActivity, FakeDocument, FakeProcess, FakeSei, MODULE_VERSION, ORG_ACRONYM, ORG_ID,
    PASSWORD, ROUTE_ACTIVITIES, ROUTE_DOCUMENTS, ROUTE_LOGIN, ROUTE_LOGIN_PAGE, ROUTE_PROCESSES,
    ROUTE_VERSION, SEI_VERSION, UNIT_GAB, UNIT_NUPROC, USER_ID, USERNAME, process_number,
};
use support::harness::{
    FIFTEEN_MINUTES, Harness, activity, bounded, config, database_store, document, hours, options,
    populate, process, start, store_as, user,
};

const CYCLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/golden/cycle.json"
));
const MONITOR_TEXT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/golden/monitor_text.json"
));
const SETTLE: Duration = Duration::from_millis(100);
const CHANGED_PASSWORD: &str = "changed-by-the-user";
const GO_ZERO_TIME: &str = "0001-01-01T00:00:00Z";

fn instant(text: &str) -> Option<DateTime<Utc>> {
    if text == GO_ZERO_TIME {
        return None;
    }
    let at = DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|error| panic!("timestamp {text:?}: {error}"));
    Some(at.to_utc())
}

fn kinds_and_refs(events: &[Event]) -> Vec<(Kind, String)> {
    events
        .iter()
        .map(|event| (event.kind, event.reference.clone()))
        .collect()
}

fn numbered_documents(ids: std::ops::RangeInclusive<i64>) -> Vec<FakeDocument> {
    ids.map(|id| document(id, &format!("Documento {id}")))
        .collect()
}

fn single_process(fake: &FakeSei) {
    fake.set_processes(UNIT_NUPROC, vec![process(9001, 1, "Contrato")]);
    fake.add_activities(
        9001,
        vec![activity(50000, start() - hours(24), "Processo gerado")],
    );
}

#[tokio::test]
async fn state_knows_the_user_acronym_before_the_first_login() {
    let fake = FakeSei::start();
    let monitor_config = config(&fake, PASSWORD);
    let store = Store::new(&Secret::new("postgres://sei@127.0.0.1:1/sei".to_owned()))
        .expect("a store that never connects");
    let harness = Harness::with(fake, store, monitor_config);

    assert_eq!(
        harness.monitor.state().user,
        CurrentUser {
            user_id: 0,
            acronym: USERNAME.to_owned(),
        }
    );
}

#[tokio::test]
async fn first_cycle_is_a_silent_baseline() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;

    let result = h.run_once().await;

    assert_eq!(
        result,
        CycleResult {
            processes_seen: 2,
            events_created: 0,
        }
    );
    let state = h.monitor.state();
    assert_eq!(state.units, ["SEDEMO/NUPROC"]);
    assert_eq!(
        state.user,
        CurrentUser {
            user_id: USER_ID,
            acronym: USERNAME.to_owned(),
        }
    );
    assert!(!state.running);
    assert_eq!(state.last_started_at, Some(start()));
    assert_eq!(h.events().await, []);
    let processes = store.processes(h.account_id()).await.expect("processes");
    assert_eq!(processes.len(), 2, "{processes:?}");
    assert_eq!(processes[0].procedure_id, 9001);
    assert_eq!(processes[0].last_activity_id, 50003);
    assert_eq!(processes[0].document_total, 3);
    assert_eq!(processes[0].assigned_user, USERNAME);
    assert_eq!(processes[0].assigned_user_id, USER_ID);
    assert_eq!(processes[0].return_date, "2026-11-05");
    assert_eq!(processes[0].unit_acronym, "SEDEMO/NUPROC");
    assert_eq!(processes[0].first_seen_at, Some(start()));
    assert_eq!(processes[1].last_activity_id, 60001);
    let documents = store
        .document_ids(h.account_id(), 9001)
        .await
        .expect("documents");
    assert_eq!(documents.len(), 3);
    let run = store
        .last_cycle_run(h.account_id())
        .await
        .expect("last run")
        .expect("a run");
    assert_eq!(run.error, "");
    assert_eq!(run.processes_seen, 2);
    assert_eq!(run.trigger, Trigger::Scheduled);
    assert!(run.finished_at.is_some());
    let key = AccountKey {
        sei_base_url: h.fake.url(),
        username: USERNAME.to_owned(),
        org_id: ORG_ID,
    };
    let account = store.ensure_account(&key, start()).await.expect("account");
    assert_eq!(account.baseline_at, Some(start()));
    assert_eq!(
        account.info,
        AccountInfo {
            org_acronym: ORG_ACRONYM.to_owned(),
            sei_version: SEI_VERSION.to_owned(),
            wssei_version: MODULE_VERSION.to_owned(),
        }
    );
}

#[tokio::test]
async fn next_cycle_reports_what_changed() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
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
    let new_activity = start() + chrono::Duration::minutes(10);
    h.fake.add_activities(
        9001,
        vec![activity(50004, new_activity, "Documento assinado")],
    );
    h.fake
        .add_documents(9001, vec![document(80004, "Ofício 4")]);

    let result = h.run_once().await;

    let cycle = "2026-11-04T18:15:00Z";
    let events = h.events().await;
    assert_eq!(
        kinds_and_refs(&events),
        [
            (Kind::AssignmentChanged, format!("9001:{cycle}")),
            (Kind::ReturnDueSoon, "9001:2026-11-05".to_owned()),
            (Kind::ProcessEntered, format!("9003:{cycle}")),
            (Kind::ProcessLeft, format!("9002:{cycle}")),
            (Kind::ActivityAdded, "50004".to_owned()),
            (Kind::DocumentAdded, "80004".to_owned()),
        ]
    );
    assert_eq!(
        result,
        CycleResult {
            processes_seen: 2,
            events_created: 6,
        }
    );
    let activity_event = &events[4];
    assert_eq!(activity_event.occurred_at, Some(new_activity));
    assert_eq!(activity_event.payload.activity_text, "Documento assinado");
    assert_eq!(activity_event.payload.unit, "SEDEMO/NUPROC");
    let document_event = &events[5];
    assert_eq!(document_event.payload.document_name, "Ofício 4");
    assert_eq!(document_event.payload.process_number, process_number(1));

    h.clock.advance(FIFTEEN_MINUTES);
    assert_eq!(h.run_once().await.events_created, 0);
}

#[tokio::test]
async fn expired_session_does_not_stop_the_cycle() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;

    h.fake.expire_tokens();
    h.fake.add_activities(
        9002,
        vec![activity(60002, start(), "Processo recebido na unidade")],
    );
    h.run_once().await;

    assert!(kinds_and_refs(&h.events().await).contains(&(Kind::ActivityAdded, "60002".to_owned())));
    assert_eq!(h.fake.logins(), 2);
}

#[tokio::test]
async fn refused_process_keeps_its_state_until_the_next_cycle() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.concurrency = 1);
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    h.fake
        .add_activities(9001, vec![activity(50004, start(), "Documento assinado")]);
    h.fake.add_activities(
        9002,
        vec![activity(60002, start(), "Processo recebido na unidade")],
    );

    h.fake.fail_next(ROUTE_ACTIVITIES, 200, 1);
    h.clock.advance(FIFTEEN_MINUTES);
    let first = h.run_once().await;
    let after_refusal = store.processes(h.account_id()).await.expect("processes");
    h.clock.advance(FIFTEEN_MINUTES);
    let second = h.run_once().await;

    assert_eq!(h.activity_refs().await, ["60002", "50004"]);
    assert_eq!(first.processes_seen, 2);
    assert_eq!(after_refusal[0].last_activity_id, 50003);
    assert_eq!(after_refusal[0].document_total, 3);
    assert!(second.events_created > 0, "{second:?}");
}

fn watch(fake: &FakeSei, unit_id: i64, numbers: std::ops::RangeInclusive<u32>) {
    let processes: Vec<FakeProcess> = numbers
        .map(|n| process(9000 + i64::from(n), n, "Contrato"))
        .collect();
    for watched in &processes {
        fake.add_activities(
            watched.id,
            vec![activity(
                watched.id * 10,
                start() - hours(1),
                "Processo gerado",
            )],
        );
    }
    fake.set_processes(unit_id, processes);
}

fn assert_rejected_login(error: &MonitorError) {
    let MonitorError::Sei(error) = error else {
        panic!("a cycle after a password change failed with {error:?}");
    };
    assert_eq!(error.kind(), Some(ErrorKind::InvalidCredentials), "{error}");
}

#[tokio::test]
async fn a_changed_password_costs_one_rejected_login() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    for round in 0..20 {
        let h = Harness::new(&store, |config| config.all_units = true);
        watch(&h.fake, UNIT_NUPROC, 1..=4);
        watch(&h.fake, UNIT_GAB, 5..=7);
        h.prepare().await;
        h.run_once().await;

        h.fake.set_password(CHANGED_PASSWORD);
        h.fake.expire_tokens();
        let error = bounded(h.monitor.run_once(Trigger::Scheduled))
            .await
            .expect_err("a cycle with a changed password");
        tokio::time::sleep(SETTLE).await;

        assert_rejected_login(&error);
        assert_eq!(h.fake.failed_logins(), 1, "round {round}");
    }
}

async fn canned_process_listing(fake: &FakeSei) -> Vec<u8> {
    let credentials = Credentials {
        username: USERNAME.to_owned(),
        password: Arc::new(StaticPassword(Secret::new(PASSWORD.to_owned()))),
        org_id: ORG_ID,
    };
    let client = Client::new(&fake.api_url(), credentials, &options(fake));
    let query = [("limit", "50".to_owned()), ("start", "0".to_owned())];
    let page = bounded(client.raw(UNIT_NUPROC, ROUTE_PROCESSES, &query))
        .await
        .expect("the process listing");
    json!({"sucesso": true, "data": page.data, "total": page.total})
        .to_string()
        .into_bytes()
}

#[tokio::test]
async fn a_password_changed_before_the_details_costs_one_rejected_login() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    for round in 0..20 {
        let h = Harness::new(&store, |_| {});
        watch(&h.fake, UNIT_NUPROC, 1..=7);
        h.prepare().await;
        h.run_once().await;

        let listing = canned_process_listing(&h.fake).await;
        let json_type = [("Content-Type", "application/json")];
        h.fake
            .respond_next(ROUTE_PROCESSES, 200, &json_type, listing);
        h.fake.set_password(CHANGED_PASSWORD);
        let activities_before = h.fake.calls(ROUTE_ACTIVITIES);
        let error = bounded(h.monitor.run_once(Trigger::Scheduled))
            .await
            .expect_err("a cycle with a changed password");
        tokio::time::sleep(SETTLE).await;

        assert_rejected_login(&error);
        assert!(h.fake.calls(ROUTE_ACTIVITIES) > activities_before);
        assert_eq!(h.fake.failed_logins(), 1, "round {round}");
    }
}

#[tokio::test]
async fn activities_beyond_the_first_page_are_all_reported() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    single_process(&h.fake);
    h.prepare().await;
    h.run_once().await;

    let added: Vec<FakeActivity> = (1..=25)
        .map(|n| activity(50000 + n, start(), &format!("Andamento {n}")))
        .collect();
    h.fake.add_activities(9001, added);
    let calls = h.fake.calls(ROUTE_ACTIVITIES);
    h.clock.advance(FIFTEEN_MINUTES);
    let result = h.run_once().await;

    let expected: Vec<String> = (50001..=50025).map(|id| id.to_string()).collect();
    assert_eq!(h.activity_refs().await, expected);
    assert_eq!(result.events_created, 25);
    assert_eq!(h.fake.calls(ROUTE_ACTIVITIES) - calls, 2);
    let processes = store.processes(h.account_id()).await.expect("processes");
    assert_eq!(processes[0].last_activity_id, 50025);
}

#[tokio::test]
async fn activities_stop_after_ten_pages() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    single_process(&h.fake);
    h.prepare().await;
    h.run_once().await;

    let added: Vec<FakeActivity> = (1..=205)
        .map(|n| activity(50000 + n, start(), &format!("Andamento {n}")))
        .collect();
    h.fake.add_activities(9001, added);
    let calls = h.fake.calls(ROUTE_ACTIVITIES);
    h.clock.advance(FIFTEEN_MINUTES);
    h.run_once().await;

    let expected: Vec<String> = (50006..=50205).map(|id| id.to_string()).collect();
    assert_eq!(h.activity_refs().await, expected);
    assert_eq!(h.fake.calls(ROUTE_ACTIVITIES) - calls, 10);
    let processes = store.processes(h.account_id()).await.expect("processes");
    assert_eq!(processes[0].last_activity_id, 50205);
}

async fn with_documents(store: &Store, documents: Vec<FakeDocument>) -> Harness {
    let h = Harness::new(store, |_| {});
    single_process(&h.fake);
    h.fake.add_documents(9001, documents);
    h.prepare().await;
    h.run_once().await;
    h.fake
        .add_activities(9001, vec![activity(50001, start(), "Documento incluído")]);
    h.clock.advance(FIFTEEN_MINUTES);
    h
}

async fn document_refs(h: &Harness) -> Vec<String> {
    h.events()
        .await
        .into_iter()
        .filter(|event| event.kind == Kind::DocumentAdded)
        .map(|event| event.reference)
        .collect()
}

async fn document_total(h: &Harness) -> i64 {
    store_processes(h).await[0].document_total
}

async fn store_processes(h: &Harness) -> Vec<sei_monitor::store::ProcessRow> {
    h.store.processes(h.account_id()).await.expect("processes")
}

#[tokio::test]
async fn documents_resume_from_the_page_of_the_saved_total() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_documents(&store, numbered_documents(80001..=80120)).await;
    assert_eq!(document_total(&h).await, 120);
    h.fake
        .add_documents(9001, vec![document(80121, "Ofício 121")]);
    let calls = h.fake.calls(ROUTE_DOCUMENTS);

    h.run_once().await;

    assert_eq!(h.fake.calls(ROUTE_DOCUMENTS) - calls, 1);
    assert_eq!(document_refs(&h).await, ["80121"]);
    assert_eq!(document_total(&h).await, 121);
}

#[tokio::test]
async fn documents_are_read_again_from_the_new_total_when_the_saved_page_is_empty() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = with_documents(&store, numbered_documents(80001..=80120)).await;
    for id in 80021..=80120 {
        h.fake.remove_document(9001, id);
    }
    h.fake
        .add_documents(9001, vec![document(80121, "Ofício 121")]);
    let calls = h.fake.calls(ROUTE_DOCUMENTS);

    h.run_once().await;

    assert_eq!(h.fake.calls(ROUTE_DOCUMENTS) - calls, 2);
    assert_eq!(document_refs(&h).await, ["80121"]);
    assert_eq!(document_total(&h).await, 21);
}

async fn after_empty_document_pages(store: &Store, empty_pages: usize) -> (Harness, usize) {
    let h = with_documents(store, numbered_documents(80001..=80120)).await;
    h.fake
        .add_documents(9001, vec![document(80121, "Ofício 121")]);
    let json_type = [("Content-Type", "application/json")];
    for _ in 0..empty_pages {
        let empty = br#"{"sucesso":true,"data":[],"total":121}"#.to_vec();
        h.fake.respond_next(ROUTE_DOCUMENTS, 200, &json_type, empty);
    }
    let calls = h.fake.calls(ROUTE_DOCUMENTS);
    h.run_once().await;
    let made = h.fake.calls(ROUTE_DOCUMENTS) - calls;
    (h, made)
}

#[tokio::test]
async fn an_empty_saved_page_is_read_again_from_the_page_of_the_new_total() {
    let Some((_database, store)) = database_store().await else {
        return;
    };

    let (h, calls) = after_empty_document_pages(&store, 1).await;

    assert_eq!(calls, 2);
    assert_eq!(document_refs(&h).await, ["80121"]);
    assert_eq!(document_total(&h).await, 121);
}

#[tokio::test]
async fn a_saved_page_that_stays_empty_is_read_from_the_first_page() {
    let Some((_database, store)) = database_store().await else {
        return;
    };

    let (h, calls) = after_empty_document_pages(&store, 2).await;

    assert_eq!(calls, 5);
    assert_eq!(document_refs(&h).await, ["80121"]);
    assert_eq!(document_total(&h).await, 121);
}

#[tokio::test]
async fn a_cycle_runs_on_a_spawned_task() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let monitor = Arc::clone(&h.monitor);

    let cycle = tokio::spawn(async move {
        monitor.prepare().await?;
        monitor.run_once(Trigger::Scheduled).await
    });

    let result = bounded(cycle).await.expect("join the cycle task");
    assert_eq!(result.expect("cycle").processes_seen, 2);
}

#[tokio::test]
async fn a_successful_cycle_clears_the_stored_pause_reason() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    let key = AccountKey {
        sei_base_url: h.fake.url(),
        username: USERNAME.to_owned(),
        org_id: ORG_ID,
    };
    let reason = "SEI rejected the login (Usuário ou Senha Inválida.)";
    let account = store.ensure_account(&key, start()).await.expect("account");
    store
        .set_paused_reason(account.id, reason)
        .await
        .expect("pause reason");
    h.prepare().await;
    assert_eq!(h.monitor.state().paused_reason, reason);

    h.run_once().await;

    assert_eq!(h.monitor.state().paused_reason, "");
    let stored = store.ensure_account(&key, start()).await.expect("account");
    assert_eq!(stored.paused_reason, "");
}

#[tokio::test]
async fn prepare_reads_the_agency_acronym_when_the_org_is_given_by_id() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.target.org = ORG_ID.to_string());

    h.prepare().await;

    let key = AccountKey {
        sei_base_url: h.fake.url(),
        username: USERNAME.to_owned(),
        org_id: ORG_ID,
    };
    let account = store.ensure_account(&key, start()).await.expect("account");
    assert_eq!(account.id, h.account_id());
    assert_eq!(account.info.org_acronym, ORG_ACRONYM);
}

#[tokio::test]
async fn long_versions_and_agency_acronym_are_cut_where_they_enter() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.target.org = ORG_ID.to_string());
    populate(&h.fake);
    let long = |label: &str| format!("{label}-{}", "ç".repeat(2000));
    let versions =
        json!({ "sucesso": true, "data": { "sei": long("sei"), "wssei": long("wssei") } });
    let page = format!(
        r#"<html><body><select id="selOrgao" name="selOrgao"><option value="{ORG_ID}">{}</option></select></body></html>"#,
        long("ORG")
    );
    for _ in 0..2 {
        h.fake.respond_next(
            ROUTE_VERSION,
            200,
            &[("Content-Type", "application/json")],
            versions.to_string().into_bytes(),
        );
        h.fake.respond_next(
            ROUTE_LOGIN_PAGE,
            200,
            &[("Content-Type", "text/html; charset=utf-8")],
            page.clone().into_bytes(),
        );
    }

    h.prepare().await;
    h.run_once().await;

    let key = AccountKey {
        sei_base_url: h.fake.url(),
        username: USERNAME.to_owned(),
        org_id: ORG_ID,
    };
    let info = store
        .ensure_account(&key, start())
        .await
        .expect("account")
        .info;
    let cut = |text: String| text.chars().take(500).collect::<String>();
    assert_eq!(info.org_acronym, cut(long("ORG")));
    assert_eq!(info.sei_version, cut(long("sei")));
    assert_eq!(info.wssei_version, cut(long("wssei")));
}

#[tokio::test]
async fn run_once_needs_a_prepared_monitor_and_runs_one_cycle_at_a_time() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);

    let not_ready = h.monitor.run_once(Trigger::Manual).await;
    assert!(
        matches!(not_ready, Err(MonitorError::NotReady)),
        "{not_ready:?}"
    );
    assert_eq!(
        not_ready.unwrap_err().to_string(),
        "sei-monitor has not reached SEI yet"
    );

    h.prepare().await;
    let mut first = Box::pin(h.monitor.run_once(Trigger::Scheduled));
    let overlapping = tokio::select! {
        biased;
        _ = &mut first => panic!("the first cycle finished before the second started"),
        second = h.monitor.run_once(Trigger::Manual) => second,
    };
    assert!(h.monitor.state().running);
    assert!(
        matches!(overlapping, Err(MonitorError::Running)),
        "{overlapping:?}"
    );
    assert_eq!(
        overlapping.unwrap_err().to_string(),
        "a cycle is already running"
    );
    drop(first);

    assert!(!h.monitor.state().running);
    assert_eq!(h.run_once().await.processes_seen, 2);
}

#[tokio::test]
async fn a_failed_cycle_records_its_error_and_frees_the_monitor() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.prepare().await;
    h.fake.fail_next(ROUTE_PROCESSES, 502, 1);

    let error = bounded(h.monitor.run_once(Trigger::Scheduled))
        .await
        .expect_err("a cycle against a failing listing");

    assert_eq!(
        error.to_string(),
        "sei: /processo/listar: answer outside the API format (HTTP 502)"
    );
    assert!(!h.monitor.state().running);
    let run = store
        .last_cycle_run(h.account_id())
        .await
        .expect("last run")
        .expect("a run");
    assert_eq!(
        run.error,
        "sei: /processo/listar: answer outside the API format (HTTP 502)"
    );
    assert_eq!(run.finished_at, Some(start()));
    assert_eq!(run.processes_seen, 0);
    assert_eq!(h.run_once().await.processes_seen, 2);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CycleFile {
    notes: String,
    #[serde(rename = "defaults")]
    _defaults: Value,
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    name: String,
    config: ScenarioConfig,
    cycles: Vec<RecordedCycle>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioConfig {
    all_units: bool,
    units: Vec<String>,
    concurrency: usize,
    return_warning_days: i64,
    retention_days: i64,
    poll_interval_ns: u64,
    channels: Vec<String>,
    trigger: String,
    org: String,
    username: String,
    zone: String,
    prepare_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedCycle {
    at: String,
    setup: Vec<Value>,
    result: RecordedResult,
    events: Vec<RecordedEvent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedResult {
    processes_seen: usize,
    events_created: usize,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct RecordedEvent {
    kind: String,
    procedure_id: i64,
    #[serde(rename = "ref")]
    reference: String,
    occurred_at: String,
    detected_at: String,
    payload: Payload,
}

fn assert_event(event: &Event, recorded: &RecordedEvent, context: &str) {
    assert_eq!(event.kind.as_str(), recorded.kind, "{context}");
    assert_eq!(event.procedure_id, recorded.procedure_id, "{context}");
    assert_eq!(event.reference, recorded.reference, "{context}");
    assert_eq!(
        event.occurred_at,
        instant(&recorded.occurred_at),
        "{context}"
    );
    assert_eq!(
        event.detected_at,
        instant(&recorded.detected_at),
        "{context}"
    );
    assert_eq!(event.payload, recorded.payload, "{context}");
}

fn trigger_named(name: &str) -> Trigger {
    [Trigger::Scheduled, Trigger::Manual]
        .into_iter()
        .find(|trigger| trigger.as_str() == name)
        .unwrap_or_else(|| panic!("unknown trigger {name:?} in a vector"))
}

fn scenario_harness(store: &Store, recorded: &ScenarioConfig) -> Harness {
    assert_eq!(recorded.channels, ["webhook"], "channels are not ported");
    Harness::new(store, |config| {
        config.all_units = recorded.all_units;
        config.units.clone_from(&recorded.units);
        config.concurrency = recorded.concurrency;
        config.return_warning_days = recorded.return_warning_days;
        config.retention_days = recorded.retention_days;
        config.poll_interval = Duration::from_nanos(recorded.poll_interval_ns);
        config.target.org.clone_from(&recorded.org);
        config.target.username.clone_from(&recorded.username);
        config.zone = load_zone(&recorded.zone).expect("the scenario zone");
    })
}

fn advance_to(clock: &FakeClock, text: &str) {
    let at = instant(text).expect("a cycle time");
    let by = (at - clock.now())
        .to_std()
        .unwrap_or_else(|_| panic!("{text} is before the clock"));
    clock.advance(by);
}

#[tokio::test]
async fn cycles_match_go() {
    let file: CycleFile = serde_json::from_str(CYCLE).expect("cycle.json");
    assert!(
        file.notes
            .contains("fresh fake SEI server and an empty database")
    );
    assert!(!file.scenarios.is_empty());
    for scenario in &file.scenarios {
        let Some((_database, store)) = database_store().await else {
            return;
        };
        let h = scenario_harness(&store, &scenario.config);
        let trigger = trigger_named(&scenario.config.trigger);
        advance_to(&h.clock, &scenario.config.prepare_at);
        h.prepare().await;
        let mut last_seen = 0;
        for (index, cycle) in scenario.cycles.iter().enumerate() {
            let context = format!("{} cycle {index}", scenario.name);
            advance_to(&h.clock, &cycle.at);
            h.fake.apply(&cycle.setup).expect("setup calls");

            let result = bounded(h.monitor.run_once(trigger))
                .await
                .unwrap_or_else(|error| panic!("{context}: {error}"));

            assert_eq!(
                result,
                CycleResult {
                    processes_seen: cycle.result.processes_seen,
                    events_created: cycle.result.events_created,
                },
                "{context}"
            );
            let events = store
                .events_after(h.account_id(), last_seen, 500)
                .await
                .expect("events");
            assert_eq!(events.len(), cycle.events.len(), "{context}: {events:#?}");
            for (position, (event, recorded)) in events.iter().zip(&cycle.events).enumerate() {
                assert_event(event, recorded, &format!("{context} event {position}"));
            }
            last_seen = events.last().map_or(last_seen, |event| event.id);
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginFailureCase {
    name: String,
    now: String,
    source: LoginSource,
    api_error: RecordedApiError,
    error: String,
    chain: Vec<ErrorLink>,
    reason: String,
    event: RecordedEvent,
    #[serde(rename = "notes")]
    _notes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginSource {
    call: String,
    server: String,
    login_answer: Option<LoginAnswer>,
    password: String,
    cycle_end: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginAnswer {
    status: u16,
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedApiError {
    route: String,
    status: u16,
    message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ErrorLink {
    depth: usize,
    #[serde(rename = "type")]
    link_type: String,
    message: String,
    sentinel: Option<String>,
}

fn kind_name(kind: ErrorKind) -> &'static str {
    let names = [
        (ErrorKind::InvalidCredentials, "invalid_credentials"),
        (ErrorKind::Unauthorized, "unauthorized"),
        (ErrorKind::ModuleNotFound, "module_not_found"),
        (ErrorKind::ModuleUnavailable, "module_unavailable"),
        (ErrorKind::Server, "server"),
        (ErrorKind::Redirect, "redirect"),
        (ErrorKind::UnexpectedResponse, "unexpected_response"),
        (ErrorKind::OrgNotFound, "org_not_found"),
        (ErrorKind::Refused, "refused"),
    ];
    names
        .iter()
        .find(|(known, _)| *known == kind)
        .map_or_else(|| panic!("no Go sentinel for {kind:?}"), |(_, name)| *name)
}

fn sei_error_in<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a SeiError> {
    if let Some(MonitorError::Sei(error)) = error.downcast_ref::<MonitorError>() {
        return Some(error);
    }
    error.downcast_ref::<SeiError>()
}

fn api_error_in<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a ApiError> {
    if let Some(SeiError::Api(api)) = sei_error_in(error) {
        return Some(api);
    }
    error.downcast_ref::<ApiError>()
}

fn link_type(error: &(dyn Error + 'static)) -> &'static str {
    if error.source().is_some() {
        return "wrapped";
    }
    if api_error_in(error).is_some() {
        return "api_error";
    }
    "plain"
}

fn rust_chain(error: &(dyn Error + 'static)) -> Vec<ErrorLink> {
    let mut links = Vec::new();
    let mut current = Some(error);
    while let Some(link) = current {
        links.push(ErrorLink {
            depth: links.len(),
            link_type: link_type(link).to_owned(),
            message: link.to_string(),
            sentinel: sei_error_in(link)
                .and_then(SeiError::kind)
                .or_else(|| api_error_in(link).map(|api| api.kind))
                .map(|kind| kind_name(kind).to_owned()),
        });
        current = link.source();
    }
    links
}

fn returned_part(chain: &[ErrorLink]) -> Vec<ErrorLink> {
    if chain[0].link_type != "joined" {
        return chain.to_vec();
    }
    let first_child = &chain[1];
    chain[1..]
        .iter()
        .enumerate()
        .take_while(|(index, link)| *index == 0 || link.depth > first_child.depth)
        .map(|(_, link)| ErrorLink {
            depth: link.depth - first_child.depth,
            ..link.clone()
        })
        .collect()
}

async fn client_login_error(source: &LoginSource) -> Box<dyn Error + 'static> {
    let fake = FakeSei::start();
    if let Some(answer) = &source.login_answer {
        let json_type = [("Content-Type", "application/json")];
        fake.respond_next(
            ROUTE_LOGIN,
            answer.status,
            &json_type,
            answer.body.clone().into_bytes(),
        );
    }
    let password = Some(source.password.as_str())
        .filter(|password| !password.is_empty())
        .unwrap_or(PASSWORD);
    let credentials = Credentials {
        username: USERNAME.to_owned(),
        password: Arc::new(StaticPassword(Secret::new(password.to_owned()))),
        org_id: ORG_ID,
    };
    let client = Client::new(&fake.api_url(), credentials, &options(&fake));
    let error = bounded(client.login())
        .await
        .expect_err("a login the server rejects");
    Box::new(error)
}

async fn run_once_login_error(source: &LoginSource) -> Option<Box<dyn Error + 'static>> {
    let (database, store) = database_store().await?;
    let fake = FakeSei::start();
    let monitor_config = config(&fake, &source.password);
    let ends = ["recorded", "closed_pool"];
    assert!(
        ends.contains(&source.cycle_end.as_str()),
        "{}",
        source.cycle_end
    );
    let monitor_store = if source.cycle_end == "closed_pool" {
        store_as(&database, "sei_cycle_end_denied", "UPDATE ON cycle_runs").await
    } else {
        store.clone()
    };
    let h = Harness::with(fake, monitor_store, monitor_config);
    h.prepare().await;
    let error = bounded(h.monitor.run_once(Trigger::Scheduled))
        .await
        .expect_err("a cycle with a wrong password");
    let run = store
        .last_cycle_run(h.account_id())
        .await
        .expect("last run")
        .expect("a run");
    let recorded = source.cycle_end == "recorded";
    assert_eq!(run.finished_at.is_some(), recorded, "{run:?}");
    Some(Box::new(error))
}

async fn login_error(source: &LoginSource) -> Option<Box<dyn Error + 'static>> {
    let servers = ["fake", "login_answer"];
    assert!(
        servers.contains(&source.server.as_str()),
        "{}",
        source.server
    );
    assert_eq!(
        source.login_answer.is_some(),
        source.server == "login_answer"
    );
    match source.call.as_str() {
        "client_login" => Some(client_login_error(source).await),
        "run_once" => run_once_login_error(source).await,
        other => panic!("unknown login failure source {other:?}"),
    }
}

#[tokio::test]
async fn login_failures_match_go() {
    let text: Value = serde_json::from_str(MONITOR_TEXT).expect("monitor_text.json");
    let cases = Vec::<LoginFailureCase>::deserialize(&text["login_failure"]).expect("cases");
    assert!(!cases.is_empty());
    for case in &cases {
        let Some(error) = login_error(&case.source).await else {
            return;
        };
        let error: &(dyn Error + 'static) = error.as_ref();
        let expected = returned_part(&case.chain);
        let api = api_error_in(error).unwrap_or_else(|| panic!("{}: {error}", case.name));
        let sei = sei_error_in(error).unwrap_or_else(|| panic!("{}: {error}", case.name));

        assert_eq!(api.route, case.api_error.route, "{}", case.name);
        assert_eq!(api.status, case.api_error.status, "{}", case.name);
        assert_eq!(api.message, case.api_error.message, "{}", case.name);
        assert_eq!(
            sei.kind(),
            Some(ErrorKind::InvalidCredentials),
            "{}",
            case.name
        );
        assert_eq!(error.to_string(), expected[0].message, "{}", case.name);
        if case.chain[0].link_type != "joined" {
            assert_eq!(error.to_string(), case.error, "{}", case.name);
        }
        assert_eq!(rust_chain(error), expected, "{}", case.name);
        let now = instant(&case.now).expect("the case time");
        let (reason, event) = login_failure(sei, now);
        assert_eq!(reason, case.reason, "{}", case.name);
        assert_event(&event, &case.event, &case.name);
    }
}

#[tokio::test]
async fn nul_characters_from_sei_are_dropped_before_they_are_stored() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    populate(&h.fake);
    h.fake.update_process(UNIT_NUPROC, 9002, |process| {
        process.description = "Di\u{0}rias".to_owned();
    });
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(FIFTEEN_MINUTES);
    h.fake.add_process(
        UNIT_NUPROC,
        FakeProcess {
            description: "Compra\u{0} de mesas".to_owned(),
            ..process(9003, 3, "Of\u{0}ício")
        },
    );
    h.fake.add_activities(
        9001,
        vec![activity(50004, start(), "Documento\u{0} assinado")],
    );

    h.run_once().await;

    let processes = store_processes(&h).await;
    let description = |id: i64| {
        processes
            .iter()
            .find(|row| row.procedure_id == id)
            .map(|row| (row.process_type.clone(), row.description.clone()))
    };
    assert_eq!(
        description(9002),
        Some(("Diárias".to_owned(), "Dirias".to_owned()))
    );
    assert_eq!(
        description(9003),
        Some(("Ofício".to_owned(), "Compra de mesas".to_owned()))
    );
    let events = h.events().await;
    let entered = events
        .iter()
        .find(|event| event.kind == Kind::ProcessEntered)
        .expect("the new process");
    assert_eq!(entered.payload.description, "Compra de mesas");
    let added = events
        .iter()
        .find(|event| event.kind == Kind::ActivityAdded)
        .expect("the new activity");
    assert_eq!(added.payload.activity_text, "Documento assinado");
}
