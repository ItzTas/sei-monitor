mod support;

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use support::fake_sei::{
    FakeSei, MODULE_VERSION, PASSWORD, ROUTE_ACTIVITIES, ROUTE_DOCUMENTS, ROUTE_LOGIN,
    ROUTE_LOGIN_PAGE, ROUTE_PROCESSES, ROUTE_VERSION, SEI_VERSION,
};
use support::process::{Exited, run_to_exit, start_command};

const COMMANDS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/golden/commands.json"
));
const DIAGNOSE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/golden/diagnose.txt"
));
const DIAGNOSE_CASE: &str = "diagnose with the scenario of diagnose.txt";
const SEI_URL: &str = "<SEI_URL>";
const EXIT_LIMIT: Duration = Duration::from_secs(30);
const PATIENCE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(10);
const USAGE_HINT: &str = "Run 'sei-monitor --help' for usage.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    setup: Vec<Value>,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    interrupt: Option<Interrupt>,
    compare: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Interrupt {
    when: String,
    note: String,
}

struct Run {
    exited: Exited,
    logins: usize,
    failed_logins: usize,
    issued_tokens: Vec<String>,
}

fn cases() -> Vec<Case> {
    let file: Value = serde_json::from_str(COMMANDS).expect("commands.json is JSON");
    let cases = Vec::<Case>::deserialize(&file["cases"]).expect("the cases of commands.json");
    assert!(!cases.is_empty(), "commands.json has no cases");
    cases
}

fn case_named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("commands.json has no case named {name:?}"))
}

fn with_url(env: &BTreeMap<String, String>, url: &str) -> Vec<(String, String)> {
    env.iter()
        .map(|(name, value)| (name.clone(), value.replace(SEI_URL, url)))
        .collect()
}

fn pairs(env: &[(String, String)]) -> Vec<(&str, &str)> {
    env.iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn replay(case: &Case) -> Run {
    let fake = FakeSei::start();
    fake.apply(&case.setup)
        .unwrap_or_else(|error| panic!("{}: setup: {error}", case.name));
    let env = with_url(&case.env, &fake.url());
    let args: Vec<&str> = case.args.iter().map(String::as_str).collect();
    let mut exited = match &case.interrupt {
        None => run_to_exit(&args, &pairs(&env), EXIT_LIMIT),
        Some(interrupt) => interrupted(&fake, interrupt, &args, &pairs(&env), libc::SIGINT),
    };
    exited.stdout = exited.stdout.replace(&fake.url(), SEI_URL);
    exited.stderr = exited.stderr.replace(&fake.url(), SEI_URL);
    Run {
        exited,
        logins: fake.logins(),
        failed_logins: fake.failed_logins(),
        issued_tokens: fake.issued_tokens(),
    }
}

fn interrupted(
    fake: &FakeSei,
    interrupt: &Interrupt,
    args: &[&str],
    env: &[(&str, &str)],
    signal: libc::c_int,
) -> Exited {
    assert_eq!(
        interrupt.when, "before_first_request",
        "unknown interruption point; its note: {}",
        interrupt.note
    );
    fake.stall(ROUTE_VERSION);
    fake.stall(ROUTE_LOGIN_PAGE);
    let mut started = start_command(args, env);
    let deadline = Instant::now() + PATIENCE;
    while fake.calls(ROUTE_VERSION) + fake.calls(ROUTE_LOGIN_PAGE) == 0 {
        assert!(
            Instant::now() < deadline,
            "the command sent no request to the fake SEI within {PATIENCE:?}"
        );
        thread::sleep(POLL);
    }
    started.signal(signal);
    started.finish(EXIT_LIMIT)
}

fn differences(case: &Case, run: &Run) -> Vec<String> {
    assert!(
        ["exact", "usage"].contains(&case.compare.as_str()),
        "{}: unknown compare value {:?}",
        case.name,
        case.compare
    );
    let exited = &run.exited;
    let checks = [
        (
            "exit code",
            format!("{:?}", Some(case.exit_code)),
            format!("{:?}", exited.status.code()),
        ),
        ("stdout", case.stdout.clone(), exited.stdout.clone()),
        ("stderr", case.stderr.clone(), exited.stderr.clone()),
    ];
    let mut found: Vec<String> = checks
        .into_iter()
        .filter(|(_, expected, got)| expected != got)
        .map(|(what, expected, got)| {
            format!(
                "{}: {what} differs\n--- Go\n{expected}\n--- Rust\n{got}",
                case.name
            )
        })
        .collect();
    let attempts = run.logins + run.failed_logins;
    if attempts > 1 {
        found.push(format!(
            "{}: {attempts} login attempts, Go makes at most 1",
            case.name
        ));
    }
    found
}

#[test]
fn commands_match_go() {
    let cases = cases();
    let failures: Vec<String> = cases
        .iter()
        .flat_map(|case| differences(case, &replay(case)))
        .collect();

    assert!(
        failures.is_empty(),
        "{} of {} cases differ from Go:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

fn assert_hidden(text: &str, values: &[&str]) {
    for value in values {
        assert!(!text.contains(value), "the output shows {value:?}:\n{text}");
    }
}

#[test]
fn diagnose_matches_go() {
    let case = case_named(DIAGNOSE_CASE);
    assert_eq!(case.stdout, DIAGNOSE, "the vector case and diagnose.txt");

    let run = replay(&case);

    let exited = &run.exited;
    assert_eq!(exited.status.code(), Some(0), "{}", exited.stderr);
    assert_eq!(exited.stderr, "");
    assert_eq!(exited.stdout, DIAGNOSE);
    assert_eq!((run.logins, run.failed_logins), (1, 0));
    assert_hidden(
        &exited.stdout,
        &[
            "00000.",
            "9001",
            "50001",
            "80001",
            "SEDEMO",
            "fulano",
            "Fulano",
            "beltrano",
            "Beltrano",
            "Diárias",
            "Brasília",
            "Contrato",
            "Ofício",
            "Despacho",
            "recebido",
            "0012345",
            "06/11/2026",
            PASSWORD,
        ],
    );
    let tokens: Vec<&str> = run.issued_tokens.iter().map(String::as_str).collect();
    assert_hidden(&exited.stdout, &tokens);
}

const WRONG_PASSWORD: &str = "not-the-password-7c1e";

fn assert_no_secret_printed(case: &str, setting: (&str, &str), logins: (usize, usize)) {
    let mut case = case_named(case);
    case.env.insert(setting.0.to_owned(), setting.1.to_owned());

    let run = replay(&case);

    assert_eq!((run.logins, run.failed_logins), logins, "{setting:?}");
    let text = format!("{}{}", run.exited.stdout, run.exited.stderr);
    let mut secrets = vec![PASSWORD, case.env["SEI_PASSWORD"].as_str()];
    secrets.extend(run.issued_tokens.iter().map(String::as_str));
    assert_hidden(&text, &secrets);
}

#[test]
fn test_login_never_prints_the_password() {
    let case = "test-login with the current unit";
    assert_no_secret_printed(case, ("SEI_UNITS", "sedemo/gab"), (1, 0));
    assert_no_secret_printed(case, ("SEI_UNITS", "SEDEMO/XYZ"), (1, 0));
    assert_no_secret_printed(case, ("SEI_PASSWORD", WRONG_PASSWORD), (0, 1));
}

#[test]
fn diagnose_never_prints_the_password() {
    assert_no_secret_printed(DIAGNOSE_CASE, ("SEI_UNITS", "all"), (1, 0));
    assert_no_secret_printed(DIAGNOSE_CASE, ("SEI_PASSWORD", WRONG_PASSWORD), (0, 1));
}

fn canned_json(fake: &FakeSei, route: &str, body: &Value) {
    let headers = [("content-type", "application/json")];
    fake.respond_next(route, 200, &headers, body.to_string().into_bytes());
}

#[test]
fn diagnose_stops_at_the_first_rejected_login() {
    let fake = FakeSei::start();
    let case = case_named("test-login with the current unit");
    let env = with_url(&case.env, &fake.url());
    fake.set_password("changed-while-diagnose-runs");
    let login = json!({"sucesso": true, "data": {
        "loginData": {"IdUsuario": "101", "IdUnidadeAtual": "1001", "sigla": "fulano.tal", "nome": "Fulano de Tal"},
        "unidades": [{"id": "1001", "sigla": "SEDEMO/NUPROC", "descricao": "Núcleo"}],
        "token": "a-token-the-fake-never-issued",
    }});
    let version = json!({"sucesso": true, "data": {"sei": SEI_VERSION, "wssei": MODULE_VERSION}});
    let listing = json!({"sucesso": true, "total": "1", "data": [
        {"id": "9001", "atributos": {"idProcedimento": "9001"}},
    ]});
    canned_json(&fake, ROUTE_LOGIN, &login);
    canned_json(&fake, ROUTE_VERSION, &version);
    canned_json(&fake, ROUTE_VERSION, &version);
    canned_json(&fake, ROUTE_PROCESSES, &listing);

    let exited = run_to_exit(&["diagnose"], &pairs(&env), EXIT_LIMIT);

    assert_eq!(
        fake.failed_logins(),
        1,
        "rejected logins\nstdout:\n{}\nstderr:\n{}",
        exited.stdout,
        exited.stderr
    );
    assert_eq!(exited.status.code(), Some(1), "{}", exited.stderr);
    assert_eq!(
        exited.stdout,
        "sei-monitor diagnose\nSEI 5.0.5, mod-wssei 3.0.4; units: 1 available, 1 watched (checks use the first one)\n\n[1] Pagination with limit=2\n"
    );
    assert_eq!(
        exited.stderr,
        "error: sei: /autenticar: Usuário ou Senha Inválida.\n"
    );
    let calls = [
        ROUTE_LOGIN,
        ROUTE_VERSION,
        ROUTE_PROCESSES,
        ROUTE_ACTIVITIES,
        ROUTE_DOCUMENTS,
    ]
    .map(|route| fake.calls(route));
    assert_eq!(
        calls,
        [2, 2, 2, 0, 0],
        "calls on login, version, processes, activities, documents"
    );
    assert_eq!(fake.logins(), 0);
}

#[test]
fn help_lists_every_command() {
    let help = run_to_exit(&["--help"], &[], EXIT_LIMIT);

    assert_eq!(help.status.code(), Some(0), "{}", help.stderr);
    assert_eq!(help.stderr, "");
    for text in [
        "monitor",
        "Serve the monitor-hub monitor protocol; the hub starts this command",
        "test-login",
        "Log in to SEI and show the versions and your units",
        "orgs",
        "List the acronym and id of every agency on the SEI login page",
        "diagnose",
        "Check how the SEI API behaves, printing only counts and types",
        "validate-settings",
        "Check the monitor settings from the hub's document or the environment and print the problems as JSON; the hub runs this command",
    ] {
        assert!(help.stdout.contains(text), "{text:?} in:\n{}", help.stdout);
    }
}

#[test]
fn unknown_flag_is_a_usage_error() {
    let exited = run_to_exit(&["--nope"], &[], EXIT_LIMIT);

    assert_eq!(exited.status.code(), Some(2), "{}", exited.stderr);
    assert_eq!(exited.stdout, "");
    assert_eq!(
        exited.stderr,
        format!("unknown flag: --nope\n{USAGE_HINT}\n")
    );
}

#[test]
fn a_terminate_signal_interrupts_the_command() {
    let fake = FakeSei::start();
    let url = fake.url();
    let interrupt = Interrupt {
        when: "before_first_request".to_owned(),
        note: "SIGTERM while orgs waits for the login page".to_owned(),
    };

    let exited = interrupted(
        &fake,
        &interrupt,
        &["orgs"],
        &[("SEI_URL", url.as_str())],
        libc::SIGTERM,
    );

    assert_eq!(exited.status.code(), Some(130), "{}", exited.stderr);
    assert_eq!(exited.stdout, "");
    assert_eq!(exited.stderr, "interrupted\n");
}
