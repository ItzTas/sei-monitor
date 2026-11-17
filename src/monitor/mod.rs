mod details;
pub mod diff;
mod prune;
mod scheduler;

use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use self::details::{build_changes, fetch_details, observe_units};
use self::diff::{CurrentUser, Observation, Rules, diff_processes};
use crate::duration::{format_go_duration, round_go_duration};
use crate::event::{Event, Kind, Payload};
use crate::gotext::{capped, trim};
use crate::sei::{
    Client, Connection, Options, SeiError, Session, Target, Unit, connect, list_orgs,
};
use crate::store::{Account, AccountInfo, AccountKey, CycleChanges, Store, StoreError, Trigger};

pub use self::scheduler::TriggerError;

const MAX_ERROR_LENGTH: usize = 500;
const MILLISECOND_NANOS: i64 = 1_000_000;

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(duration))
    }
}

pub type Jitter = Arc<dyn Fn(Duration) -> Duration + Send + Sync>;

pub fn random_jitter() -> Jitter {
    Arc::new(|limit| {
        let nanos = u64::try_from(limit.as_nanos()).unwrap_or(u64::MAX);
        if nanos == 0 {
            return Duration::ZERO;
        }
        Duration::from_nanos(rand::random_range(0..nanos))
    })
}

#[derive(Clone)]
pub struct MonitorConfig {
    pub target: Target,
    pub options: Options,
    pub units: Vec<String>,
    pub all_units: bool,
    pub poll_interval: Duration,
    pub concurrency: usize,
    pub return_warning_days: i64,
    pub retention_days: i64,
    pub zone: Tz,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub account_id: i64,
    pub units: Vec<String>,
    pub user: CurrentUser,
    pub running: bool,
    pub paused: bool,
    pub held: bool,
    pub paused_reason: String,
    pub next_cycle_at: Option<DateTime<Utc>>,
    pub last_started_at: Option<DateTime<Utc>>,
    pub fatal: Option<String>,
    pub prepare_error: Option<String>,
    pub cycle_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CycleResult {
    pub processes_seen: usize,
    pub events_created: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    #[error(transparent)]
    Sei(#[from] SeiError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("a cycle is already running")]
    Running,
    #[error("sei-monitor has not reached SEI yet")]
    NotReady,
    #[error(
        "monitoring is paused because SEI rejected the login; restart sei-monitor after fixing the configuration"
    )]
    Paused,
    #[error("context canceled")]
    Canceled,
}

pub struct Monitor {
    config: MonitorConfig,
    store: Store,
    clock: Arc<dyn Clock>,
    jitter: Jitter,
    acked: AtomicI64,
    manual: mpsc::Sender<()>,
    manual_signals: tokio::sync::Mutex<mpsc::Receiver<()>>,
    stopping: watch::Sender<bool>,
    inner: Mutex<Inner>,
}

struct Inner {
    prepared: Option<Prepared>,
    logins: u64,
    reason_to_clear: bool,
    pause_unrecorded: bool,
    state: State,
}

#[derive(Clone)]
struct Prepared {
    connection: Connection,
    account: Account,
}

struct RunningGuard<'a>(&'a Monitor);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.inner().state.running = false;
    }
}

impl Monitor {
    pub fn new(
        config: MonitorConfig,
        store: Store,
        clock: Arc<dyn Clock>,
        jitter: Jitter,
    ) -> Arc<Monitor> {
        let state = State {
            user: CurrentUser {
                user_id: 0,
                acronym: config.target.username.clone(),
            },
            ..State::default()
        };
        let config = MonitorConfig {
            concurrency: config.concurrency.max(1),
            ..config
        };
        let (manual, manual_signals) = mpsc::channel(1);
        Arc::new(Monitor {
            config,
            store,
            clock,
            jitter,
            acked: AtomicI64::new(0),
            manual,
            manual_signals: tokio::sync::Mutex::new(manual_signals),
            stopping: watch::Sender::new(false),
            inner: Mutex::new(Inner {
                prepared: None,
                logins: 0,
                reason_to_clear: false,
                pause_unrecorded: false,
                state,
            }),
        })
    }

    pub fn state(&self) -> State {
        self.inner().state.clone()
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn config(&self) -> &MonitorConfig {
        &self.config
    }

    pub(crate) fn shut_down(&self) {
        self.stopping.send_replace(true);
    }

    pub(crate) fn report_fatal(&self, reason: &str) {
        self.inner().state.fatal = Some(reason.to_owned());
    }

    pub async fn prepare(&self) -> Result<(), MonitorError> {
        let target = &self.config.target;
        let connection = connect(target, &self.config.options).await?;
        let key = AccountKey {
            sei_base_url: target.sei_url.clone(),
            username: target.username.clone(),
            org_id: connection.org.id,
        };
        let (mut account, acronym) =
            tokio::try_join!(self.store.ensure_account(&key, self.clock.now()), async {
                Ok(self.org_acronym(&connection).await)
            },)?;
        if !acronym.is_empty() && acronym != account.info.org_acronym {
            account.info.org_acronym = acronym;
            self.store
                .update_account_info(account.id, &account.info)
                .await?;
        }
        account.paused_reason = capped(&account.paused_reason).to_owned();
        let mut inner = self.inner();
        inner.state.account_id = account.id;
        inner.state.paused_reason.clone_from(&account.paused_reason);
        inner.prepared = Some(Prepared {
            connection,
            account,
        });
        Ok(())
    }

    pub async fn run_once(&self, trigger: Trigger) -> Result<CycleResult, MonitorError> {
        let (prepared, started) = self.begin()?;
        let _running = RunningGuard(self);
        let outcome = self.recorded_cycle(&prepared, trigger, started).await;
        self.inner().state.cycle_error = outcome.as_ref().err().map(error_text);
        outcome
    }

    async fn recorded_cycle(
        &self,
        prepared: &Prepared,
        trigger: Trigger,
        started: DateTime<Utc>,
    ) -> Result<CycleResult, MonitorError> {
        let account_id = prepared.account.id;
        let run_id = self
            .store
            .start_cycle_run(account_id, trigger, started)
            .await?;
        let outcome = self.cycle(prepared, run_id, started).await;
        if let Err(error) = &outcome {
            self.record_failure(run_id, account_id, error).await;
        }
        outcome
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn org_acronym(&self, connection: &Connection) -> String {
        if !connection.org.acronym.is_empty() {
            return capped(&connection.org.acronym).to_owned();
        }
        let orgs = match list_orgs(&self.config.target.sei_url, &self.config.options).await {
            Ok(orgs) => orgs,
            Err(error) => {
                debug!(
                    error = &error as &dyn Error,
                    "could not read the agency acronym from the login page"
                );
                return String::new();
            }
        };
        orgs.into_iter()
            .find(|org| org.id == connection.org.id)
            .map(|org| capped(&org.acronym).to_owned())
            .unwrap_or_default()
    }

    fn stopping(&self) -> bool {
        *self.stopping.borrow()
    }

    async fn stop_requested(&self) {
        let mut stopping = self.stopping.subscribe();
        let _ = stopping.wait_for(|stopping| *stopping).await;
    }

    async fn unless_stopped<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            done = work => Some(done),
            () = self.stop_requested() => None,
        }
    }

    fn begin(&self) -> Result<(Prepared, DateTime<Utc>), MonitorError> {
        if self.stopping() {
            return Err(MonitorError::Canceled);
        }
        let mut inner = self.inner();
        if inner.state.paused {
            return Err(MonitorError::Paused);
        }
        if inner.state.running {
            return Err(MonitorError::Running);
        }
        let prepared = inner.prepared.clone().ok_or(MonitorError::NotReady)?;
        let started = self.clock.now();
        inner.state.running = true;
        inner.state.last_started_at = Some(started);
        Ok((prepared, started))
    }

    async fn record_failure(&self, run_id: i64, account_id: i64, error: &MonitorError) {
        let text = error_text(error);
        let finished = self
            .store
            .finish_cycle_run(run_id, account_id, self.clock.now(), &text)
            .await;
        if let Err(failure) = finished {
            warn!(
                error = &failure as &dyn Error,
                "could not record the end of a failed cycle"
            );
        }
    }

    async fn cycle(
        &self,
        prepared: &Prepared,
        run_id: i64,
        started: DateTime<Utc>,
    ) -> Result<CycleResult, MonitorError> {
        let account_id = prepared.account.id;
        let (mut changes, processes_seen) = self
            .unless_stopped(self.gather(prepared, run_id, started))
            .await
            .ok_or(MonitorError::Canceled)??;
        changes.finished_at = self.clock.now();
        let inserted = self.store.apply_cycle(&changes).await?;
        let baseline = changes.baseline;
        self.after_success(account_id, baseline, started).await;
        self.prune(account_id, started).await;
        let result = CycleResult {
            processes_seen,
            events_created: inserted.len(),
        };
        info!(
            processes = result.processes_seen,
            events = result.events_created,
            baseline,
            duration = %self.elapsed_since(started),
            "cycle finished"
        );
        Ok(result)
    }

    async fn gather(
        &self,
        prepared: &Prepared,
        run_id: i64,
        started: DateTime<Utc>,
    ) -> Result<(CycleChanges, usize), MonitorError> {
        let account = &prepared.account;
        let client = &prepared.connection.client;
        let ((session, observations), previous) =
            tokio::try_join!(self.observe(client, account), async {
                Ok(self.store.processes(account.id).await?)
            },)?;
        let baseline = account.baseline_at.is_none();
        let rules = Rules {
            now: started,
            zone: self.config.zone,
            return_warning_days: self.config.return_warning_days,
            user: CurrentUser {
                user_id: session.user_id,
                acronym: session.user_acronym,
            },
            baseline,
        };
        let diff = diff_processes(&previous, &observations, &rules);
        let details = fetch_details(
            &self.store,
            client,
            account.id,
            &diff.present,
            baseline,
            self.config.concurrency,
        )
        .await?;
        let processes_seen = diff.present.len();
        let changes = build_changes(account.id, run_id, started, baseline, diff, details);
        Ok((changes, processes_seen))
    }

    async fn observe(
        &self,
        client: &Client,
        account: &Account,
    ) -> Result<(Session, Vec<Observation>), MonitorError> {
        let session = client.session().await?;
        self.accept_login(account.id).await;
        let listing = async {
            let units = session.select_units(&self.config.units, self.config.all_units)?;
            self.remember_session(&session, &units);
            Ok(observe_units(client, &units, self.config.concurrency).await?)
        };
        let ((), observations) = tokio::try_join!(self.refresh_versions(client, account), listing)?;
        Ok((session, observations))
    }

    async fn refresh_versions(
        &self,
        client: &Client,
        account: &Account,
    ) -> Result<(), MonitorError> {
        let logins = client.logins().await;
        if logins == self.inner().logins {
            return Ok(());
        }
        let version = client.version().await?;
        let info = AccountInfo {
            org_acronym: account.info.org_acronym.clone(),
            sei_version: version.sei,
            wssei_version: version.module,
        };
        self.store.update_account_info(account.id, &info).await?;
        let mut inner = self.inner();
        inner.logins = logins;
        if let Some(prepared) = inner.prepared.as_mut() {
            prepared.account.info = info;
        }
        Ok(())
    }

    fn remember_session(&self, session: &Session, units: &[Unit]) {
        let mut inner = self.inner();
        inner.state.units = units.iter().map(|unit| unit.acronym.clone()).collect();
        inner.state.user = CurrentUser {
            user_id: session.user_id,
            acronym: session.user_acronym.clone(),
        };
    }

    async fn accept_login(&self, account_id: i64) {
        if self.forget_rejection() {
            self.clear_recorded_reason(account_id).await;
        }
    }

    fn forget_rejection(&self) -> bool {
        let mut inner = self.inner();
        let Some(prepared) = inner.prepared.as_mut() else {
            return false;
        };
        if std::mem::take(&mut prepared.account.paused_reason).is_empty() {
            return false;
        }
        inner.state.paused_reason.clear();
        true
    }

    async fn clear_recorded_reason(&self, account_id: i64) {
        let Err(error) = self.store.set_paused_reason(account_id, "").await else {
            return;
        };
        self.inner().reason_to_clear = true;
        warn!(
            error = &error as &dyn Error,
            "could not clear the pause reason"
        );
    }

    async fn after_success(&self, account_id: i64, baseline: bool, started: DateTime<Utc>) {
        if self.settle_success(baseline, started) {
            self.clear_recorded_reason(account_id).await;
        }
    }

    fn settle_success(&self, baseline: bool, started: DateTime<Utc>) -> bool {
        let mut inner = self.inner();
        if baseline && let Some(prepared) = inner.prepared.as_mut() {
            prepared.account.baseline_at = Some(started);
        }
        std::mem::take(&mut inner.reason_to_clear)
    }

    fn elapsed_since(&self, started: DateTime<Utc>) -> String {
        let nanos = (self.clock.now() - started)
            .num_nanoseconds()
            .unwrap_or(i64::MAX);
        format_go_duration(round_go_duration(nanos, MILLISECOND_NANOS))
    }
}

pub fn login_failure(error: &SeiError, now: DateTime<Utc>) -> (String, Event) {
    let mut reason = "SEI rejected the login".to_owned();
    if let SeiError::Api(api) = error
        && !api.message.is_empty()
    {
        reason = format!("{reason} ({})", api.message);
    }
    let event = Event {
        id: 0,
        account_id: 0,
        kind: Kind::AccountLoginFailed,
        procedure_id: 0,
        reference: format!("login:{}", now.to_rfc3339_opts(SecondsFormat::Secs, true)),
        occurred_at: None,
        detected_at: None,
        payload: Payload {
            reason: reason.clone(),
            ..Payload::default()
        },
    };
    (reason, event)
}

fn error_text(error: &MonitorError) -> String {
    describe(&error.to_string())
}

fn describe(text: &str) -> String {
    let text = trim(text);
    text[..text.floor_char_boundary(MAX_ERROR_LENGTH)].to_owned()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::golden;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct DescribeCase {
        input: String,
        output: String,
    }

    #[test]
    fn describe_matches_go() {
        for case in golden::cases::<DescribeCase>("monitor_text", "/describe") {
            assert_eq!(describe(&case.input), case.output, "{:?}", case.input);
        }
    }

    #[test]
    fn random_jitter_stays_below_its_limit() {
        let jitter = random_jitter();
        assert_eq!(jitter(Duration::ZERO), Duration::ZERO);
        assert_eq!(jitter(Duration::from_nanos(1)), Duration::ZERO);
        let limit = Duration::from_secs(90);
        let draws: Vec<Duration> = (0..1000).map(|_| jitter(limit)).collect();
        assert!(draws.iter().all(|draw| *draw < limit), "{draws:?}");
        assert!(draws.iter().any(|draw| *draw != draws[0]), "{draws:?}");
    }
}
