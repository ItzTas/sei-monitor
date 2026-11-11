mod support;

use std::future::{Future, ready};
use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use flate2::Compression;
use flate2::write::GzEncoder;
use futures_util::future::join_all;
use serde_json::{Value, json};

use sei_monitor::password::{CommandFailure, PasswordCommandError, PasswordSource, StaticPassword};
use sei_monitor::secret::Secret;
use sei_monitor::sei::{
    Client, Credentials, ErrorKind, Options, PROCESS_PAGE_SIZE, Process, ProcessQuery, SeiError,
};
use support::fake_sei::{
    FakeProcess, FakeSei, MODULE_VERSION, ORG_ID, PASSWORD, ROUTE_LOGIN, ROUTE_PROCESSES,
    ROUTE_VERSION, SEI_VERSION, UNIT_GAB, UNIT_NUPROC, USER_ID, USER_NAME, USERNAME,
    process_number,
};

const USER_AGENT: &str = "sei-monitor/test";
const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const SEVENTEEN_MIB: usize = 17 << 20;

struct CountingPassword {
    calls: AtomicUsize,
    password: Result<&'static str, ()>,
}

impl CountingPassword {
    fn returning(password: &'static str) -> Arc<CountingPassword> {
        Arc::new(CountingPassword {
            calls: AtomicUsize::new(0),
            password: Ok(password),
        })
    }

    fn failing() -> Arc<CountingPassword> {
        Arc::new(CountingPassword {
            calls: AtomicUsize::new(0),
            password: Err(()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl PasswordSource for CountingPassword {
    fn password(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, PasswordCommandError>> + Send + '_>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self
            .password
            .map(|password| Secret::new(password.to_owned()))
            .map_err(|()| PasswordCommandError {
                program: "pass".to_owned(),
                exit_code: 1,
                stderr_bytes: 0,
                failure: CommandFailure::Exit,
            });
        Box::pin(ready(answer))
    }
}

fn options(fake: &FakeSei, timeout: Duration) -> Options {
    Options {
        user_agent: USER_AGENT.to_owned(),
        zone: fake.zone(),
        timeout,
    }
}

fn client_with(fake: &FakeSei, password: Arc<dyn PasswordSource>) -> Client {
    let credentials = Credentials {
        username: USERNAME.to_owned(),
        password,
        org_id: ORG_ID,
    };
    Client::new(&fake.api_url(), credentials, &options(fake, TEST_TIMEOUT))
}

fn client(fake: &FakeSei) -> Client {
    client_with(fake, static_password(PASSWORD))
}

fn static_password(password: &str) -> Arc<dyn PasswordSource> {
    Arc::new(StaticPassword(Secret::new(password.to_owned())))
}

fn processes(numbers: impl IntoIterator<Item = u32>) -> Vec<FakeProcess> {
    numbers
        .into_iter()
        .map(|n| FakeProcess {
            id: 9000 + i64::from(n),
            number: process_number(n),
            process_type: "Contrato".to_owned(),
            ..FakeProcess::default()
        })
        .collect()
}

fn ids(processes: &[Process]) -> Vec<i64> {
    processes
        .iter()
        .map(|process| process.procedure_id)
        .collect()
}

async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(TEST_TIMEOUT, work)
        .await
        .expect("the call finished within the test timeout")
}

fn api_message(error: &SeiError) -> &str {
    match error {
        SeiError::Api(api) => &api.message,
        other => panic!("error = {other}, want an API error"),
    }
}

fn canned_page(ids: impl IntoIterator<Item = i64>) -> Vec<u8> {
    let items: Vec<Value> = ids.into_iter().map(|id| json!({"id": id})).collect();
    json!({"sucesso": true, "data": items})
        .to_string()
        .into_bytes()
}

fn oversized_envelope() -> Vec<u8> {
    let mut body = br#"{"sucesso":true,"total":0,"data":[],"padding":""#.to_vec();
    body.resize(SEVENTEEN_MIB - 2, b'a');
    body.extend_from_slice(br#""}"#);
    body
}

fn assert_outside_the_api_format(error: &SeiError) {
    assert_eq!(error.kind(), Some(ErrorKind::UnexpectedResponse), "{error}");
    assert_eq!(
        api_message(error),
        "answer outside the API format (HTTP 200)"
    );
}

#[tokio::test]
async fn login_returns_the_session() {
    let fake = FakeSei::start();

    let session = bounded(client(&fake).login()).await.expect("login");

    assert_eq!(session.user_id, USER_ID);
    assert_eq!(session.user_acronym, USERNAME);
    assert_eq!(session.user_name, USER_NAME);
    assert_eq!(session.current_unit_id, UNIT_NUPROC);
    let units: Vec<(i64, &str)> = session
        .units
        .iter()
        .map(|unit| (unit.id, unit.acronym.as_str()))
        .collect();
    assert_eq!(
        units,
        [(UNIT_NUPROC, "SEDEMO/NUPROC"), (UNIT_GAB, "SEDEMO/GAB")]
    );
    assert_eq!(fake.last_user_agent(), USER_AGENT);
}

#[tokio::test]
async fn session_logs_in_only_without_a_token() {
    let fake = FakeSei::start();
    let client = client(&fake);

    let first = bounded(client.session()).await.expect("first session");
    let second = bounded(client.session()).await.expect("second session");
    bounded(client.login()).await.expect("explicit login");

    assert_eq!(first, second);
    assert_eq!(fake.logins(), 2);
    assert_eq!(bounded(client.logins()).await, 2);
}

#[tokio::test]
async fn version() {
    let fake = FakeSei::start();

    let version = bounded(client(&fake).version()).await.expect("version");

    assert_eq!(version.sei, SEI_VERSION);
    assert_eq!(version.module, MODULE_VERSION);
    assert_eq!(SEI_VERSION, "5.0.5");
    assert_eq!(MODULE_VERSION, "3.0.4");
}

#[tokio::test]
async fn expired_token_is_renewed_once_and_the_call_repeated() {
    let fake = FakeSei::start();
    fake.set_processes(UNIT_NUPROC, processes([1, 2]));
    let password = CountingPassword::returning(PASSWORD);
    let client = client_with(&fake, password.clone());
    bounded(client.processes(UNIT_NUPROC))
        .await
        .expect("first listing");

    fake.expire_tokens();
    let calls = (0..5).map(|_| {
        let client = client.clone();
        tokio::spawn(async move { client.processes(UNIT_NUPROC).await })
    });
    let results = bounded(join_all(calls)).await;

    for result in results {
        let listed = result
            .expect("the task ran")
            .expect("listing after renewal");
        assert_eq!(ids(&listed), [9001, 9002]);
    }
    assert_eq!(fake.logins(), 2);
    assert_eq!(password.calls(), 2);
}

#[tokio::test]
async fn changed_password_stops_after_one_login_attempt() {
    let fake = FakeSei::start();
    let client = client(&fake);
    bounded(client.login()).await.expect("login");

    fake.set_password("new-password");
    fake.expire_tokens();
    let error = bounded(client.processes(UNIT_NUPROC))
        .await
        .expect_err("the old password is rejected");

    assert_eq!(error.kind(), Some(ErrorKind::InvalidCredentials));
    assert_eq!(fake.failed_logins(), 1);
}

#[tokio::test]
async fn concurrent_calls_after_a_changed_password_try_one_login_each() {
    let fake = FakeSei::start();
    let client = client(&fake);
    bounded(client.login()).await.expect("login");

    fake.set_password("new-password");
    let calls = (0..5).map(|_| client.processes(UNIT_NUPROC));
    let results = bounded(join_all(calls)).await;

    for result in results {
        let error = result.expect_err("the old password is rejected");
        assert_eq!(error.kind(), Some(ErrorKind::InvalidCredentials), "{error}");
    }
    assert_eq!(fake.failed_logins(), 5);
    assert_eq!(fake.logins(), 1);
    assert_eq!(bounded(client.logins()).await, 1);
}

#[tokio::test]
async fn call_rejected_again_after_the_renewal_is_returned() {
    let fake = FakeSei::start();
    let client = client(&fake);
    bounded(client.login()).await.expect("login");
    fake.fail_next(ROUTE_PROCESSES, 403, 2);

    let error = bounded(client.process_page(UNIT_NUPROC, &first_page()))
        .await
        .expect_err("both answers are 403");

    assert_eq!(error.kind(), Some(ErrorKind::Unauthorized));
    assert_eq!(fake.logins(), 2);
    assert_eq!(fake.calls(ROUTE_PROCESSES), 2);
}

#[tokio::test]
async fn password_source_failure_is_not_an_invalid_credential() {
    let fake = FakeSei::start();
    let password = CountingPassword::failing();
    let client = client_with(&fake, password.clone());

    let error = bounded(client.processes(UNIT_NUPROC))
        .await
        .expect_err("the password source fails");

    assert_eq!(error.kind(), Some(ErrorKind::PasswordCommand));
    assert_eq!(
        error.to_string(),
        "sei: /autenticar: SEI_PASSWORD_CMD: pass exited with code 1"
    );
    assert_eq!(fake.calls(ROUTE_LOGIN), 0);
    assert_eq!(password.calls(), 1);
}

#[tokio::test]
async fn processes_walks_every_page() {
    let fake = FakeSei::start();
    fake.set_processes(UNIT_NUPROC, processes(1..=120));

    let listed = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect("listing");

    assert_eq!(listed.len(), 120);
    assert_eq!(listed[0].procedure_id, 9001);
    assert_eq!(listed[119].procedure_id, 9120);
    assert_eq!(fake.calls(ROUTE_PROCESSES), 3);
}

#[tokio::test]
async fn processes_stop_once_the_total_is_collected() {
    let fake = FakeSei::start();
    fake.set_processes(UNIT_NUPROC, processes(1..=100));

    let listed = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect("listing");

    assert_eq!(listed.len(), 100);
    assert_eq!(fake.calls(ROUTE_PROCESSES), 2);
}

#[tokio::test]
async fn processes_drop_id_zero_and_repeats_and_stop_when_a_page_adds_nothing() {
    let fake = FakeSei::start();
    let full_page = || std::iter::once(0).chain(1..=49);
    let headers = [("content-type", "application/json")];
    fake.respond_next(ROUTE_PROCESSES, 200, &headers, canned_page(full_page()));
    fake.respond_next(ROUTE_PROCESSES, 200, &headers, canned_page(full_page()));

    let listed = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect("listing");

    assert_eq!(ids(&listed), (1..=49).collect::<Vec<i64>>());
    assert_eq!(fake.calls(ROUTE_PROCESSES), 2);
}

#[tokio::test]
async fn processes_past_400_pages_fail() {
    let fake = FakeSei::start();
    let headers = [("content-type", "application/json")];
    let size = i64::try_from(PROCESS_PAGE_SIZE).expect("a small page size");
    for page in 0..401 {
        let first = page * size + 1;
        fake.respond_next(
            ROUTE_PROCESSES,
            200,
            &headers,
            canned_page(first..first + size),
        );
    }

    let error = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect_err("the list never ends");

    assert!(matches!(error, SeiError::TooManyPages), "{error}");
    assert_eq!(fake.calls(ROUTE_PROCESSES), 400);
}

#[tokio::test]
async fn unit_header_selects_the_unit() {
    let fake = FakeSei::start();
    fake.set_processes(UNIT_NUPROC, processes([1]));
    fake.set_processes(UNIT_GAB, processes([2, 3]));
    let client = client(&fake);

    let gab = bounded(client.processes(UNIT_GAB)).await.expect("GAB");
    let nuproc = bounded(client.processes(UNIT_NUPROC))
        .await
        .expect("NUPROC");

    assert_eq!(gab.len(), 2);
    assert_eq!(nuproc.len(), 1);
    assert_eq!(nuproc[0].procedure_id, 9001);
}

#[tokio::test]
async fn return_date_of_another_process_is_ignored() {
    let fake = FakeSei::start();
    let mut listed = processes([1, 2]);
    listed[0].return_date = "06/11/2026".to_owned();
    fake.set_processes(UNIT_NUPROC, listed);

    let got = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect("listing");

    assert_eq!(got[0].return_date, NaiveDate::from_ymd_opt(2026, 11, 6));
    assert_eq!(got[1].return_date, None);
}

#[tokio::test]
async fn server_errors_are_transient() {
    let fake = FakeSei::start();
    fake.fail_next(ROUTE_PROCESSES, 502, 1);

    let error = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect_err("HTTP 502");

    assert_eq!(error.kind(), Some(ErrorKind::Server), "{error}");
}

#[tokio::test]
async fn refusal_without_a_known_cause_is_refused() {
    let fake = FakeSei::start();
    let body = json!({"sucesso": false, "mensagem": "Não foi possível"});
    fake.respond_next(
        ROUTE_PROCESSES,
        200,
        &[("content-type", "application/json")],
        body.to_string().into_bytes(),
    );

    let error = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect_err("refused");

    assert_eq!(error.kind(), Some(ErrorKind::Refused), "{error}");
}

#[tokio::test]
async fn logins_counts_every_successful_login() {
    let fake = FakeSei::start();
    let client = client(&fake);
    assert_eq!(bounded(client.logins()).await, 0);

    bounded(client.processes(UNIT_NUPROC))
        .await
        .expect("first listing");
    assert_eq!(bounded(client.logins()).await, 1);

    fake.expire_tokens();
    bounded(client.processes(UNIT_NUPROC))
        .await
        .expect("second listing");
    assert_eq!(bounded(client.logins()).await, 2);
}

struct LoginFailure {
    name: &'static str,
    setup: fn(&FakeSei),
    password: &'static str,
    kind: ErrorKind,
    text: &'static str,
}

#[tokio::test]
async fn login_failures() {
    let cases = [
        LoginFailure {
            name: "rejected credentials",
            setup: |_| {},
            password: "wrong-password",
            kind: ErrorKind::InvalidCredentials,
            text: "sei: /autenticar: Usuário ou Senha Inválida.",
        },
        LoginFailure {
            name: "disabled module",
            setup: |fake| fake.set_module_inactive(true),
            password: PASSWORD,
            kind: ErrorKind::ModuleUnavailable,
            text: "sei: /autenticar: Módulo inativo.",
        },
        LoginFailure {
            name: "HTML page instead of the API",
            setup: |fake| {
                let html = [("content-type", "text/html")];
                fake.respond_next(ROUTE_LOGIN, 200, &html, b"<html></html>".to_vec());
            },
            password: PASSWORD,
            kind: ErrorKind::UnexpectedResponse,
            text: "sei: /autenticar: answer outside the API format (HTTP 200)",
        },
        LoginFailure {
            name: "internal error",
            setup: |fake| fake.fail_next(ROUTE_LOGIN, 502, 1),
            password: PASSWORD,
            kind: ErrorKind::Server,
            text: "sei: /autenticar: answer outside the API format (HTTP 502)",
        },
    ];
    for case in cases {
        let fake = FakeSei::start();
        (case.setup)(&fake);

        let error = bounded(client_with(&fake, static_password(case.password)).login())
            .await
            .expect_err(case.name);

        assert_eq!(error.kind(), Some(case.kind), "{}: {error}", case.name);
        assert_eq!(error.to_string(), case.text, "{}", case.name);
    }
}

#[tokio::test]
async fn api_calls_do_not_follow_redirects() {
    let fake = FakeSei::start();
    let elsewhere = format!("{}{ROUTE_VERSION}", fake.api_url());
    fake.respond_next(ROUTE_LOGIN, 302, &[("location", &elsewhere)], Vec::new());

    let error = bounded(client(&fake).login())
        .await
        .expect_err("a redirect");

    assert_eq!(error.kind(), Some(ErrorKind::Redirect));
    assert_eq!(
        api_message(&error),
        "SEI answered with a redirect, which sei-monitor does not follow on API calls; check SEI_URL and SEI_API_URL"
    );
    assert_eq!(fake.calls(ROUTE_VERSION), 0);
}

#[tokio::test]
async fn password_with_reserved_characters_logs_in() {
    let fake = FakeSei::start();
    let password = "a+b&c=d*e~f %ç";
    fake.set_password(password);

    bounded(client_with(&fake, static_password(password)).login())
        .await
        .expect("login with the reserved characters");

    assert_eq!(fake.logins(), 1);
}

#[tokio::test]
async fn errors_never_show_the_token_or_the_query() {
    let fake = FakeSei::start();
    let client = client(&fake);
    bounded(client.login()).await.expect("login");
    fake.fail_next(ROUTE_PROCESSES, 403, 2);

    let rejected = bounded(client.process_page(UNIT_NUPROC, &first_page()))
        .await
        .expect_err("HTTP 403");

    let tokens = fake.issued_tokens();
    assert_eq!(tokens.len(), 2);
    for text in [rejected.to_string(), format!("{rejected:?}")] {
        for token in &tokens {
            assert!(!text.contains(token.as_str()), "{text}");
        }
    }

    drop(fake);
    let unreachable = bounded(client.process_page(UNIT_NUPROC, &first_page()))
        .await
        .expect_err("the server is closed");

    assert!(
        matches!(&unreachable, SeiError::Transport { context, .. } if context == ROUTE_PROCESSES),
        "{unreachable:?}"
    );
    for text in [unreachable.to_string(), format!("{unreachable:?}")] {
        assert!(!text.contains('?'), "{text}");
        assert!(!text.contains("limit="), "{text}");
        for token in &tokens {
            assert!(!text.contains(token.as_str()), "{text}");
        }
    }
}

#[tokio::test]
async fn oversized_body_is_cut_and_reported() {
    let fake = FakeSei::start();
    let body = oversized_envelope();
    serde_json::from_slice::<Value>(&body).expect("the oversized body is a valid envelope");
    fake.respond_next(
        ROUTE_PROCESSES,
        200,
        &[("content-type", "application/json")],
        body,
    );

    let error = bounded(client(&fake).processes(UNIT_NUPROC))
        .await
        .expect_err("cut at 16 MiB");

    assert_outside_the_api_format(&error);
}

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(body).expect("compress the body");
    encoder.finish().expect("finish the gzip stream")
}

#[tokio::test]
async fn compressed_body_is_decoded_and_cut_after_decoding() {
    let fake = FakeSei::start();
    let client = client(&fake);
    let headers = [
        ("content-type", "application/json"),
        ("content-encoding", "gzip"),
    ];
    let oversized = gzip(&oversized_envelope());
    assert!(oversized.len() < 1 << 20, "{}", oversized.len());
    fake.respond_next(ROUTE_PROCESSES, 200, &headers, gzip(&canned_page([7, 8])));
    fake.respond_next(ROUTE_PROCESSES, 200, &headers, oversized);

    let small = bounded(client.processes(UNIT_NUPROC))
        .await
        .expect("a small compressed page");
    let error = bounded(client.processes(UNIT_NUPROC))
        .await
        .expect_err("cut at 16 MiB after decoding");

    assert_eq!(ids(&small), [7, 8]);
    assert_outside_the_api_format(&error);
}

#[tokio::test]
async fn stalled_body_fails_within_the_client_timeout() {
    let fake = FakeSei::start();
    let credentials = Credentials {
        username: USERNAME.to_owned(),
        password: static_password(PASSWORD),
        org_id: ORG_ID,
    };
    let client = Client::new(
        &fake.api_url(),
        credentials,
        &options(&fake, Duration::from_secs(1)),
    );
    bounded(client.login()).await.expect("login");
    fake.stall(ROUTE_PROCESSES);

    let started = Instant::now();
    let error = bounded(client.processes(UNIT_NUPROC))
        .await
        .expect_err("the body never ends");

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let text = error.to_string();
    assert!(
        text.starts_with("sei: read the /processo/listar answer: "),
        "{text}"
    );
    assert!(!text.contains('?'), "{text}");
}

fn first_page() -> ProcessQuery {
    ProcessQuery {
        page: 0,
        limit: PROCESS_PAGE_SIZE,
        kind: String::new(),
    }
}
