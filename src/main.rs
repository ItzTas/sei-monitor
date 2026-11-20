use std::error::Error;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use monitor_hub_sdk::{NOT_LAUNCHED_BY_HUB, launched_by_hub, logging};
use sei_monitor::app::run_monitor;
use sei_monitor::cli::{Cli, Command, command_exit, diagnose, orgs, test_login, usage_exit};
use sei_monitor::config::{Config, ConfigError, Source};
use sei_monitor::harden::harden;
use tokio::runtime::{Builder, Runtime};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, warn};

const RUNTIME_SHUTDOWN_LIMIT: Duration = Duration::from_millis(500);

type Operation = for<'a> fn(
    &'a Source,
    &'a mut dyn Write,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;

fn main() -> ExitCode {
    let hardening = harden();
    if let Err(failure) = &hardening {
        eprintln!("warning: could not harden the process: {failure}");
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(usage) => return usage_exit(usage),
    };
    match cli.command {
        Some(Command::Monitor) => monitor(hardening.err()),
        Some(Command::TestLogin) => operate(|source, out| Box::pin(test_login(source, out))),
        Some(Command::Orgs) => operate(|source, out| Box::pin(orgs(source, out))),
        Some(Command::Diagnose) => operate(|source, out| Box::pin(diagnose(source, out))),
        None => not_launched_by_hub(),
    }
}

fn operate(operation: Operation) -> ExitCode {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(failure) => return command_exit(Err(failure.into()), false),
    };
    let source = Source::environment();
    let mut stdout = io::stdout().lock();
    let (result, interrupted) =
        runtime.block_on(until_interrupted(operation(&source, &mut stdout)));
    let flushed = stdout.flush();
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_LIMIT);
    command_exit(result.and(flushed.map_err(Into::into)), interrupted)
}

async fn until_interrupted(
    work: impl Future<Output = anyhow::Result<()>>,
) -> (anyhow::Result<()>, bool) {
    let (mut interrupt, mut terminate) = match (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
    ) {
        (Ok(interrupt), Ok(terminate)) => (interrupt, terminate),
        (Err(failure), _) | (_, Err(failure)) => return (Err(failure.into()), false),
    };
    tokio::select! {
        biased;
        _ = interrupt.recv() => (Ok(()), true),
        _ = terminate.recv() => (Ok(()), true),
        result = work => (result, false),
    }
}

fn not_launched_by_hub() -> ExitCode {
    eprintln!("{NOT_LAUNCHED_BY_HUB}");
    ExitCode::FAILURE
}

fn monitor(hardening: Option<io::Error>) -> ExitCode {
    if !launched_by_hub() {
        return not_launched_by_hub();
    }
    let config = match Config::load(&Source::environment()) {
        Ok(config) => config,
        Err(problems) => return refuse(&problems),
    };
    if let Err(failure) = logging::init(config.log_filter()) {
        eprintln!("cannot start logging: {failure}");
        return ExitCode::FAILURE;
    }
    if let Some(failure) = &hardening {
        warn!(
            error = failure as &dyn Error,
            "could not harden the process"
        );
    }
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(failure) => {
            error!(
                error = &failure as &dyn Error,
                "cannot start the async runtime"
            );
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run_monitor(config));
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_LIMIT);
    let Err(failure) = result else {
        return ExitCode::SUCCESS;
    };
    error!(error = &*failure as &dyn Error, "fatal");
    ExitCode::FAILURE
}

fn refuse(problems: &ConfigError) -> ExitCode {
    if let Err(failure) = logging::init("info") {
        eprintln!("cannot start logging: {failure}");
        return ExitCode::FAILURE;
    }
    error!(error = problems as &dyn Error, "fatal");
    ExitCode::FAILURE
}

fn runtime() -> io::Result<Runtime> {
    Builder::new_multi_thread().enable_all().build()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use sei_monitor::password::{PasswordCommand, PasswordSource};

    use super::runtime;

    const REAP_LIMIT: Duration = Duration::from_secs(5);

    fn sleep_installed() -> bool {
        let installed = Command::new("sleep")
            .arg("0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if installed {
            return true;
        }
        if std::env::var_os("CI").is_some_and(|value| !value.is_empty()) {
            panic!("sleep is required when CI is set");
        }
        eprintln!("skipping: sleep is not available");
        false
    }

    fn children() -> Vec<(u32, char)> {
        let own = std::process::id();
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_to_string(entry.path().join("stat")).ok())
            .filter_map(|stat| {
                let (head, rest) = stat.rsplit_once(')')?;
                let pid = head.split_whitespace().next()?.parse().ok()?;
                let mut fields = rest.split_whitespace();
                let state = fields.next()?.chars().next()?;
                let parent: u32 = fields.next()?.parse().ok()?;
                (parent == own).then_some((pid, state))
            })
            .collect()
    }

    #[test]
    fn a_password_command_past_its_time_leaves_no_zombie() {
        if !sleep_installed() {
            return;
        }
        let runtime = runtime().expect("the monitor runtime");
        let command = PasswordCommand::new("sleep 30")
            .expect("a command line")
            .with_timeout(Duration::from_millis(200));

        let result = runtime.block_on(command.password());

        assert!(result.is_err());
        let deadline = Instant::now() + REAP_LIMIT;
        while !children().is_empty() && Instant::now() < deadline {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        }
        assert_eq!(children(), []);
    }
}
