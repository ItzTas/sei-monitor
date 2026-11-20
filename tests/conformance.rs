mod support;

use std::env;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use monitor_hub_sdk_testkit::conformance::{Conformance, Outcome, run};

use sei_monitor::event::{Event, Kind, Payload};
use sei_monitor::secret::Secret;
use sei_monitor::store::{AccountKey, CycleChanges, Store};

use support::fake_sei::{
    FakeSei, ORG_ACRONYM, ORG_ID, PASSWORD, TIME_ZONE, USERNAME, process_number,
};
use support::process::ShortTempDir;
use support::{migrated_database, normalized_sei_url};

const READY_LIMIT: Duration = Duration::from_secs(30);

async fn seed_one_event(database_url: &str, sei_url: &str) {
    let store = Store::new(&Secret::new(database_url.to_owned())).expect("store");
    let now = Utc::now();
    let key = AccountKey {
        sei_base_url: normalized_sei_url(sei_url),
        username: USERNAME.to_owned(),
        org_id: ORG_ID,
    };
    let account = store.ensure_account(&key, now).await.expect("account");
    let entered = Event {
        id: 0,
        account_id: account.id,
        kind: Kind::ProcessEntered,
        procedure_id: 9001,
        reference: format!("9001:{}", now.to_rfc3339_opts(SecondsFormat::Secs, true)),
        occurred_at: Some(now),
        detected_at: Some(now),
        payload: Payload {
            process_number: process_number(1),
            unit: "SEDEMO/NUPROC".to_owned(),
            process_type: "Contrato".to_owned(),
            ..Payload::default()
        },
    };
    let stored = store
        .apply_cycle(&CycleChanges {
            account_id: account.id,
            now,
            finished_at: now,
            baseline: true,
            events: vec![entered],
            ..CycleChanges::default()
        })
        .await
        .expect("seed the baseline and one event");
    assert_eq!(stored.len(), 1, "{stored:?}");
}

fn plugin_conformance(sei_url: &str, database_url: &str, sockets: &ShortTempDir) -> Conformance {
    let mut conformance = Conformance::new(env!("CARGO_BIN_EXE_sei-monitor"));
    conformance.command.push("plugin".to_owned());
    conformance.env = [
        ("SEI_URL", sei_url),
        ("SEI_USERNAME", USERNAME),
        ("SEI_PASSWORD", PASSWORD),
        ("SEI_ORG", ORG_ACRONYM),
        ("SEI_TIMEZONE", TIME_ZONE),
        ("DATABASE_URL", database_url),
        ("PATH", &env::var("PATH").unwrap_or_default()),
        ("TMPDIR", sockets.text()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    conformance.min_events = 1;
    conformance.ready_timeout = READY_LIMIT;
    conformance
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_passes_the_hub_conformance_suite() {
    let Some(database) = migrated_database().await else {
        return;
    };
    let fake = FakeSei::start();
    let sei_url = format!("{}/sei/", fake.url());
    seed_one_event(&database.url, &sei_url).await;
    let sockets = ShortTempDir::new();
    let conformance = plugin_conformance(&sei_url, &database.url, &sockets);

    let outcome = tokio::task::spawn_blocking(move || run(&conformance))
        .await
        .expect("join the conformance run");

    match outcome {
        Outcome::Passed => {}
        Outcome::Skipped(reason) => eprintln!("skipping: {reason}"),
        Outcome::Failed(output) => panic!("{output}"),
        other => panic!("unexpected conformance outcome: {other:?}"),
    }
}
