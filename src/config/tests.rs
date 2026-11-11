use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::golden;

pub(crate) fn source<K: AsRef<str>, V: AsRef<str>>(env: &[(K, V)]) -> Source {
    let env: BTreeMap<String, String> = env
        .iter()
        .map(|(name, value)| (name.as_ref().to_owned(), value.as_ref().to_owned()))
        .collect();
    Source {
        lookup: Box::new(move |name| env.get(name).cloned()),
        read_file: Box::new(|_| Err(io::ErrorKind::NotFound.into())),
    }
}

pub(crate) fn minimal(changes: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env: BTreeMap<String, String> = [
        ("SEI_URL", "https://sei.example.org"),
        ("SEI_USERNAME", "user.name"),
        ("SEI_PASSWORD", "s3cr3t-password"),
        ("SEI_ORG", "SEDEMO"),
        (
            "DATABASE_URL",
            "postgres://monitor:s3cr3t-db@db.example.org/monitor",
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    for (name, value) in changes {
        env.insert((*name).to_owned(), (*value).to_owned());
    }
    env.into_iter().collect()
}

fn defaults_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SEI_URL", "https://sei.rr.gov.br/sei/"),
        ("SEI_USERNAME", "fulano.tal"),
        ("SEI_PASSWORD", "x"),
        ("SEI_ORG", "SEDEMO"),
        ("DATABASE_URL", "postgres://u:p@h/db"),
    ]
}

fn with(changes: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut env = defaults_env();
    env.retain(|(name, _)| !changes.iter().any(|(changed, _)| changed == name));
    env.extend_from_slice(changes);
    env
}

fn problems<T>(result: Result<T, ConfigError>) -> Vec<String> {
    match result {
        Ok(_) => panic!("the configuration loaded without problems"),
        Err(err) => err.problems,
    }
}

fn loaded(env: &[(&str, &str)]) -> PluginConfig {
    match PluginConfig::load(&source(env)) {
        Ok(config) => config,
        Err(err) => panic!("{err}"),
    }
}

#[test]
fn load_applies_the_defaults() {
    let config = loaded(&defaults_env());

    assert_eq!(config.sei.url, "https://sei.rr.gov.br");
    assert_eq!(config.sei.api_url, "");
    assert_eq!(config.sei.username, "fulano.tal");
    assert_eq!(config.sei.org, "SEDEMO");
    assert_eq!(config.sei.zone.name(), DEFAULT_TIME_ZONE);
    assert_eq!(config.sei.concurrency, 3);
    assert!(config.sei.units.is_empty());
    assert!(!config.sei.all_units);
    assert_eq!(config.database_url.reveal(), "postgres://u:p@h/db");
    assert_eq!(config.poll_interval, Duration::from_secs(15 * 60));
    assert_eq!(config.poll_interval, DEFAULT_POLL_INTERVAL);
    assert_eq!(config.return_warning_days, 2);
    assert_eq!(config.retention_days, 90);
    assert!(!config.full_detail);
    assert_eq!(config.log_level, 0);
    assert_eq!(config.log_filter(), "info");
}

#[tokio::test]
async fn password_setting_gives_its_secret() {
    let config = loaded(&defaults_env());

    assert_eq!(config.sei.password.password().await.unwrap().reveal(), "x");
}

#[test]
fn retention_outlasts_the_return_warning() {
    let too_short = with(&[("RETURN_WARNING_DAYS", "30"), ("RETENTION_DAYS", "31")]);
    assert_eq!(
        problems(PluginConfig::load(&source(&too_short))),
        [
            "RETENTION_DAYS: keep events for at least 32 days, RETURN_WARNING_DAYS plus 2, or use 0 to keep them forever (got 31)"
        ]
    );

    let config = loaded(&with(&[
        ("RETURN_WARNING_DAYS", "30"),
        ("RETENTION_DAYS", "32"),
    ]));
    assert_eq!(
        (config.return_warning_days, config.retention_days),
        (30, 32)
    );

    let config = loaded(&with(&[("RETENTION_DAYS", "0")]));
    assert_eq!(config.retention_days, 0);
}

#[test]
fn load_requires_a_postgres_database() {
    let mut missing = defaults_env();
    missing.retain(|(name, _)| *name != "DATABASE_URL");
    assert_eq!(
        problems(PluginConfig::load(&source(&missing))),
        ["DATABASE_URL: set the Postgres address, postgres://user:password@host/database"]
    );

    let mysql = with(&[("DATABASE_URL", "mysql://x")]);
    assert_eq!(
        problems(PluginConfig::load(&source(&mysql))),
        ["DATABASE_URL: use postgres://user:password@host/database"]
    );
}

#[test]
fn problems_never_show_secret_values() {
    let env = with(&[
        ("SEI_PASSWORD", "hunter2"),
        ("SEI_PASSWORD_CMD", "pass show hunter2"),
        ("DATABASE_URL", "mysql://u:hunter3@h/db"),
    ]);

    let Err(err) = PluginConfig::load(&source(&env)) else {
        panic!("the configuration loaded without problems");
    };

    assert_eq!(
        err.problems,
        [
            "set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD",
            "DATABASE_URL: use postgres://user:password@host/database",
        ]
    );
    for text in [err.to_string(), format!("{err:?}"), format!("{err:#?}")] {
        for secret in ["hunter2", "hunter3"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

#[test]
fn every_problem_is_listed_at_once() {
    let Err(err) = PluginConfig::load(&source(&[("POLL_INTERVAL", "1m")])) else {
        panic!("the configuration loaded without problems");
    };

    let text = err.to_string();
    assert!(text.starts_with("invalid configuration:\n  - "), "{text}");
    let positions: Vec<usize> = [
        "SEI_URL is required",
        "SEI_USERNAME is required",
        "SEI_PASSWORD is required",
        "SEI_ORG is required",
    ]
    .iter()
    .map(|problem| {
        text.find(problem)
            .unwrap_or_else(|| panic!("{problem} is missing from {text}"))
    })
    .collect();
    assert!(positions.is_sorted(), "{text}");
}

#[test]
fn log_filter_follows_the_level() {
    let cases = [
        ("debug", -4, "debug"),
        ("info-4", -4, "debug"),
        ("debug-1", -5, "debug"),
        ("debug+1", -3, "info"),
        ("INFO", 0, "info"),
        ("warn", 4, "warn"),
        ("ınfo", 0, "info"),
        ("warn+1", 5, "error"),
        ("erroR+03", 11, "error"),
    ];
    for (value, level, filter) in cases {
        let config = loaded(&with(&[("LOG_LEVEL", value)]));
        assert_eq!(
            (config.log_level, config.log_filter()),
            (level, filter),
            "{value}"
        );
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigCase {
    name: String,
    env: BTreeMap<String, String>,
    files: BTreeMap<String, String>,
    result: Value,
    problems: Option<Vec<String>>,
}

fn fake_program_dir() -> &'static str {
    static DIR: OnceLock<String> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("sei-monitor-config-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let sh = dir.join("sh");
        fs::write(&sh, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&sh, fs::Permissions::from_mode(0o755)).unwrap();
        dir.into_os_string().into_string().unwrap()
    })
}

fn case_source(case: &ConfigCase) -> Source {
    let mut env = case.env.clone();
    env.entry("PATH".to_owned())
        .or_insert_with(|| fake_program_dir().to_owned());
    let files = case.files.clone();
    Source {
        lookup: Box::new(move |name| env.get(name).cloned()),
        read_file: Box::new(move |path| {
            if path.starts_with("/denied/") {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            files
                .get(path)
                .map(|content| content.clone().into_bytes())
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }),
    }
}

fn password_record(password: &PasswordSetting) -> Value {
    match password {
        PasswordSetting::Static(secret) => json!({"source": "static", "value": secret.reveal()}),
        PasswordSetting::Command(command) => json!({"source": "command", "args": command.args()}),
    }
}

fn sei_record(sei: &SeiConfig) -> Value {
    json!({
        "url": sei.url,
        "api_url": sei.api_url,
        "username": sei.username,
        "org": sei.org,
        "units": sei.units,
        "all_units": sei.all_units,
        "zone": sei.zone.name(),
        "concurrency": sei.concurrency,
        "password": password_record(&sei.password),
    })
}

fn plugin_record(config: &PluginConfig) -> Value {
    json!({
        "sei": sei_record(&config.sei),
        "database_url": config.database_url.reveal(),
        "poll_interval_ns": u64::try_from(config.poll_interval.as_nanos()).unwrap(),
        "return_warning_days": config.return_warning_days,
        "retention_days": config.retention_days,
        "full_detail": config.full_detail,
        "log_level": config.log_level,
    })
}

fn outcome<T>(
    result: Result<T, ConfigError>,
    record: impl Fn(&T) -> Value,
) -> (Value, Option<Vec<String>>) {
    match result {
        Ok(value) => (record(&value), None),
        Err(err) => (Value::Null, Some(err.problems)),
    }
}

#[test]
fn config_matches_go() {
    for case in golden::cases::<ConfigCase>("config", "/plugin") {
        let got = outcome(PluginConfig::load(&case_source(&case)), plugin_record);
        assert_eq!(got, (case.result, case.problems), "plugin: {}", case.name);
    }
    for case in golden::cases::<ConfigCase>("config", "/sei") {
        let got = outcome(SeiConfig::load(&case_source(&case)), sei_record);
        assert_eq!(got, (case.result, case.problems), "sei: {}", case.name);
    }
    for case in golden::cases::<ConfigCase>("config", "/sei_url") {
        let got = outcome(load_sei_url(&case_source(&case)), |url| json!(url));
        assert_eq!(got, (case.result, case.problems), "sei_url: {}", case.name);
    }
}
