use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio_postgres::NoTls;

use super::Database;

use sei_monitor::event::{Event, Kind};
use sei_monitor::monitor::{CycleResult, Jitter, Monitor, MonitorConfig};
use sei_monitor::password::StaticPassword;
use sei_monitor::secret::Secret;
use sei_monitor::sei::{Options, Target};
use sei_monitor::store::{Store, Trigger};

use super::clock::FakeClock;
use super::fake_sei::{
    FakeActivity, FakeDocument, FakeProcess, FakeSei, FakeUser, ORG_ACRONYM, PASSWORD, UNIT_NUPROC,
    USER_ID, USER_NAME, USERNAME, process_number,
};
use super::{database_url_as, login_role, migrated_database};

pub const TEST_TIMEOUT: Duration = Duration::from_secs(20);
pub const FIFTEEN_MINUTES: Duration = Duration::from_secs(15 * 60);
const ROLE_PASSWORD: &str = "sei-role-password";
pub fn start() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-04T18:00:00Z")
        .expect("the start time")
        .to_utc()
}

pub fn hours(n: i64) -> chrono::Duration {
    chrono::Duration::hours(n)
}

pub async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(TEST_TIMEOUT, work)
        .await
        .expect("the monitor did not finish in time")
}

pub fn options(fake: &FakeSei) -> Options {
    Options {
        user_agent: "sei-monitor/test".to_owned(),
        zone: fake.zone(),
        timeout: Duration::from_secs(10),
    }
}

pub fn config(fake: &FakeSei, password: &str) -> MonitorConfig {
    MonitorConfig {
        target: Target {
            sei_url: fake.url(),
            api_url: String::new(),
            org: ORG_ACRONYM.to_owned(),
            username: USERNAME.to_owned(),
            password: Arc::new(StaticPassword(Secret::new(password.to_owned()))),
        },
        options: options(fake),
        units: Vec::new(),
        all_units: false,
        poll_interval: FIFTEEN_MINUTES,
        concurrency: 3,
        return_warning_days: 2,
        retention_days: 0,
        zone: fake.zone(),
    }
}

pub fn no_jitter() -> Jitter {
    Arc::new(|_| Duration::ZERO)
}

pub struct Harness {
    pub fake: FakeSei,
    pub store: Store,
    pub clock: Arc<FakeClock>,
    pub monitor: Arc<Monitor>,
}

impl Harness {
    pub fn new(store: &Store, configure: impl FnOnce(&mut MonitorConfig)) -> Harness {
        let fake = FakeSei::start();
        let mut monitor_config = config(&fake, PASSWORD);
        configure(&mut monitor_config);
        Harness::with(fake, store.clone(), monitor_config)
    }

    pub fn with(fake: FakeSei, store: Store, monitor_config: MonitorConfig) -> Harness {
        let clock = FakeClock::new(start());
        let monitor = Monitor::new(monitor_config, store.clone(), clock.clone(), no_jitter());
        Harness {
            fake,
            store,
            clock,
            monitor,
        }
    }

    pub async fn prepare(&self) {
        bounded(self.monitor.prepare()).await.expect("prepare");
    }

    pub async fn run_once(&self) -> CycleResult {
        bounded(self.monitor.run_once(Trigger::Scheduled))
            .await
            .expect("cycle")
    }

    pub fn account_id(&self) -> i64 {
        self.monitor.state().account_id
    }

    pub async fn events(&self) -> Vec<Event> {
        self.store
            .events_after(self.account_id(), 0, 500)
            .await
            .expect("events")
    }

    pub async fn activity_refs(&self) -> Vec<String> {
        self.events()
            .await
            .into_iter()
            .filter(|event| event.kind == Kind::ActivityAdded)
            .map(|event| event.reference)
            .collect()
    }
}

pub async fn database_store() -> Option<(Database, Store)> {
    let database = migrated_database().await?;
    let store = Store::new(&Secret::new(database.url.clone())).expect("store");
    Some((database, store))
}

pub async fn store_as(database: &Database, role: &str, revoke: &str) -> Store {
    let (client, connection) = tokio_postgres::connect(&database.url, NoTls)
        .await
        .expect("connect as the test superuser");
    let driver = tokio::spawn(connection);
    let login = login_role(role, ROLE_PASSWORD);
    let setup = format!(
        "{login}
         GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role};
         GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO {role};
         REVOKE {revoke} FROM {role};"
    );
    client
        .batch_execute(&setup)
        .await
        .unwrap_or_else(|error| panic!("a role without {revoke}: {error}"));
    drop(client);
    driver
        .await
        .expect("join the setup connection")
        .expect("close the setup connection");
    let url = database_url_as(&database.url, role, ROLE_PASSWORD);
    Store::new(&Secret::new(url)).expect("store")
}

pub fn user(id: i64, acronym: &str, name: &str) -> FakeUser {
    FakeUser {
        id,
        acronym: acronym.to_owned(),
        name: name.to_owned(),
    }
}

pub fn process(id: i64, n: u32, process_type: &str) -> FakeProcess {
    FakeProcess {
        id,
        number: process_number(n),
        process_type: process_type.to_owned(),
        ..FakeProcess::default()
    }
}

pub fn activity(id: i64, at: DateTime<Utc>, text: &str) -> FakeActivity {
    FakeActivity {
        id,
        user: USERNAME.to_owned(),
        unit: "SEDEMO/NUPROC".to_owned(),
        text: text.to_owned(),
        at,
    }
}

pub fn document(id: i64, label: &str) -> FakeDocument {
    FakeDocument {
        id,
        protocol: format!("00{label}"),
        document_type: "Ofício".to_owned(),
        number: String::new(),
        label: label.to_owned(),
    }
}

pub fn populate(fake: &FakeSei) {
    let first = FakeProcess {
        description: "Compra de cadeiras".to_owned(),
        assignee: Some(user(USER_ID, USERNAME, USER_NAME)),
        return_date: "05/11/2026".to_owned(),
        ..process(9001, 1, "Contrato")
    };
    fake.set_processes(UNIT_NUPROC, vec![first, process(9002, 2, "Diárias")]);
    fake.add_activities(
        9001,
        vec![
            activity(50001, start() - hours(72), "Processo gerado"),
            activity(
                50002,
                start() - hours(48),
                "Processo remetido pela unidade SEDEMO/PROTOCOLO",
            ),
            activity(50003, start() - hours(24), "Processo recebido na unidade"),
        ],
    );
    fake.add_activities(
        9002,
        vec![activity(60001, start() - hours(24), "Processo gerado")],
    );
    fake.add_documents(
        9001,
        vec![
            document(80001, "Ofício 1"),
            document(80002, "Despacho 2"),
            document(80003, "Anexo 3"),
        ],
    );
}
