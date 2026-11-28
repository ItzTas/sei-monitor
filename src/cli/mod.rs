mod commands;
mod diagnose;

use std::process::ExitCode;

use chrono_tz::Tz;
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory, Parser, Subcommand};
use monitor_hub_sdk::settings::Problem;

use crate::config::{Config, ConfigError, SeiConfig, database_url_path, load_document};
use crate::gotext::{go_quote, to_lower};
use crate::sei::{Connection, Options, REQUEST_TIMEOUT, SeiError, Target, connect};
use crate::store::check_database_url;
use crate::version::{VERSION, user_agent};

pub use commands::{orgs, test_login};
pub use diagnose::diagnose;

const PROGRAM: &str = "sei-monitor";
const USAGE_HINT: &str = "Run 'sei-monitor --help' for usage.";
const USAGE_ERROR: u8 = 2;
const INTERRUPTED: u8 = 130;
const SUGGESTION_DISTANCE: usize = 2;

#[derive(Parser)]
#[command(
    name = "sei-monitor",
    version = VERSION,
    about = "Watch your processes on SEI and get told when something changes"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Serve the monitor-hub monitor protocol; the hub starts this command")]
    Monitor,
    #[command(about = "Log in to SEI and show the versions and your units")]
    TestLogin,
    #[command(about = "List the acronym and id of every agency on the SEI login page")]
    Orgs,
    #[command(about = "Check how the SEI API behaves, printing only counts and types")]
    Diagnose,
    #[command(
        about = "Check the monitor settings from the hub's document or the environment and print the problems as JSON; the hub runs this command"
    )]
    ValidateSettings,
}

/// What start-up refuses before it connects, checked without connecting anywhere or running the
/// password command.
pub fn validate_settings() -> Vec<Problem> {
    let source = match load_document() {
        Ok(source) => source,
        Err(refusal) => return problems(refusal),
    };
    let config = match Config::load(&source) {
        Ok(config) => config,
        Err(refusal) => return problems(refusal),
    };
    let Err(refusal) = check_database_url(&config.database_url) else {
        return Vec::new();
    };
    vec![Problem::new(
        database_url_path(&source),
        refusal.to_string(),
    )]
}

fn problems(refusal: ConfigError) -> Vec<Problem> {
    refusal
        .problems
        .into_iter()
        .map(|problem| Problem::new(problem.path, problem.text))
        .collect()
}

pub fn usage_exit(error: clap::Error) -> ExitCode {
    if !error.use_stderr() {
        let _ = error.print();
        return ExitCode::SUCCESS;
    }
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    match go_usage_error(&error, &args) {
        Some(message) => eprintln!("{message}"),
        None => {
            let _ = error.print();
        }
    }
    eprintln!("{USAGE_HINT}");
    ExitCode::from(USAGE_ERROR)
}

pub fn command_exit(result: anyhow::Result<()>, interrupted: bool) -> ExitCode {
    if interrupted {
        eprintln!("interrupted");
        return ExitCode::from(INTERRUPTED);
    }
    let Err(failure) = result else {
        return ExitCode::SUCCESS;
    };
    eprintln!("error: {failure}");
    ExitCode::FAILURE
}

fn go_usage_error(error: &clap::Error, args: &[String]) -> Option<String> {
    let ContextValue::String(argument) = error.get(argument_context(error.kind())?)? else {
        return None;
    };
    if error.kind() == ErrorKind::InvalidSubcommand {
        return Some(unknown_command(argument, PROGRAM, &suggestions(argument)));
    }
    if let Some(flag) = argument.strip_prefix("--") {
        let name = flag.split_once('=').map_or(flag, |(name, _)| name);
        return Some(format!("unknown flag: --{name}"));
    }
    if let Some(shorthands) = argument.strip_prefix('-').filter(|rest| !rest.is_empty()) {
        let written = args
            .iter()
            .find(|arg| arg.starts_with(argument.as_str()))
            .and_then(|arg| arg.strip_prefix('-'))
            .unwrap_or(shorthands);
        let first = shorthands.chars().next()?;
        return Some(format!("unknown shorthand flag: '{first}' in -{written}"));
    }
    Some(unknown_command(argument, &command_path(args), &[]))
}

fn argument_context(kind: ErrorKind) -> Option<ContextKind> {
    let contexts = [
        (ErrorKind::InvalidSubcommand, ContextKind::InvalidSubcommand),
        (ErrorKind::UnknownArgument, ContextKind::InvalidArg),
    ];
    contexts
        .into_iter()
        .find(|(known, _)| *known == kind)
        .map(|(_, context)| context)
}

fn unknown_command(name: &str, path: &str, suggestions: &[String]) -> String {
    let message = format!("unknown command {} for {}", go_quote(name), go_quote(path));
    if suggestions.is_empty() {
        return message;
    }
    format!(
        "{message}\n\nDid you mean this?\n\t{}",
        suggestions.join("\n\t")
    )
}

fn command_path(args: &[String]) -> String {
    let cli = Cli::command();
    let subcommand = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .and_then(|arg| cli.find_subcommand(arg));
    match subcommand {
        Some(subcommand) => format!("{PROGRAM} {}", subcommand.get_name()),
        None => PROGRAM.to_owned(),
    }
}

fn suggestions(typed: &str) -> Vec<String> {
    let typed = to_lower(typed);
    Cli::command()
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_owned())
        .filter(|name| {
            let name = to_lower(name);
            distance(&typed, &name) <= SUGGESTION_DISTANCE || name.starts_with(&typed)
        })
        .collect()
}

fn distance(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut previous: Vec<usize> = (0..=a.len()).collect();
    for (j, &right) in b.iter().enumerate() {
        let mut current = vec![j + 1; a.len() + 1];
        for (i, &left) in a.iter().enumerate() {
            current[i + 1] = if left == right {
                previous[i]
            } else {
                1 + previous[i].min(previous[i + 1]).min(current[i])
            };
        }
        previous = current;
    }
    previous[a.len()]
}

fn options(zone: Tz) -> Options {
    Options {
        user_agent: user_agent(),
        zone,
        timeout: REQUEST_TIMEOUT,
    }
}

async fn open(config: &SeiConfig) -> Result<Connection, SeiError> {
    let target = Target {
        sei_url: config.url.clone(),
        api_url: config.api_url.clone(),
        org: config.org.clone(),
        username: config.username.clone(),
        password: config.password.clone(),
    };
    connect(&target, &options(config.zone)).await
}

fn units_error(error: SeiError) -> anyhow::Error {
    anyhow::anyhow!("SEI_UNITS: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_follow_cobra_in_declaration_order() {
        assert_eq!(suggestions("ORGZ"), ["orgs"]);
        assert_eq!(suggestions("diagnsoe"), ["diagnose"]);
        assert_eq!(
            suggestions(""),
            [
                "monitor",
                "test-login",
                "orgs",
                "diagnose",
                "validate-settings"
            ]
        );
        assert_eq!(suggestions("validate"), ["validate-settings"]);
        assert_eq!(suggestions("test-logn"), ["test-login"]);
        assert!(suggestions("x").is_empty());
        assert!(suggestions("ççrgs").is_empty());
    }

    #[test]
    fn the_settings_command_has_the_sdk_name() {
        assert!(
            Cli::command()
                .find_subcommand(monitor_hub_sdk::settings::VALIDATE_SETTINGS_ARG)
                .is_some()
        );
    }
}
