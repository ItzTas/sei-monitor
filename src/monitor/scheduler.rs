use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use tokio::sync::mpsc;
use tracing::{error, warn};

use super::{CycleResult, Monitor, MonitorError, login_failure};
use crate::config::MIN_POLL_INTERVAL;
use crate::duration::{format_go_duration, round_go_duration};
use crate::event::{Event, Kind, Payload};
use crate::sei::{ErrorKind, SeiError};
use crate::store::{CycleChanges, StoreError, Trigger};

const FIRST_RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_DOUBLINGS: u32 = 16;
const SECOND_NANOS: i64 = 1_000_000_000;
const MINIMUM_GAP: TimeDelta = TimeDelta::seconds(MIN_POLL_INTERVAL.as_secs() as i64);
const HELD_SUFFIX: &str =
    "; still paused after a restart: request a check once the configuration is fixed";

enum Readiness {
    Ready,
    Paused,
    Stopped,
}

enum PrepareFailure<'a> {
    Retry,
    Fatal,
    Rejected(&'a SeiError),
}

enum Standing {
    Active,
    Held,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerError {
    #[error(
        "monitoring is paused because SEI rejected the login; restart sei-monitor after fixing the configuration"
    )]
    Paused,
    #[error("sei-monitor has not reached SEI yet")]
    NotReady,
    #[error("a cycle is already running")]
    Running,
    #[error(
        "the last cycle started less than {} ago; try again in {}",
        go_text(MIN_POLL_INTERVAL),
        go_seconds(*.retry_after)
    )]
    TooSoon { retry_after: Duration },
}

impl Monitor {
    pub fn trigger_cycle(&self) -> Result<(), TriggerError> {
        let inner = self.inner();
        if inner.state.paused && !inner.state.held {
            return Err(TriggerError::Paused);
        }
        if inner.prepared.is_none() {
            return Err(TriggerError::NotReady);
        }
        if inner.state.running {
            return Err(TriggerError::Running);
        }
        if let Some(retry_after) = self.retry_after(inner.state.last_started_at) {
            return Err(TriggerError::TooSoon { retry_after });
        }
        self.manual.try_send(()).map_err(|_| TriggerError::Running)
    }

    pub async fn run(self: Arc<Self>) {
        let Ok(mut manual) = self.manual_signals.try_lock() else {
            return;
        };
        if !self.inner().state.paused && !self.start().await {
            return;
        }
        let mut trigger = Trigger::Scheduled;
        let mut failures = 0;
        loop {
            match self.standing() {
                Standing::Active => {}
                Standing::Held => {
                    if self
                        .unless_stopped(self.wait_for_check(&mut manual))
                        .await
                        .is_none()
                    {
                        return;
                    }
                    trigger = Trigger::Manual;
                }
                Standing::Paused => {
                    self.unless_stopped(self.record_the_pause()).await;
                    return self.stop_requested().await;
                }
            }
            let outcome = self.run_once(trigger).await;
            if matches!(outcome, Err(MonitorError::Canceled)) {
                return;
            }
            if let Some(rejection) = rejected_login(&outcome) {
                self.pause(rejection).await;
                continue;
            }
            if let Err(failure) = &outcome
                && let Some(reason) = self.rejection_on_record()
            {
                warn!(
                    error = failure as &dyn Error,
                    "the requested check failed; monitoring stays paused until another check is requested"
                );
                self.mark_held(&reason);
                continue;
            }
            let wait = next_wait(
                outcome.as_ref().err(),
                &mut failures,
                self.config.poll_interval,
                &*self.jitter,
            );
            self.schedule(wait);
            trigger = tokio::select! {
                () = self.clock.sleep(wait) => Trigger::Scheduled,
                Some(()) = manual.recv() => Trigger::Manual,
                () = self.stop_requested() => return,
            };
        }
    }

    async fn start(&self) -> bool {
        match self.prepare_until_ready().await {
            Readiness::Ready => self.hold_a_recorded_rejection().await,
            Readiness::Paused => {}
            Readiness::Stopped => return false,
        }
        true
    }

    async fn hold_a_recorded_rejection(&self) {
        if let Some(reason) = self.rejection_on_record() {
            self.hold(&reason).await;
        }
    }

    fn standing(&self) -> Standing {
        let inner = self.inner();
        if !inner.state.paused {
            return Standing::Active;
        }
        if inner.state.held {
            return Standing::Held;
        }
        Standing::Paused
    }

    async fn wait_for_check(&self, manual: &mut mpsc::Receiver<()>) {
        manual.recv().await;
        let mut inner = self.inner();
        inner.state.paused = false;
        inner.state.held = false;
        self.notifier.status_changed();
    }

    fn rejection_on_record(&self) -> Option<String> {
        let inner = self.inner();
        let reason = &inner.prepared.as_ref()?.account.paused_reason;
        (!reason.is_empty()).then(|| reason.clone())
    }

    async fn hold(&self, reason: &str) {
        let now = self.clock.now();
        let account_id = self.mark_held(reason);
        error!(
            reason = %reason,
            "SEI rejected the login in an earlier run; monitoring stays paused until a check is requested with the configuration fixed"
        );
        let event = held_event(reason, now);
        if let Err(failure) = self.record_event(account_id, now, event).await {
            error!(
                error = &failure as &dyn Error,
                "could not record the login failure event"
            );
        }
    }

    fn mark_held(&self, reason: &str) -> i64 {
        let mut inner = self.inner();
        inner.state.paused = true;
        inner.state.held = true;
        inner.state.paused_reason = reason.to_owned();
        inner.state.next_cycle_at = None;
        self.notifier.status_changed();
        inner.state.account_id
    }

    async fn record_event(
        &self,
        account_id: i64,
        now: DateTime<Utc>,
        event: Event,
    ) -> Result<(), StoreError> {
        let changes = CycleChanges {
            account_id,
            run_id: None,
            now,
            finished_at: now,
            baseline: false,
            processes: Vec::new(),
            left: Vec::new(),
            documents: Vec::new(),
            events: vec![event],
        };
        self.store.apply_cycle(&changes).await?;
        Ok(())
    }

    async fn prepare_until_ready(&self) -> Readiness {
        let mut failures = 0u32;
        loop {
            let Some(prepared) = self.unless_stopped(self.prepare()).await else {
                return Readiness::Stopped;
            };
            let Err(failure) = prepared else {
                {
                    let mut inner = self.inner();
                    inner.state.prepare_error = None;
                    inner.state.fatal = None;
                }
                self.notifier.status_changed();
                return Readiness::Ready;
            };
            failures = failures.saturating_add(1);
            let wait = backoff(failures, self.config.poll_interval, &*self.jitter);
            match prepare_failure(&failure) {
                PrepareFailure::Retry => self.retry_preparation(&failure, wait),
                PrepareFailure::Fatal => self.report_configuration(&failure),
                PrepareFailure::Rejected(rejection) => {
                    self.pause(rejection).await;
                    return Readiness::Paused;
                }
            }
            if self.unless_stopped(self.clock.sleep(wait)).await.is_none() {
                return Readiness::Stopped;
            }
        }
    }

    fn retry_preparation(&self, failure: &MonitorError, wait: Duration) {
        {
            let mut inner = self.inner();
            inner.state.fatal = None;
            inner.state.prepare_error = Some(failure.to_string());
        }
        self.notifier.status_changed();
        warn!(
            error = failure as &dyn Error,
            retry_in = %go_seconds(wait),
            "could not reach SEI; trying again"
        );
    }

    fn report_configuration(&self, failure: &MonitorError) {
        let text = failure.to_string();
        let previous = self.inner().state.fatal.replace(text.clone());
        if previous.as_deref() == Some(text.as_str()) {
            return;
        }
        self.notifier.status_changed();
        error!(
            error = failure as &dyn Error,
            "sei-monitor cannot monitor SEI with this configuration; trying again with a back-off"
        );
    }

    fn retry_after(&self, last_started_at: Option<DateTime<Utc>>) -> Option<Duration> {
        let since = self.clock.now() - last_started_at?;
        if since >= MINIMUM_GAP {
            return None;
        }
        (MINIMUM_GAP - since).to_std().ok()
    }

    fn schedule(&self, wait: Duration) {
        let next = TimeDelta::from_std(wait)
            .ok()
            .and_then(|wait| self.clock.now().checked_add_signed(wait));
        self.inner().state.next_cycle_at = next;
        self.notifier.status_changed();
    }

    async fn pause(&self, cause: &SeiError) {
        let now = self.clock.now();
        let (reason, event) = login_failure(cause, now);
        let account_id = self.mark_paused(&reason);
        error!(
            reason = %reason,
            "SEI rejected the login; monitoring is paused until sei-monitor restarts with the configuration fixed"
        );
        if account_id == 0 {
            return;
        }
        let (paused, recorded) = tokio::join!(
            self.store.set_paused_reason(account_id, &reason),
            self.record_event(account_id, now, event)
        );
        if let Err(failure) = paused {
            self.inner().pause_unrecorded = true;
            error!(error = &failure as &dyn Error, "could not record the pause");
        }
        if let Err(failure) = recorded {
            error!(
                error = &failure as &dyn Error,
                "could not record the login failure event"
            );
        }
    }

    async fn record_the_pause(&self) {
        let mut failures = 0u32;
        while let Some((account_id, reason)) = self.unrecorded_pause() {
            failures = failures.saturating_add(1);
            self.clock
                .sleep(backoff(failures, self.config.poll_interval, &*self.jitter))
                .await;
            let Err(failure) = self.store.set_paused_reason(account_id, &reason).await else {
                self.inner().pause_unrecorded = false;
                continue;
            };
            warn!(error = &failure as &dyn Error, "could not record the pause");
        }
    }

    fn unrecorded_pause(&self) -> Option<(i64, String)> {
        let inner = self.inner();
        inner
            .pause_unrecorded
            .then(|| (inner.state.account_id, inner.state.paused_reason.clone()))
    }

    fn mark_paused(&self, reason: &str) -> i64 {
        let mut inner = self.inner();
        inner.state.paused = true;
        inner.state.held = false;
        inner.state.paused_reason = reason.to_owned();
        inner.state.next_cycle_at = None;
        inner.reason_to_clear = false;
        if let Some(prepared) = inner.prepared.as_mut() {
            prepared.account.paused_reason = reason.to_owned();
        }
        self.notifier.status_changed();
        inner.state.account_id
    }
}

fn held_event(reason: &str, now: DateTime<Utc>) -> Event {
    Event {
        id: 0,
        account_id: 0,
        kind: Kind::AccountLoginFailed,
        procedure_id: 0,
        reference: format!("login-held:{}", now.date_naive()),
        occurred_at: None,
        detected_at: None,
        payload: Payload {
            reason: format!("{reason}{HELD_SUFFIX}"),
            ..Payload::default()
        },
    }
}

fn rejected_login(outcome: &Result<CycleResult, MonitorError>) -> Option<&SeiError> {
    let Err(MonitorError::Sei(error)) = outcome else {
        return None;
    };
    (error.kind() == Some(ErrorKind::InvalidCredentials)).then_some(error)
}

fn prepare_failure(error: &MonitorError) -> PrepareFailure<'_> {
    let MonitorError::Sei(error) = error else {
        return PrepareFailure::Retry;
    };
    match error.kind() {
        Some(ErrorKind::InvalidCredentials) => PrepareFailure::Rejected(error),
        Some(ErrorKind::OrgNotFound | ErrorKind::ModuleNotFound) => PrepareFailure::Fatal,
        _ => PrepareFailure::Retry,
    }
}

fn next_wait(
    failure: Option<&MonitorError>,
    failures: &mut u32,
    poll_interval: Duration,
    jitter: &(dyn Fn(Duration) -> Duration + Send + Sync),
) -> Duration {
    let Some(failure) = failure else {
        *failures = 0;
        return poll_interval + jitter(poll_interval / 10);
    };
    *failures = failures.saturating_add(1);
    let wait = backoff(*failures, poll_interval, jitter);
    warn!(
        error = failure as &dyn Error,
        retry_in = %go_seconds(wait),
        "cycle failed"
    );
    wait
}

fn backoff(
    failures: u32,
    poll_interval: Duration,
    jitter: &(dyn Fn(Duration) -> Duration + Send + Sync),
) -> Duration {
    let doublings = failures.saturating_sub(1).min(MAX_DOUBLINGS);
    let delay = (FIRST_RETRY_DELAY * 2u32.pow(doublings)).min(poll_interval.max(FIRST_RETRY_DELAY));
    delay + jitter(delay / 10)
}

fn nanos(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

fn go_text(duration: Duration) -> String {
    format_go_duration(nanos(duration))
}

fn go_seconds(duration: Duration) -> String {
    format_go_duration(round_go_duration(nanos(duration), SECOND_NANOS))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde::Deserialize;
    use tokio::time::timeout;

    use super::*;
    use crate::golden;
    use crate::monitor::offline_monitor;
    use crate::sei::ApiError;

    const REASON: &str = "SEI rejected the login";

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BackoffCase {
        poll_interval_ns: u64,
        failures: u32,
        jitter_limit_ns: u64,
        wait_ns: u64,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SuccessWaitCase {
        poll_interval_ns: u64,
        failures_before: u32,
        failures_after: u32,
        jitter_limit_ns: u64,
        jitter_ns: u64,
        wait_ns: u64,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TooSoonCase {
        retry_after_ns: u64,
        text: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ErrorTexts {
        running: String,
        paused: String,
        not_ready: String,
    }

    fn recording(draw: Duration) -> (Arc<Mutex<Vec<Duration>>>, impl Fn(Duration) -> Duration) {
        let limits = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&limits);
        let jitter = move |limit| {
            seen.lock().expect("the limits").push(limit);
            draw
        };
        (limits, jitter)
    }

    fn api_error(route: &str, kind: ErrorKind) -> MonitorError {
        MonitorError::Sei(SeiError::Api(ApiError {
            route: route.to_owned(),
            status: 401,
            message: "Usuário ou Senha Inválida.".to_owned(),
            kind,
        }))
    }

    #[test]
    fn a_failed_preparation_is_retried_paused_or_fatal() {
        let rejected = api_error("/autenticar", ErrorKind::InvalidCredentials);
        assert!(matches!(
            prepare_failure(&rejected),
            PrepareFailure::Rejected(SeiError::Api(ApiError {
                kind: ErrorKind::InvalidCredentials,
                ..
            }))
        ));
        let fatal = [
            MonitorError::Sei(SeiError::OrgNotFound("NOPE".to_owned())),
            MonitorError::Sei(SeiError::ModuleNotFound {
                first: "a".to_owned(),
                second: "b".to_owned(),
            }),
        ];
        for error in &fatal {
            assert!(
                matches!(prepare_failure(error), PrepareFailure::Fatal),
                "{error}"
            );
        }
        let retried = [
            api_error("/versao", ErrorKind::ModuleUnavailable),
            MonitorError::Sei(SeiError::Transport {
                context: "look for mod-wssei".to_owned(),
                detail: "connection refused".to_owned(),
                timed_out: false,
            }),
            MonitorError::Sei(SeiError::LoginPageStatus(502)),
        ];
        for error in &retried {
            assert!(
                matches!(prepare_failure(error), PrepareFailure::Retry),
                "{error}"
            );
        }
    }

    #[test]
    fn backoff_matches_go() {
        let cases = golden::cases::<BackoffCase>("monitor_text", "/backoff");
        for case in &cases {
            let (limits, jitter) = recording(Duration::ZERO);
            let wait = backoff(
                case.failures,
                Duration::from_nanos(case.poll_interval_ns),
                &jitter,
            );
            let label = (case.poll_interval_ns, case.failures);
            assert_eq!(wait, Duration::from_nanos(case.wait_ns), "{label:?}");
            assert_eq!(
                *limits.lock().expect("the limits"),
                [Duration::from_nanos(case.jitter_limit_ns)],
                "{label:?}"
            );
        }
    }

    #[test]
    fn success_wait_matches_go() {
        let cases = golden::cases::<SuccessWaitCase>("monitor_text", "/success_wait");
        for case in &cases {
            let (limits, jitter) = recording(Duration::from_nanos(case.jitter_ns));
            let mut failures = case.failures_before;
            let wait = next_wait(
                None,
                &mut failures,
                Duration::from_nanos(case.poll_interval_ns),
                &jitter,
            );
            let label = (case.poll_interval_ns, case.failures_before);
            assert_eq!(wait, Duration::from_nanos(case.wait_ns), "{label:?}");
            assert_eq!(failures, case.failures_after, "{label:?}");
            assert_eq!(
                *limits.lock().expect("the limits"),
                [Duration::from_nanos(case.jitter_limit_ns)],
                "{label:?}"
            );
        }
    }

    #[test]
    fn too_soon_matches_go() {
        for case in golden::cases::<TooSoonCase>("monitor_text", "/too_soon") {
            let error = TriggerError::TooSoon {
                retry_after: Duration::from_nanos(case.retry_after_ns),
            };
            assert_eq!(error.to_string(), case.text, "{}", case.retry_after_ns);
        }
    }

    #[test]
    fn error_texts_match_go() {
        let document: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/golden/monitor_text.json"
        )))
        .expect("monitor_text.json is JSON");
        let texts = ErrorTexts::deserialize(&document["errors"]).expect("the error texts");
        assert_eq!(TriggerError::Running.to_string(), texts.running);
        assert_eq!(TriggerError::Paused.to_string(), texts.paused);
        assert_eq!(MonitorError::Paused.to_string(), texts.paused);
        assert_eq!(TriggerError::NotReady.to_string(), texts.not_ready);
    }

    #[tokio::test]
    async fn marking_a_pause_and_a_hold_signals_the_status() {
        let monitor = offline_monitor();
        let mut signals = monitor.notifier().subscribe();
        signals.latest_seq();

        monitor.mark_paused(REASON);
        let paused = timeout(Duration::ZERO, signals.changed()).await.ok();
        let after_the_pause = monitor.state();
        monitor.mark_held(REASON);
        let held = timeout(Duration::ZERO, signals.changed()).await.ok();
        let after_the_hold = monitor.state();

        assert_eq!(paused, Some(0));
        assert!(
            after_the_pause.paused && !after_the_pause.held,
            "{after_the_pause:?}"
        );
        assert_eq!(held, Some(0));
        assert!(
            after_the_hold.paused && after_the_hold.held,
            "{after_the_hold:?}"
        );
    }

    #[tokio::test]
    async fn scheduling_the_next_cycle_signals_the_status() {
        let monitor = offline_monitor();
        let mut signals = monitor.notifier().subscribe();
        signals.latest_seq();

        monitor.schedule(Duration::from_secs(60));

        assert_eq!(
            timeout(Duration::ZERO, signals.changed()).await.ok(),
            Some(0)
        );
        assert!(monitor.state().next_cycle_at.is_some());
    }

    #[tokio::test]
    async fn preparation_failures_signal_the_status_once_per_new_text() {
        let monitor = offline_monitor();
        let mut signals = monitor.notifier().subscribe();
        signals.latest_seq();
        let unreachable = MonitorError::Sei(SeiError::Transport {
            context: "look for mod-wssei".to_owned(),
            detail: "connection refused".to_owned(),
            timed_out: false,
        });
        let unknown = MonitorError::Sei(SeiError::OrgNotFound("NOPE".to_owned()));
        let another = MonitorError::Sei(SeiError::OrgNotFound("OTHER".to_owned()));

        monitor.retry_preparation(&unreachable, Duration::from_secs(30));
        let retried = timeout(Duration::ZERO, signals.changed()).await.ok();
        let retrying = monitor.state();
        monitor.report_configuration(&unknown);
        let reported = timeout(Duration::ZERO, signals.changed()).await.ok();
        let failing = monitor.state();
        monitor.report_configuration(&unknown);
        let repeated = timeout(Duration::ZERO, signals.changed()).await.ok();
        monitor.report_configuration(&another);
        let changed = timeout(Duration::ZERO, signals.changed()).await.ok();

        assert_eq!(retried, Some(0));
        assert_eq!(retrying.prepare_error, Some(unreachable.to_string()));
        assert_eq!(reported, Some(0));
        assert_eq!(failing.fatal, Some(unknown.to_string()));
        assert_eq!(repeated, None);
        assert_eq!(changed, Some(0));
        assert_eq!(monitor.state().fatal, Some(another.to_string()));
    }

    #[tokio::test]
    async fn the_end_of_a_hold_signals_the_status() {
        let monitor = offline_monitor();
        monitor.mark_held(REASON);
        let mut signals = monitor.notifier().subscribe();
        signals.latest_seq();

        monitor
            .manual
            .try_send(())
            .expect("a free slot for the check");
        let mut manual = monitor
            .manual_signals
            .try_lock()
            .expect("the manual signals");
        monitor.wait_for_check(&mut manual).await;
        let ended = timeout(Duration::ZERO, signals.changed()).await.ok();

        assert_eq!(ended, Some(0));
        let state = monitor.state();
        assert!(!state.paused && !state.held, "{state:?}");
    }

    #[tokio::test]
    async fn a_fatal_report_signals_the_status() {
        let monitor = offline_monitor();
        let mut signals = monitor.notifier().subscribe();
        signals.latest_seq();

        monitor.report_fatal("x");

        assert_eq!(
            timeout(Duration::ZERO, signals.changed()).await.ok(),
            Some(0)
        );
        assert_eq!(monitor.state().fatal.as_deref(), Some("x"));
    }
}
