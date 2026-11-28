use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::sync::{Mutex, MutexGuard, PoisonError};

use axum::body::Bytes;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use chrono_tz::Tz;

use super::{
    FakeActivity, FakeDocument, FakeProcess, FakeUnit, FakeUser, ORG_ID, PASSWORD, UNIT_GAB,
    UNIT_NUPROC, USER_ID, USER_NAME, USERNAME,
};

pub(super) struct Shared {
    state: Mutex<State>,
}

impl Shared {
    pub(super) fn new(zone: Tz) -> Shared {
        Shared {
            state: Mutex::new(State::new(zone)),
        }
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(super) struct Account {
    pub(super) username: String,
    pub(super) password: String,
    pub(super) org_id: i64,
    pub(super) user: FakeUser,
    pub(super) units: Vec<FakeUnit>,
}

pub(super) struct Canned {
    pub(super) status: StatusCode,
    pub(super) headers: Vec<(HeaderName, HeaderValue)>,
    pub(super) body: Bytes,
}

pub(super) enum Armed {
    Canned(Canned),
    Stall,
}

pub(super) struct State {
    pub(super) zone: Tz,
    pub(super) account: Account,
    pub(super) sessions: HashSet<String>,
    pub(super) issued_tokens: Vec<String>,
    pub(super) processes: HashMap<i64, Vec<FakeProcess>>,
    pub(super) activities: HashMap<i64, Vec<FakeActivity>>,
    pub(super) documents: HashMap<i64, Vec<FakeDocument>>,
    pub(super) calls: HashMap<String, usize>,
    pub(super) failures: HashMap<String, VecDeque<u16>>,
    pub(super) canned: HashMap<String, VecDeque<Canned>>,
    pub(super) stalled: HashSet<String>,
    pub(super) inactive: bool,
    pub(super) logins: usize,
    pub(super) failed_logins: usize,
    pub(super) user_agent: String,
}

impl State {
    fn new(zone: Tz) -> State {
        State {
            zone,
            account: default_account(),
            sessions: HashSet::new(),
            issued_tokens: Vec::new(),
            processes: HashMap::new(),
            activities: HashMap::new(),
            documents: HashMap::new(),
            calls: HashMap::new(),
            failures: HashMap::new(),
            canned: HashMap::new(),
            stalled: HashSet::new(),
            inactive: false,
            logins: 0,
            failed_logins: 0,
            user_agent: String::new(),
        }
    }

    pub(super) fn record_call(&mut self, route: &str, user_agent: String) {
        *self.calls.entry(route.to_owned()).or_default() += 1;
        self.user_agent = user_agent;
    }

    pub(super) fn take_armed(&mut self, route: &str) -> Option<Armed> {
        let canned = self.canned.get_mut(route).and_then(VecDeque::pop_front);
        if let Some(canned) = canned {
            return Some(Armed::Canned(canned));
        }
        self.stalled.contains(route).then_some(Armed::Stall)
    }

    pub(super) fn pop_failure(&mut self, route: &str) -> Option<u16> {
        self.failures.get_mut(route).and_then(VecDeque::pop_front)
    }

    pub(super) fn open_session(&mut self) -> String {
        let token = random_token();
        self.sessions.insert(token.clone());
        self.issued_tokens.push(token.clone());
        self.logins += 1;
        token
    }
}

fn default_account() -> Account {
    Account {
        username: USERNAME.to_owned(),
        password: PASSWORD.to_owned(),
        org_id: ORG_ID,
        user: FakeUser {
            id: USER_ID,
            acronym: USERNAME.to_owned(),
            name: USER_NAME.to_owned(),
        },
        units: vec![
            FakeUnit {
                id: UNIT_NUPROC,
                acronym: "SEDEMO/NUPROC".to_owned(),
                description: "Núcleo de Processos".to_owned(),
            },
            FakeUnit {
                id: UNIT_GAB,
                acronym: "SEDEMO/GAB".to_owned(),
                description: "Gabinete".to_owned(),
            },
        ],
    }
}

fn random_token() -> String {
    let keys = RandomState::new();
    (0u8..3)
        .flat_map(|block| keys.hash_one(block).to_le_bytes())
        .fold(String::with_capacity(48), |mut token, byte| {
            let _ = write!(token, "{byte:02x}");
            token
        })
}
