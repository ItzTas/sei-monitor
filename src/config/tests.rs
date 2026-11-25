use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use monitor_hub_sdk::config::{Document, from_lookup};
use monitor_hub_sdk::settings::{self, Field, FieldType, Settings};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::golden;

fn declaration() -> Settings {
    match settings::parse(SETTINGS.as_bytes()) {
        Ok(declaration) => declaration,
        Err(err) => panic!("{err}"),
    }
}

fn env_source(env: BTreeMap<String, String>, read_file: ReadFile) -> Source {
    let document = match from_lookup(&declaration(), |name| env.get(name).map(OsString::from)) {
        Ok(document) => document,
        Err(err) => panic!("{err}"),
    };
    match Source::with(document, env.get("PATH").cloned(), read_file) {
        Ok(source) => source,
        Err(err) => panic!("{err}"),
    }
}

fn files_source<K: AsRef<str>, V: AsRef<str>>(env: &[(K, V)], files: &[File]) -> Source {
    let env = env
        .iter()
        .map(|(name, value)| (name.as_ref().to_owned(), value.as_ref().to_owned()))
        .collect();
    let files: BTreeMap<String, Vec<u8>> = files
        .iter()
        .map(|(path, content)| ((*path).to_owned(), content.to_vec()))
        .collect();
    env_source(
        env,
        Box::new(move |path| {
            files
                .get(path)
                .cloned()
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }),
    )
}

pub(crate) fn source<K: AsRef<str>, V: AsRef<str>>(env: &[(K, V)]) -> Source {
    files_source(env, &[])
}

fn object(value: Value) -> Document {
    match value {
        Value::Object(document) => document,
        other => panic!("{other} is not an object"),
    }
}

fn not_found(_: &str) -> io::Result<Vec<u8>> {
    Err(io::ErrorKind::NotFound.into())
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

fn refused<T>(result: Result<T, ConfigError>) -> ConfigError {
    match result {
        Ok(_) => panic!("the configuration loaded without problems"),
        Err(err) => err,
    }
}

fn problems<T>(result: Result<T, ConfigError>) -> Vec<String> {
    refused(result).texts()
}

fn loaded(env: &[(&str, &str)]) -> Config {
    match Config::load(&source(env)) {
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
    assert_eq!(config.sei.zone.name(), "America/Sao_Paulo");
    assert_eq!(config.sei.concurrency, 3);
    assert!(config.sei.units.is_empty());
    assert!(!config.sei.all_units);
    assert_eq!(config.database_url.reveal(), "postgres://u:p@h/db");
    assert_eq!(config.poll_interval, Duration::from_secs(15 * 60));
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
        problems(Config::load(&source(&too_short))),
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
        problems(Config::load(&source(&missing))),
        ["DATABASE_URL: set the Postgres address, postgres://user:password@host/database"]
    );

    let mysql = with(&[("DATABASE_URL", "mysql://x")]);
    assert_eq!(
        problems(Config::load(&source(&mysql))),
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
    let document: Document = env
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), json!(value)))
        .collect();
    let direct = match Source::with(document, None, Box::new(not_found)) {
        Ok(source) => source,
        Err(err) => panic!("{err}"),
    };

    for source in [source(&env), direct] {
        let err = refused(Config::load(&source));

        for text in [err.to_string(), format!("{err:?}"), format!("{err:#?}")] {
            for secret in ["hunter2", "hunter3"] {
                assert!(!text.contains(secret), "a problem shows {secret}");
            }
        }
        assert_eq!(
            err.texts(),
            [
                "set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD",
                "DATABASE_URL: use postgres://user:password@host/database",
            ]
        );
    }
}

#[test]
fn a_secret_of_another_type_is_refused_without_its_value() {
    let cases = [
        ("sei_password", json!(8675309), "8675309"),
        ("sei_password_cmd", json!(true), "true"),
        ("database_url", json!(1.5), "1.5"),
    ];
    for (field, value, shown) in cases {
        let mut document = object(json!({"sei_url": "https://sei.example.org"}));
        document.insert(field.to_owned(), value);

        let Err(err) = Source::document(document) else {
            panic!("{field}: the document was taken");
        };

        for text in [err.to_string(), format!("{err:?}")] {
            assert!(
                !text.contains(shown),
                "{field}: the problem shows the value"
            );
        }
        assert_eq!(err.problems, [problem(field, "must be string")]);
    }

    let document = object(json!({"sei_url": "https://sei.example.org", "sei_password": null}));
    let Ok(source) = Source::document(document) else {
        panic!("a null secret was refused");
    };
    assert_eq!((source.lookup)("SEI_PASSWORD"), None);
}

#[test]
fn the_document_must_match_the_declaration() {
    for document in [json!({"sei_url": "x", "other": "y"}), json!({"sei_url": 5})] {
        let Err(err) = Source::document(object(document.clone())) else {
            panic!("{document} was taken");
        };

        assert_eq!(
            err.problems,
            [problem(
                "",
                "the configuration document does not match settings.json"
            )],
            "{document}"
        );
    }
}

#[test]
fn every_problem_is_listed_at_once() {
    let Err(err) = Config::load(&source(&[("POLL_INTERVAL", "1m")])) else {
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

#[test]
fn text_that_is_not_utf8_follows_the_sdk() {
    use std::os::unix::ffi::OsStringExt;

    let document = |name: &str| {
        let mut env: BTreeMap<String, OsString> = minimal(&[])
            .into_iter()
            .map(|(name, value)| (name, OsString::from(value)))
            .collect();
        env.insert(name.to_owned(), OsString::from_vec(b"a\xffb".to_vec()));
        from_lookup(&declaration(), |name| env.get(name).cloned())
    };
    for name in ["SEI_PASSWORD", "DATABASE_URL", "SEI_PASSWORD_CMD"] {
        let Err(err) = document(name) else {
            panic!("{name} was taken");
        };
        assert_eq!(
            ConfigError::from(err).problems,
            [problem(
                &name.to_ascii_lowercase(),
                &format!("{name} is not valid UTF-8")
            )]
        );
    }

    let source = match document("SEI_ORG").map_err(ConfigError::from) {
        Ok(document) => Source::with(document, None, Box::new(not_found)),
        Err(err) => Err(err),
    };
    let config = match source.and_then(|source| Config::load(&source)) {
        Ok(config) => config,
        Err(err) => panic!("{err}"),
    };
    assert_eq!(config.sei.org, "a\u{fffd}b");

    let no_address =
        "DATABASE_URL: set the Postgres address, postgres://user:password@host/database";
    let cases: [(Changes, File, Vec<ConfigProblem>); 2] = [
        (
            &[
                ("SEI_PASSWORD", ""),
                ("SEI_PASSWORD_FILE", "/run/secrets/sei"),
            ],
            ("/run/secrets/sei", b"pw\xff\n"),
            vec![problem(
                "sei_password_file",
                "SEI_PASSWORD_FILE: /run/secrets/sei is not valid UTF-8",
            )],
        ),
        (
            &[
                ("DATABASE_URL", ""),
                ("DATABASE_URL_FILE", "/run/secrets/database"),
            ],
            ("/run/secrets/database", b"postgres://u:\xc3@h/db\n"),
            vec![
                problem(
                    "database_url_file",
                    "DATABASE_URL_FILE: /run/secrets/database is not valid UTF-8",
                ),
                problem("database_url_file", no_address),
            ],
        ),
    ];
    for (changes, file, want) in cases {
        let source = files_source(&minimal(changes), &[file]);
        assert_eq!(refused(Config::load(&source)).problems, want, "{changes:?}");
    }
}

#[test]
fn blank_values_take_the_declared_default() {
    let load = |changes: &[(&str, &str)]| match Config::load(&source(&minimal(changes))) {
        Ok(config) => config_record(&config),
        Err(err) => panic!("{err}"),
    };
    let without = load(&[]);
    let names: Vec<String> = declared()
        .into_iter()
        .filter(|field| field.default.is_some())
        .filter_map(|field| field.env)
        .collect();
    assert!(!names.is_empty());
    for name in &names {
        assert_eq!(load(&[(name, " \t ")]), without, "{name}");
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

struct FakeProgramDir(PathBuf);

impl FakeProgramDir {
    fn create(name: &str) -> Option<FakeProgramDir> {
        let dir = FakeProgramDir(
            std::env::temp_dir().join(format!("sei-monitor-{name}-{}", std::process::id())),
        );
        fs::create_dir_all(&dir.0).unwrap();
        let sh = dir.0.join("sh");
        fs::write(&sh, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&sh, fs::Permissions::from_mode(0o755)).unwrap();
        if lookpath::is_executable(&sh) {
            return Some(dir);
        }
        let notice = format!(
            "the fake sh in {} cannot be executed (is the temporary directory mounted noexec?)",
            dir.0.display()
        );
        if std::env::var_os("CI").is_some_and(|ci| !ci.is_empty()) {
            panic!("{notice}, and CI is set");
        }
        eprintln!("skipping: {notice}");
        None
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for FakeProgramDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fake_program_dir_is_removed_after_use() {
    let Some(dir) = FakeProgramDir::create("cleanup-test") else {
        return;
    };
    let path = dir.0.clone();
    assert!(path.join("sh").is_file());

    drop(dir);

    assert!(!path.exists(), "{} is still there", path.display());
}

fn case_source(case: &ConfigCase, program_dir: &FakeProgramDir) -> Source {
    let mut env = case.env.clone();
    env.entry("PATH".to_owned())
        .or_insert_with(|| program_dir.path().to_owned());
    let files = case.files.clone();
    env_source(
        env,
        Box::new(move |path| {
            if path.starts_with("/denied/") {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            files
                .get(path)
                .map(|content| content.clone().into_bytes())
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }),
    )
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

fn config_record(config: &Config) -> Value {
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
        Err(err) => (Value::Null, Some(err.texts())),
    }
}

#[test]
fn config_matches_go() {
    let Some(program_dir) = FakeProgramDir::create("config-test") else {
        return;
    };
    // The frozen golden file names this group "plugin".
    for case in golden::cases::<ConfigCase>("config", "/plugin") {
        let got = outcome(
            Config::load(&case_source(&case, &program_dir)),
            config_record,
        );
        assert_eq!(got, (case.result, case.problems), "plugin: {}", case.name);
    }
    for case in golden::cases::<ConfigCase>("config", "/sei") {
        let got = outcome(
            SeiConfig::load(&case_source(&case, &program_dir)),
            sei_record,
        );
        assert_eq!(got, (case.result, case.problems), "sei: {}", case.name);
    }
    for case in golden::cases::<ConfigCase>("config", "/sei_url") {
        let got = outcome(load_sei_url(&case_source(&case, &program_dir)), |url| {
            json!(url)
        });
        assert_eq!(got, (case.result, case.problems), "sei_url: {}", case.name);
    }
}

fn declared() -> Vec<Field> {
    declaration().fields
}

fn names_where(fields: &[Field], keep: impl Fn(&Field) -> bool) -> BTreeSet<&str> {
    fields
        .iter()
        .filter(|field| keep(field))
        .map(|field| field.name.as_str())
        .collect()
}

#[test]
fn the_declaration_is_version_2() {
    let fields = declared();

    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "sei_url",
            "sei_api_url",
            "sei_username",
            "sei_org",
            "sei_password",
            "sei_password_file",
            "sei_password_cmd",
            "sei_timezone",
            "sei_units",
            "poll_interval",
            "sei_concurrency",
            "retention_days",
            "return_warning_days",
            "notification_detail",
            "database_url",
            "database_url_file",
            "log_level",
        ]
    );
    for field in &fields {
        assert_eq!(
            field.env,
            Some(field.name.to_ascii_uppercase()),
            "{}",
            field.name
        );
        assert!(
            field.fields.is_none() && field.items.is_none(),
            "{}",
            field.name
        );
    }
}

#[test]
fn declared_defaults_are_todays() {
    let defaults: BTreeMap<String, Value> = declared()
        .into_iter()
        .filter_map(|field| Some((field.env?, field.default?)))
        .collect();
    let today = [
        ("SEI_TIMEZONE", "America/Sao_Paulo"),
        ("POLL_INTERVAL", "15m"),
        ("SEI_CONCURRENCY", "3"),
        ("RETENTION_DAYS", "90"),
        ("RETURN_WARNING_DAYS", "2"),
        ("NOTIFICATION_DETAIL", "minimal"),
        ("LOG_LEVEL", "info"),
    ];
    assert_eq!(
        defaults,
        today
            .iter()
            .map(|(name, value)| ((*name).to_owned(), json!(value)))
            .collect::<BTreeMap<_, _>>()
    );

    let load = |changes: &[(&str, &str)]| match Config::load(&source(&minimal(changes))) {
        Ok(config) => config_record(&config),
        Err(err) => panic!("{err}"),
    };
    let without = load(&[]);
    for pair in today {
        assert_eq!(load(&[pair]), without, "{}={}", pair.0, pair.1);
    }
}

#[test]
fn the_declaration_keeps_files_and_programs_out_of_the_page() {
    let fields = declared();

    assert_eq!(
        names_where(&fields, |field| field.editable == Some(false)),
        BTreeSet::from(["database_url_file", "sei_password_cmd", "sei_password_file"])
    );
    for field in fields.iter().filter(|field| !field.is_editable()) {
        assert!(
            field
                .reason
                .as_deref()
                .is_some_and(|reason| !reason.is_empty()),
            "{}",
            field.name
        );
    }
    assert_eq!(
        names_where(&fields, |field| field.secret),
        BTreeSet::from(["database_url", "sei_password", "sei_password_cmd"])
    );
    assert_eq!(
        names_where(&fields, |field| field.required),
        BTreeSet::from(["sei_org", "sei_url", "sei_username"])
    );

    let kinds = BTreeMap::from([
        ("notification_detail", FieldType::Choice),
        ("poll_interval", FieldType::Duration),
    ]);
    for field in &fields {
        let kind = kinds
            .get(field.name.as_str())
            .copied()
            .unwrap_or(FieldType::String);
        assert_eq!(field.kind, kind, "{}", field.name);
    }
    let detail = fields
        .iter()
        .find(|field| field.name == "notification_detail")
        .unwrap();
    assert_eq!(
        detail.options.as_deref(),
        Some(["minimal".to_owned(), "full".to_owned()].as_slice())
    );
}

type Changes<'a> = &'a [(&'a str, &'a str)];
type File<'a> = (&'a str, &'a [u8]);

fn problem(path: &str, text: &str) -> ConfigProblem {
    ConfigProblem {
        path: path.to_owned(),
        text: text.to_owned(),
    }
}

#[test]
fn every_problem_names_its_path() {
    const ONLY_ONE: &str = "set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD";
    const NO_ADDRESS: &str =
        "DATABASE_URL: set the Postgres address, postgres://user:password@host/database";
    const NOT_POSTGRES: &str = "DATABASE_URL: use postgres://user:password@host/database";
    const DATABASE: &[u8] = b"postgres://u:p@h/db\n";

    let empty = golden::cases::<ConfigCase>("config", "/plugin")
        .into_iter()
        .find(|case| case.name == "empty environment")
        .and_then(|case| case.problems)
        .expect("the empty environment case");
    let want: Vec<ConfigProblem> = [
        "sei_url",
        "sei_username",
        "sei_password",
        "sei_org",
        "database_url",
    ]
    .iter()
    .zip(&empty)
    .map(|(path, text)| problem(path, text))
    .collect();
    let empty_source = files_source(&[("PATH", "/nonexistent")], &[]);
    assert_eq!(refused(Config::load(&empty_source)).problems, want);

    let cases: [(Changes, &[u8], Vec<ConfigProblem>); 21] = [
        (
            &[("SEI_URL", "ftp://x")],
            DATABASE,
            vec![problem(
                "sei_url",
                "SEI_URL: use a full address starting with https://",
            )],
        ),
        (
            &[("SEI_API_URL", "https://h/api?x")],
            DATABASE,
            vec![problem(
                "sei_api_url",
                "SEI_API_URL: remove everything after ? or #",
            )],
        ),
        (
            &[("SEI_PASSWORD_FILE", "/run/secrets/sei")],
            DATABASE,
            vec![problem("sei_password", ONLY_ONE)],
        ),
        (
            &[
                ("SEI_PASSWORD", ""),
                ("SEI_PASSWORD_FILE", "/run/secrets/sei"),
                ("SEI_PASSWORD_CMD", "sh"),
            ],
            DATABASE,
            vec![problem("", ONLY_ONE)],
        ),
        (
            &[
                ("SEI_PASSWORD", ""),
                ("SEI_PASSWORD_FILE", "/run/secrets/missing"),
            ],
            DATABASE,
            vec![problem(
                "sei_password_file",
                "SEI_PASSWORD_FILE: /run/secrets/missing does not exist",
            )],
        ),
        (
            &[("SEI_PASSWORD", ""), ("SEI_PASSWORD_CMD", "'x")],
            DATABASE,
            vec![problem(
                "sei_password_cmd",
                "SEI_PASSWORD_CMD: unterminated quote or trailing backslash",
            )],
        ),
        (
            &[
                ("SEI_PASSWORD", ""),
                ("SEI_PASSWORD_CMD", "no-such-program-sei-monitor"),
            ],
            DATABASE,
            vec![problem(
                "sei_password_cmd",
                "SEI_PASSWORD_CMD: program not found: no-such-program-sei-monitor",
            )],
        ),
        (
            &[("SEI_TIMEZONE", "Mars/Base")],
            DATABASE,
            vec![problem(
                "sei_timezone",
                "SEI_TIMEZONE: unknown time zone \"Mars/Base\" (e.g. America/Boa_Vista)",
            )],
        ),
        (
            &[("SEI_CONCURRENCY", "0")],
            DATABASE,
            vec![problem(
                "sei_concurrency",
                "SEI_CONCURRENCY: use a whole number from 1 to 10 (got \"0\")",
            )],
        ),
        (
            &[("RETURN_WARNING_DAYS", "61")],
            DATABASE,
            vec![problem(
                "return_warning_days",
                "RETURN_WARNING_DAYS: use a whole number from 0 to 60 (got \"61\")",
            )],
        ),
        (
            &[("RETENTION_DAYS", "3651")],
            DATABASE,
            vec![problem(
                "retention_days",
                "RETENTION_DAYS: use a whole number from 0 to 3650 (got \"3651\")",
            )],
        ),
        (
            &[("POLL_INTERVAL", "15")],
            DATABASE,
            vec![problem(
                "poll_interval",
                "POLL_INTERVAL: use a duration such as 15m or 1h (got \"15\")",
            )],
        ),
        (
            &[("POLL_INTERVAL", "1m")],
            DATABASE,
            vec![problem(
                "poll_interval",
                "POLL_INTERVAL: the minimum is 5m0s, to keep the load on the agency's SEI low (got 1m)",
            )],
        ),
        (
            &[("NOTIFICATION_DETAIL", "verbose")],
            DATABASE,
            vec![problem(
                "notification_detail",
                "NOTIFICATION_DETAIL: use minimal or full (got \"verbose\")",
            )],
        ),
        (
            &[("LOG_LEVEL", "loud")],
            DATABASE,
            vec![problem(
                "log_level",
                "LOG_LEVEL: use debug, info, warn or error (got \"loud\")",
            )],
        ),
        (
            &[("RETURN_WARNING_DAYS", "30"), ("RETENTION_DAYS", "31")],
            DATABASE,
            vec![problem(
                "retention_days",
                "RETENTION_DAYS: keep events for at least 32 days, RETURN_WARNING_DAYS plus 2, or use 0 to keep them forever (got 31)",
            )],
        ),
        (
            &[("RETURN_WARNING_DAYS", "x"), ("RETENTION_DAYS", "3")],
            DATABASE,
            vec![
                problem(
                    "return_warning_days",
                    "RETURN_WARNING_DAYS: use a whole number from 0 to 60 (got \"x\")",
                ),
                problem(
                    "retention_days",
                    "RETENTION_DAYS: keep events for at least 4 days, RETURN_WARNING_DAYS plus 2, or use 0 to keep them forever (got 3)",
                ),
            ],
        ),
        (
            &[("DATABASE_URL", "mysql://x")],
            DATABASE,
            vec![problem("database_url", NOT_POSTGRES)],
        ),
        (
            &[("DATABASE_URL_FILE", "/run/secrets/db")],
            DATABASE,
            vec![
                problem(
                    "database_url",
                    "set either DATABASE_URL or DATABASE_URL_FILE, not both",
                ),
                problem("database_url", NO_ADDRESS),
            ],
        ),
        (
            &[
                ("DATABASE_URL", ""),
                ("DATABASE_URL_FILE", "/run/secrets/missing"),
            ],
            DATABASE,
            vec![
                problem(
                    "database_url_file",
                    "DATABASE_URL_FILE: /run/secrets/missing does not exist",
                ),
                problem("database_url_file", NO_ADDRESS),
            ],
        ),
        (
            &[
                ("DATABASE_URL", ""),
                ("DATABASE_URL_FILE", "/run/secrets/db"),
            ],
            b"mysql://u:p@h/db\n",
            vec![problem("database_url_file", NOT_POSTGRES)],
        ),
    ];
    let path: Changes = &[("PATH", "/nonexistent")];
    for (changes, database, want) in cases {
        let env = minimal(&[changes, path].concat());
        let files: [File; 2] = [
            ("/run/secrets/sei", b"s3cr3t-file\n"),
            ("/run/secrets/db", database),
        ];
        let got = refused(Config::load(&files_source(&env, &files))).problems;
        assert_eq!(got, want, "{changes:?}");
    }
}

#[test]
fn every_problem_names_a_declared_path() {
    let Some(program_dir) = FakeProgramDir::create("paths-test") else {
        return;
    };
    let fields = declared();
    let names = names_where(&fields, |_| true);
    for case in golden::cases::<ConfigCase>("config", "/plugin") {
        let Err(err) = Config::load(&case_source(&case, &program_dir)) else {
            continue;
        };
        for problem in err.problems {
            assert!(
                problem.path.is_empty() || names.contains(problem.path.as_str()),
                "{}: {problem:?}",
                case.name
            );
        }
    }
}

fn recorded(source: Source, asked: &Arc<Mutex<BTreeSet<String>>>) -> Source {
    let asked = Arc::clone(asked);
    let Source { lookup, read_file } = source;
    Source {
        lookup: Box::new(move |name| {
            asked.lock().unwrap().insert(name.to_owned());
            lookup(name)
        }),
        read_file,
    }
}

#[test]
fn the_reader_asks_only_for_declared_variables() {
    let asked = Arc::new(Mutex::new(BTreeSet::new()));
    let envs: [Changes; 2] = [&[], &[("SEI_PASSWORD_CMD", "sh"), ("PATH", "/nonexistent")]];
    for env in envs {
        let _ = Config::load(&recorded(source(env), &asked));
    }

    let mut asked = asked.lock().unwrap().clone();
    assert!(asked.remove("PATH"), "{asked:?}");
    let variables: BTreeSet<String> = declared()
        .into_iter()
        .filter_map(|field| field.env)
        .collect();
    assert_eq!(asked, variables);
}

fn not_utf8() -> OsString {
    use std::os::unix::ffi::OsStringExt;

    OsString::from_vec(b"a\xffb".to_vec())
}

// Paths never carry a value, so a failure can print them.
fn paths(err: &ConfigError) -> Vec<&str> {
    err.problems
        .iter()
        .map(|problem| problem.path.as_str())
        .collect()
}

#[test]
fn operator_commands_read_only_their_variables() {
    let orgs_env = BTreeMap::from([
        ("SEI_URL", OsString::from("https://sei.example.org/sei/")),
        ("DATABASE_URL", not_utf8()),
        ("SEI_PASSWORD", not_utf8()),
        ("SEI_ORG", not_utf8()),
    ]);
    let url = operator_source_from(ORGS_VARIABLES, |name| orgs_env.get(name).cloned())
        .and_then(|source| load_sei_url(&source));
    match url {
        Ok(url) => assert_eq!(url, "https://sei.example.org"),
        Err(err) => panic!("orgs was refused: {:?}", paths(&err)),
    }

    let mut sei_env: BTreeMap<String, OsString> = minimal(&[])
        .into_iter()
        .map(|(name, value)| (name, OsString::from(value)))
        .collect();
    sei_env.insert("DATABASE_URL".to_owned(), not_utf8());
    let sei = operator_source_from(SEI_VARIABLES, |name| sei_env.get(name).cloned())
        .and_then(|source| SeiConfig::load(&source));
    match sei {
        Ok(sei) => assert_eq!(
            (sei.url.as_str(), sei.username.as_str(), sei.org.as_str()),
            ("https://sei.example.org", "user.name", "SEDEMO")
        ),
        Err(err) => panic!("test-login was refused: {:?}", paths(&err)),
    }
}

fn asked_by(load: impl Fn(&Source), envs: &[Changes]) -> BTreeSet<String> {
    let asked = Arc::new(Mutex::new(BTreeSet::new()));
    for env in envs {
        load(&recorded(source(env), &asked));
    }
    asked.lock().unwrap().clone()
}

fn names_of(variables: &[&str]) -> BTreeSet<String> {
    variables.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn operator_lists_are_what_the_commands_read() {
    let orgs = asked_by(
        |source| {
            let _ = load_sei_url(source);
        },
        &[&[]],
    );
    assert_eq!(orgs, names_of(ORGS_VARIABLES));

    let sei = asked_by(
        |source| {
            let _ = SeiConfig::load(source);
        },
        &[&[], &[("SEI_PASSWORD_CMD", "sh")]],
    );
    assert_eq!(sei, names_of(&[SEI_VARIABLES, &["PATH"]].concat()));
}
