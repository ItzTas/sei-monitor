use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use monitor_hub_plugin::serve;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};
use tracing::info;

use crate::config::PluginConfig;
use crate::monitor::{Monitor, MonitorConfig, SystemClock, random_jitter};
use crate::secret::Secret;
use crate::sei::{Options, REQUEST_TIMEOUT, Target};
use crate::server::SeiPlugin;
use crate::store::{Store, WriterLock, acquire_writer_lock};
use crate::version::user_agent;

const START_UP_LIMIT: Duration = Duration::from_secs(45);
const CYCLE_CLOSE_LIMIT: Duration = Duration::from_millis(500);
const LOCK_LOST: &str = "the database lock was lost";
const SCHEDULER_STOPPED: &str = "the monitor stopped after an internal error; see the plugin log";

struct Started {
    lock: WriterLock,
    store: Store,
    first_read_boundary: i64,
}

pub async fn run_plugin(config: PluginConfig) -> anyhow::Result<()> {
    let started = start_up(&config.database_url, START_UP_LIMIT).await?;
    let monitor = Monitor::new(
        monitor_config(&config),
        started.store,
        Arc::new(SystemClock),
        random_jitter(),
    );
    let plugin = SeiPlugin::new(Arc::clone(&monitor), config.full_detail)
        .with_first_read_boundary(started.first_read_boundary);
    serve_until_stopped(started.lock, monitor, plugin).await
}

async fn start_up(database_url: &Secret, limit: Duration) -> anyhow::Result<Started> {
    let deadline = Instant::now() + limit;
    let mut lock = within(
        deadline,
        limit,
        "taking the database writer lock",
        acquire_writer_lock(database_url),
    )
    .await?;
    let applied = within(
        deadline,
        limit,
        "applying the database migrations",
        lock.migrate(),
    )
    .await?;
    info!(versions = ?applied, "database migrations applied");
    let store = Store::new(database_url).context("opening the database pool")?;
    let first_read_boundary = within(
        deadline,
        limit,
        "reading the last issued event",
        store.last_issued_seq(),
    )
    .await?;
    Ok(Started {
        lock,
        store,
        first_read_boundary,
    })
}

async fn within<T, E>(
    deadline: Instant,
    limit: Duration,
    step: &'static str,
    work: impl Future<Output = Result<T, E>>,
) -> anyhow::Result<T>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let Ok(finished) = timeout_at(deadline, work).await else {
        return Err(anyhow!("the start-up limit of {limit:?} ran out").context(step));
    };
    finished.context(step)
}

fn monitor_config(config: &PluginConfig) -> MonitorConfig {
    let sei = &config.sei;
    MonitorConfig {
        target: Target {
            sei_url: sei.url.clone(),
            api_url: sei.api_url.clone(),
            org: sei.org.clone(),
            username: sei.username.clone(),
            password: sei.password.clone(),
        },
        options: Options {
            user_agent: user_agent(),
            zone: sei.zone,
            timeout: REQUEST_TIMEOUT,
        },
        units: sei.units.clone(),
        all_units: sei.all_units,
        poll_interval: config.poll_interval,
        concurrency: sei.concurrency,
        return_warning_days: config.return_warning_days,
        retention_days: config.retention_days,
        zone: sei.zone,
    }
}

async fn serve_until_stopped(
    mut lock: WriterLock,
    monitor: Arc<Monitor>,
    plugin: SeiPlugin,
) -> anyhow::Result<()> {
    let run = tokio::spawn(Arc::clone(&monitor).run());
    let scheduler = run.abort_handle();
    let supervisor = tokio::spawn(supervise(Arc::clone(&monitor), run));
    let ended = tokio::select! {
        served = serve(plugin) => served.context("serving the plugin"),
        () = lock.closed() => Err(lock_lost(&lock)),
    };
    monitor.shut_down();
    if timeout(CYCLE_CLOSE_LIMIT, supervisor).await.is_err() {
        scheduler.abort();
    }
    ended
}

async fn supervise(monitor: Arc<Monitor>, run: JoinHandle<()>) {
    let Err(failure) = run.await else {
        return;
    };
    if failure.is_panic() {
        monitor.report_fatal(SCHEDULER_STOPPED);
    }
}

fn lock_lost(lock: &WriterLock) -> anyhow::Error {
    let Some(reason) = lock.close_reason() else {
        return anyhow!(LOCK_LOST);
    };
    anyhow::Error::msg(reason).context(LOCK_LOST)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tokio::net::TcpListener;

    use super::{SCHEDULER_STOPPED, start_up, supervise};
    use crate::monitor::{Monitor, MonitorConfig, SystemClock, random_jitter};
    use crate::password::StaticPassword;
    use crate::secret::Secret;
    use crate::sei::{Options, REQUEST_TIMEOUT, Target};
    use crate::store::Store;

    fn monitor() -> Arc<Monitor> {
        let zone = "America/Boa_Vista".parse().expect("a zone");
        let config = MonitorConfig {
            target: Target {
                sei_url: "http://127.0.0.1:9".to_owned(),
                api_url: String::new(),
                org: "SEDEMO".to_owned(),
                username: "fulano.tal".to_owned(),
                password: Arc::new(StaticPassword(Secret::new("unused".to_owned()))),
            },
            options: Options {
                user_agent: "sei-monitor/test".to_owned(),
                zone,
                timeout: REQUEST_TIMEOUT,
            },
            units: Vec::new(),
            all_units: false,
            poll_interval: Duration::from_secs(15 * 60),
            concurrency: 1,
            return_warning_days: 2,
            retention_days: 0,
            zone,
        };
        let store = Store::new(&Secret::new("postgres://sei@127.0.0.1:1/sei".to_owned()))
            .expect("a store that never connects");
        Monitor::new(config, store, Arc::new(SystemClock), random_jitter())
    }

    #[tokio::test]
    async fn a_scheduler_that_panics_leaves_its_failure_for_status() {
        let monitor = monitor();

        supervise(
            Arc::clone(&monitor),
            tokio::spawn(async { panic!("a bug in the scheduler") }),
        )
        .await;

        assert_eq!(monitor.state().fatal.as_deref(), Some(SCHEDULER_STOPPED));
    }

    #[tokio::test]
    async fn a_scheduler_that_returns_leaves_the_state_alone() {
        let monitor = monitor();

        supervise(Arc::clone(&monitor), tokio::spawn(async {})).await;

        assert_eq!(monitor.state().fatal, None);
    }

    #[tokio::test]
    async fn start_up_stops_at_its_limit_and_names_the_step() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let silent = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let url = Secret::new(format!("postgres://sei:hunter2@{address}/sei"));

        let started = Instant::now();
        let Err(error) = start_up(&url, Duration::from_millis(300)).await else {
            panic!("a silent database let the start-up finish");
        };
        let elapsed = started.elapsed();
        silent.abort();

        assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
        let text = format!("{error:#}");
        assert_eq!(
            text,
            "taking the database writer lock: the start-up limit of 300ms ran out"
        );
        assert!(!text.contains("hunter2"), "{text}");
    }
}
