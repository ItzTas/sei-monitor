mod answers;
mod gohttp;
mod handlers;
mod server;
mod setup;
mod state;

use std::sync::Arc;

use axum::http::{HeaderName, HeaderValue, StatusCode};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use self::server::Server;
use self::state::{Canned, Shared};

pub const USERNAME: &str = "fulano.tal";
pub const PASSWORD: &str = "senha-de-teste";
pub const USER_ID: i64 = 101;
pub const USER_NAME: &str = "Fulano de Tal";
pub const ORG_ID: i64 = 63;
pub const ORG_ACRONYM: &str = "SEDEMO";
pub const UNIT_NUPROC: i64 = 1001;
pub const UNIT_GAB: i64 = 1002;
pub const SEI_VERSION: &str = "5.0.5";
pub const MODULE_VERSION: &str = "3.0.4";
pub const TIME_ZONE: &str = "America/Boa_Vista";
pub const API_PATH: &str = "/sei/modulos/wssei/controlador_ws.php/api/v2";
pub const ROUTE_LOGIN: &str = "/autenticar";
pub const ROUTE_VERSION: &str = "/versao";
pub const ROUTE_ORGS: &str = "/orgao/listar";
pub const ROUTE_PROCESSES: &str = "/processo/listar";
pub const ROUTE_ACTIVITIES: &str = "/atividade/listar";
pub const ROUTE_DOCUMENTS: &str = "/documento/listar";
pub const ROUTE_LOGIN_PAGE: &str = "/sip/login.php";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FakeUser {
    pub id: i64,
    pub acronym: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FakeUnit {
    pub id: i64,
    pub acronym: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FakeProcess {
    pub id: i64,
    pub number: String,
    pub process_type: String,
    pub description: String,
    pub assignee: Option<FakeUser>,
    pub return_date: String,
    pub return_overdue: bool,
    pub generated: bool,
    pub generating_unit: Option<FakeUnit>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FakeActivity {
    pub id: i64,
    pub user: String,
    pub unit: String,
    pub text: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FakeDocument {
    pub id: i64,
    pub protocol: String,
    pub document_type: String,
    pub number: String,
    pub label: String,
}

pub fn process_number(n: u32) -> String {
    format!("00000.{n:06}/2026-00")
}

pub fn default_orgs() -> Vec<(i64, &'static str, &'static str)> {
    vec![
        (63, ORG_ACRONYM, "Secretaria de Demonstração"),
        (34, "ADEMO", "Agência de Demonstração"),
        (48, "BDEMO", "Banco de Demonstração"),
        (12, "CDEMO", "Corpo de Demonstração"),
        (15, "DDEMO", "Departamento de Demonstração"),
        (21, "EDEMO", "Escritório de Demonstração"),
        (25, "FDEMO", "Fundação de Demonstração"),
        (40, "GDEMO", "Gabinete de Demonstração"),
        (50, "HDEMO", "Hospital de Demonstração"),
        (70, "IDEMO", "Instituto de Demonstração"),
        (72, "JDEMO", "Junta de Demonstração"),
        (80, "KDEMO", "Kit de Demonstração"),
    ]
}

pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/testdata/sei/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("read the fixture {path}: {error}"))
}

pub struct FakeSei {
    shared: Arc<Shared>,
    server: Server,
}

impl FakeSei {
    pub fn start() -> FakeSei {
        let zone: Tz = TIME_ZONE.parse().expect("the fake SEI time zone");
        let shared = Arc::new(Shared::new(zone));
        let server = Server::start(handlers::router(Arc::clone(&shared)));
        FakeSei { shared, server }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.server.address())
    }

    pub fn api_url(&self) -> String {
        format!("{}{API_PATH}", self.url())
    }

    pub fn zone(&self) -> Tz {
        self.shared.lock().zone
    }

    pub fn set_processes(&self, unit_id: i64, processes: Vec<FakeProcess>) {
        self.shared.lock().processes.insert(unit_id, processes);
    }

    pub fn add_process(&self, unit_id: i64, process: FakeProcess) {
        self.shared
            .lock()
            .processes
            .entry(unit_id)
            .or_default()
            .push(process);
    }

    pub fn remove_process(&self, unit_id: i64, procedure_id: i64) {
        self.shared
            .lock()
            .processes
            .entry(unit_id)
            .or_default()
            .retain(|process| process.id != procedure_id);
    }

    pub fn update_process(
        &self,
        unit_id: i64,
        procedure_id: i64,
        change: impl FnOnce(&mut FakeProcess),
    ) {
        let mut state = self.shared.lock();
        let found = state
            .processes
            .get_mut(&unit_id)
            .and_then(|processes| processes.iter_mut().find(|p| p.id == procedure_id));
        if let Some(process) = found {
            change(process);
        }
    }

    pub fn add_activities(&self, procedure_id: i64, activities: Vec<FakeActivity>) {
        self.shared
            .lock()
            .activities
            .entry(procedure_id)
            .or_default()
            .extend(activities);
    }

    pub fn add_documents(&self, procedure_id: i64, documents: Vec<FakeDocument>) {
        self.shared
            .lock()
            .documents
            .entry(procedure_id)
            .or_default()
            .extend(documents);
    }

    pub fn remove_document(&self, procedure_id: i64, document_id: i64) {
        self.shared
            .lock()
            .documents
            .entry(procedure_id)
            .or_default()
            .retain(|document| document.id != document_id);
    }

    pub fn expire_tokens(&self) {
        self.shared.lock().sessions.clear();
    }

    pub fn set_password(&self, password: &str) {
        let mut state = self.shared.lock();
        state.account.password = password.to_owned();
        state.sessions.clear();
    }

    pub fn fail_next(&self, route: &str, status: u16, times: usize) {
        valid_status(status);
        self.shared
            .lock()
            .failures
            .entry(route.to_owned())
            .or_default()
            .extend(std::iter::repeat_n(status, times));
    }

    pub fn set_module_inactive(&self, inactive: bool) {
        self.shared.lock().inactive = inactive;
    }

    pub fn stall(&self, route: &str) {
        self.shared.lock().stalled.insert(route.to_owned());
    }

    pub fn respond_next(&self, route: &str, status: u16, headers: &[(&str, &str)], body: Vec<u8>) {
        let headers = headers
            .iter()
            .map(|(name, value)| {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .unwrap_or_else(|_| panic!("respond_next: invalid header name {name:?}"));
                let value = HeaderValue::from_str(value)
                    .unwrap_or_else(|_| panic!("respond_next: invalid header value {value:?}"));
                (name, value)
            })
            .collect();
        let canned = Canned {
            status: valid_status(status),
            headers,
            body: body.into(),
        };
        self.shared
            .lock()
            .canned
            .entry(route.to_owned())
            .or_default()
            .push_back(canned);
    }

    pub fn calls(&self, route: &str) -> usize {
        self.shared.lock().calls.get(route).copied().unwrap_or(0)
    }

    pub fn logins(&self) -> usize {
        self.shared.lock().logins
    }

    pub fn failed_logins(&self) -> usize {
        self.shared.lock().failed_logins
    }

    pub fn last_user_agent(&self) -> String {
        self.shared.lock().user_agent.clone()
    }

    pub fn issued_tokens(&self) -> Vec<String> {
        self.shared.lock().issued_tokens.clone()
    }
}

fn valid_status(status: u16) -> StatusCode {
    StatusCode::from_u16(status)
        .ok()
        .filter(|code| !code.is_informational())
        .unwrap_or_else(|| panic!("the fake SEI cannot answer with HTTP status {status}"))
}
