mod support;

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use monitor_hub_sdk::config::from_lookup;
use monitor_hub_sdk::settings;
use monitor_hub_sdk_testkit::settings::command_with_document;
use serde_json::{Value, json};

use sei_monitor::config::SETTINGS;
use support::fake_sei::{FakeSei, ORG_ACRONYM, TIME_ZONE, USERNAME};
use support::process::{COOKIE, Exited, ShortTempDir, run_command, run_isolated};

const LIMIT: Duration = Duration::from_secs(5);
const AT_ONCE: Duration = Duration::from_secs(1);
const VALID: &str = "{\"problems\":[]}\n";
const PASSWORD_MARKER: &str = "pw-marker-5e1c";
const DB_PASSWORD_MARKER: &str = "db-pass-marker-77a";
const DB_HOST_MARKER: &str = "db-host-marker-3c9";
const FILE_MARKER: &str = "file-marker-0b4";
const DB_FILE_MARKER: &str = "db-file-marker-6d2";
const URL_MARKER: &str = "url-marker-2a8";
const ARGUMENT_MARKER: &str = "cmd-arg-marker-91c";
const MISTYPED_PASSWORD: i64 = 8675309;
const MARKERS: [&str; 8] = [
    PASSWORD_MARKER,
    DB_PASSWORD_MARKER,
    DB_HOST_MARKER,
    FILE_MARKER,
    DB_FILE_MARKER,
    URL_MARKER,
    ARGUMENT_MARKER,
    "8675309",
];

static PROGRAM_DIRS: AtomicUsize = AtomicUsize::new(0);

type Env = Vec<(&'static str, String)>;

/// The fake SEI, and a listener where Postgres would be that nothing should connect to.
struct Base {
    fake: FakeSei,
    database: TcpListener,
}

impl Base {
    fn start() -> Base {
        let database = TcpListener::bind("127.0.0.1:0").expect("listen where Postgres would");
        database
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        Base {
            fake: FakeSei::start(),
            database,
        }
    }

    fn port(&self) -> u16 {
        self.database
            .local_addr()
            .expect("the listener's address")
            .port()
    }

    fn env(&self) -> Env {
        vec![
            ("SEI_URL", self.fake.url()),
            ("SEI_API_URL", self.fake.api_url()),
            ("SEI_USERNAME", USERNAME.to_owned()),
            ("SEI_PASSWORD", PASSWORD_MARKER.to_owned()),
            ("SEI_ORG", ORG_ACRONYM.to_owned()),
            ("SEI_TIMEZONE", TIME_ZONE.to_owned()),
            (
                "DATABASE_URL",
                format!(
                    "postgres://monitor:{DB_PASSWORD_MARKER}@127.0.0.1:{}/monitor",
                    self.port()
                ),
            ),
        ]
    }
}

/// A password program that leaves `ran` next to itself when it runs.
struct MarkerProgram(PathBuf);

impl MarkerProgram {
    fn create() -> MarkerProgram {
        let number = PROGRAM_DIRS.fetch_add(1, Ordering::SeqCst);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("validate-{}-{number}", std::process::id()));
        fs::create_dir_all(&dir).expect("create the program's directory");
        let program = MarkerProgram(dir);
        fs::write(
            program.path(),
            format!(
                "#!/bin/sh\necho ran > '{}'\necho password\n",
                program.ran().display()
            ),
        )
        .expect("write the program");
        fs::set_permissions(program.path(), fs::Permissions::from_mode(0o755))
            .expect("make the program executable");
        program
    }

    fn dir(&self) -> &str {
        self.0.to_str().expect("a UTF-8 directory")
    }

    fn path(&self) -> PathBuf {
        self.0.join("leave-marker")
    }

    fn ran(&self) -> PathBuf {
        self.0.join("ran")
    }
}

impl Drop for MarkerProgram {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn with(env: &Env, name: &'static str, value: &str) -> Env {
    let mut env = without(env, name);
    env.push((name, value.to_owned()));
    env
}

fn without(env: &Env, name: &str) -> Env {
    env.iter()
        .filter(|(known, _)| *known != name)
        .cloned()
        .collect()
}

fn borrowed(env: &Env) -> Vec<(&str, &str)> {
    env.iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

fn document_of(env: &Env) -> Value {
    let declaration = settings::parse(SETTINGS.as_bytes()).expect("settings.json");
    let document = from_lookup(&declaration, |name| {
        env.iter()
            .find(|(known, _)| *known == name)
            .map(|(_, value)| OsString::from(value))
    })
    .expect("a document from the variables");
    Value::Object(document)
}

fn path_only(env: &Env) -> Env {
    env.iter()
        .filter(|(name, _)| *name == "PATH")
        .cloned()
        .collect()
}

fn answer(problems: &[(&str, &str)]) -> String {
    let problems: Vec<String> = problems
        .iter()
        .map(|(path, message)| {
            format!(
                "{{\"path\":{},\"message\":{}}}",
                json!(path),
                json!(message)
            )
        })
        .collect();
    format!("{{\"problems\":[{}]}}\n", problems.join(","))
}

fn validate(base: &Base, env: &Env) -> Exited {
    let dir = ShortTempDir::new();
    let exited = run_isolated(&["validate-settings"], &borrowed(env), dir.path(), LIMIT);
    untouched(base, &exited, &dir);
    exited
}

fn validate_document(base: &Base, document: &Value, env: &Env) -> Exited {
    let dir = ShortTempDir::new();
    let bytes = serde_json::to_vec(document).expect("encode the document");
    let mut handed = command_with_document(
        env!("CARGO_BIN_EXE_sei-monitor"),
        &["validate-settings"],
        &bytes,
    )
    .expect("write the document");
    handed.command.envs(borrowed(env)).current_dir(dir.path());
    let exited = run_command(&mut handed.command, LIMIT);
    untouched(base, &exited, &dir);
    exited
}

// The markers come first, with messages that never print the output.
fn untouched(base: &Base, exited: &Exited, dir: &ShortTempDir) {
    for (index, marker) in MARKERS.iter().enumerate() {
        assert!(
            !exited.stdout.contains(marker),
            "stdout shows marker {index}"
        );
        assert!(
            !exited.stderr.contains(marker),
            "stderr shows marker {index}"
        );
    }
    assert_eq!(exited.stderr, "");
    assert_eq!(base.fake.requests(), 0, "requests to SEI");
    assert_eq!(base.fake.logins(), 0, "logins to SEI");
    assert_eq!(base.fake.failed_logins(), 0, "failed logins to SEI");
    let connection = base.database.accept().map(drop).map_err(|err| err.kind());
    assert_eq!(
        connection,
        Err(ErrorKind::WouldBlock),
        "a database connection"
    );
    assert!(is_empty(dir.path()), "the working directory is not empty");
}

fn is_empty(dir: &Path) -> bool {
    fs::read_dir(dir)
        .expect("read the directory")
        .next()
        .is_none()
}

fn secret_file(dir: &ShortTempDir, name: &str, content: &str) -> String {
    let path = dir.path().join(name);
    fs::write(&path, content).expect("write the file");
    path.to_str().expect("a UTF-8 path").to_owned()
}

#[test]
fn validate_settings_touches_neither_sei_nor_the_database() {
    let base = Base::start();
    let temporary = ShortTempDir::new();

    let exited = validate(&base, &with(&base.env(), "TMPDIR", temporary.text()));

    assert_eq!(exited.status.code(), Some(0));
    assert_eq!(exited.stdout, VALID);
    assert!(is_empty(temporary.path()), "TMPDIR is not empty");
}

#[test]
fn an_unreachable_address_answers_at_once() {
    let base = Base::start();
    let env = with(&base.env(), "SEI_URL", "https://192.0.2.1");
    let env = without(&env, "SEI_API_URL");
    let env = with(
        &env,
        "DATABASE_URL",
        "postgres://monitor:x@192.0.2.1:5432/monitor",
    );

    let started = Instant::now();
    let exited = validate(&base, &env);
    let took = started.elapsed();

    assert_eq!(exited.status.code(), Some(0));
    assert_eq!(exited.stdout, VALID);
    assert!(took < AT_ONCE, "the answer took {took:?}");
}

#[test]
fn an_empty_environment_lists_the_required_settings() {
    let base = Base::start();

    let exited = validate(&base, &Env::new());

    assert_eq!(exited.status.code(), Some(1));
    assert_eq!(
        exited.stdout,
        "{\"problems\":[{\"path\":\"sei_url\",\"message\":\"SEI_URL is required (the SEI address, e.g. https://sei.rr.gov.br)\"},{\"path\":\"sei_username\",\"message\":\"SEI_USERNAME is required\"},{\"path\":\"sei_password\",\"message\":\"SEI_PASSWORD is required (or SEI_PASSWORD_FILE, or SEI_PASSWORD_CMD)\"},{\"path\":\"sei_org\",\"message\":\"SEI_ORG is required (acronym or numeric id; the orgs subcommand lists them)\"},{\"path\":\"database_url\",\"message\":\"DATABASE_URL: set the Postgres address, postgres://user:password@host/database\"}]}\n"
    );
}

#[test]
fn the_hub_cookie_changes_nothing() {
    let base = Base::start();

    for env in [base.env(), Env::new()] {
        let alone = validate(&base, &env);
        let with_cookie = validate(&base, &with(&env, COOKIE.0, COOKIE.1));

        assert_eq!(with_cookie.status.code(), alone.status.code());
        assert_eq!(with_cookie.stdout, alone.stdout);
        assert_eq!(with_cookie.stderr, alone.stderr);
        assert!(!with_cookie.stdout.contains("|grpc"), "a handshake line");
    }
}

#[test]
fn every_problem_names_its_path() {
    let base = Base::start();

    let exited = validate(&base, &invalid_env(&base));

    assert_eq!(exited.status.code(), Some(1));
    assert_eq!(
        exited.stdout,
        "{\"problems\":[{\"path\":\"notification_detail\",\"message\":\"NOTIFICATION_DETAIL: use minimal or full (got \\\"verbose\\\")\"},{\"path\":\"sei_concurrency\",\"message\":\"SEI_CONCURRENCY: use a whole number from 1 to 10 (got \\\"0\\\")\"},{\"path\":\"poll_interval\",\"message\":\"POLL_INTERVAL: the minimum is 5m0s, to keep the load on the agency's SEI low (got 1m)\"}]}\n"
    );
}

fn invalid_env(base: &Base) -> Env {
    let env = with(&base.env(), "NOTIFICATION_DETAIL", "verbose");
    let env = with(&env, "SEI_CONCURRENCY", "0");
    with(&env, "POLL_INTERVAL", "1m")
}

#[test]
fn cross_field_problems_name_the_field_to_change() {
    let base = Base::start();
    let program = MarkerProgram::create();
    let files = ShortTempDir::new();
    let password_file = secret_file(&files, "password", &format!("{FILE_MARKER}\n"));
    let command = program.path().to_str().expect("a UTF-8 path").to_owned();
    let only_one = "set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD";
    let file_and_command = with(
        &without(&base.env(), "SEI_PASSWORD"),
        "SEI_PASSWORD_FILE",
        &password_file,
    );
    let retention = with(&base.env(), "RETURN_WARNING_DAYS", "30");
    let cases = [
        (
            with(&base.env(), "SEI_PASSWORD_CMD", &command),
            answer(&[("sei_password", only_one)]),
        ),
        (
            with(&file_and_command, "SEI_PASSWORD_CMD", &command),
            answer(&[("", only_one)]),
        ),
        (
            with(&retention, "RETENTION_DAYS", "31"),
            answer(&[(
                "retention_days",
                "RETENTION_DAYS: keep events for at least 32 days, RETURN_WARNING_DAYS plus 2, or use 0 to keep them forever (got 31)",
            )]),
        ),
    ];

    for (index, (env, expected)) in cases.iter().enumerate() {
        let exited = validate(&base, env);

        assert_eq!(exited.status.code(), Some(1), "case {index}");
        assert_eq!(&exited.stdout, expected, "case {index}");
    }
}

#[test]
fn validate_settings_never_runs_the_password_command() {
    let base = Base::start();
    let program = MarkerProgram::create();
    let absolute = format!("{} {ARGUMENT_MARKER}", program.path().display());
    let base_env = without(&base.env(), "SEI_PASSWORD");
    let on_path = with(&base_env, "PATH", program.dir());
    let cases = [
        (
            with(&base_env, "SEI_PASSWORD_CMD", &absolute),
            0,
            VALID.to_owned(),
        ),
        (
            with(
                &on_path,
                "SEI_PASSWORD_CMD",
                &format!("leave-marker {ARGUMENT_MARKER}"),
            ),
            0,
            VALID.to_owned(),
        ),
        (
            with(
                &with(&base_env, "SEI_PASSWORD_CMD", &absolute),
                "SEI_PASSWORD",
                PASSWORD_MARKER,
            ),
            1,
            answer(&[(
                "sei_password",
                "set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD",
            )]),
        ),
        (
            with(
                &on_path,
                "SEI_PASSWORD_CMD",
                &format!("no-such-program-sei-monitor {ARGUMENT_MARKER}"),
            ),
            1,
            answer(&[(
                "sei_password_cmd",
                "SEI_PASSWORD_CMD: program not found: no-such-program-sei-monitor",
            )]),
        ),
    ];

    for (index, (env, code, expected)) in cases.iter().enumerate() {
        let from_env = validate(&base, env);
        let from_document = validate_document(&base, &document_of(env), &path_only(env));

        for (source, exited) in [("environment", from_env), ("document", from_document)] {
            assert_eq!(exited.status.code(), Some(*code), "case {index}, {source}");
            assert_eq!(&exited.stdout, expected, "case {index}, {source}");
        }
    }
    assert!(!program.ran().exists(), "the password program ran");
}

#[test]
fn validate_settings_never_prints_a_secret() {
    let base = Base::start();
    let files = ShortTempDir::new();
    let password_file = secret_file(&files, "password", &format!("{FILE_MARKER}\n"));
    let database_file = secret_file(
        &files,
        "database",
        &format!("mysql://u:{DB_FILE_MARKER}@h/db"),
    );
    let mysql = format!("mysql://monitor:{DB_PASSWORD_MARKER}@{DB_HOST_MARKER}.invalid/monitor");
    let cases = [
        (
            with(
                &base.env(),
                "DATABASE_URL",
                &format!(
                    "postgres://monitor:{DB_PASSWORD_MARKER}@{DB_HOST_MARKER}.invalid/monitor"
                ),
            ),
            0,
            None,
        ),
        (
            with(
                &with(&base.env(), "DATABASE_URL", &mysql),
                "SEI_PASSWORD_FILE",
                &password_file,
            ),
            1,
            None,
        ),
        (
            with(
                &without(&base.env(), "SEI_PASSWORD"),
                "SEI_PASSWORD_FILE",
                &password_file,
            ),
            0,
            None,
        ),
        (
            with(
                &without(&base.env(), "DATABASE_URL"),
                "DATABASE_URL_FILE",
                &database_file,
            ),
            1,
            Some(answer(&[(
                "database_url_file",
                "DATABASE_URL: use postgres://user:password@host/database",
            )])),
        ),
        (
            with(
                &base.env(),
                "SEI_URL",
                &format!("https://user:{URL_MARKER}@sei.example.org"),
            ),
            1,
            None,
        ),
    ];

    for (index, (env, code, expected)) in cases.iter().enumerate() {
        let from_env = validate(&base, env);
        let from_document = validate_document(&base, &document_of(env), &Env::new());

        for (source, exited) in [("environment", from_env), ("document", from_document)] {
            assert_eq!(exited.status.code(), Some(*code), "case {index}, {source}");
            if let Some(expected) = expected {
                assert_eq!(&exited.stdout, expected, "case {index}, {source}");
            }
        }
    }
}

#[test]
fn a_secret_of_another_type_never_reaches_the_answer() {
    let base = Base::start();
    let mut document = document_of(&base.env());
    document["sei_password"] = json!(MISTYPED_PASSWORD);

    let exited = validate_document(&base, &document, &Env::new());

    assert_eq!(exited.status.code(), Some(1));
    assert_eq!(exited.stdout, answer(&[("sei_password", "must be string")]));
}

#[test]
fn a_document_on_descriptor_3_gives_the_same_answers() {
    let base = Base::start();

    for env in [base.env(), invalid_env(&base)] {
        let from_env = validate(&base, &env);
        let from_document = validate_document(&base, &document_of(&env), &Env::new());

        assert_eq!(from_document.status.code(), from_env.status.code());
        assert_eq!(from_document.stdout, from_env.stdout);
    }
}

#[test]
fn the_document_wins_over_the_environment() {
    let base = Base::start();

    let exited = validate_document(
        &base,
        &document_of(&base.env()),
        &vec![("SEI_CONCURRENCY", "0".to_owned())],
    );

    assert_eq!(exited.status.code(), Some(0));
    assert_eq!(exited.stdout, VALID);
}
