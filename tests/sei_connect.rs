mod support;

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use chrono_tz::Tz;
use futures_util::{StreamExt, stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use sei_monitor::password::{PasswordSource, StaticPassword};
use sei_monitor::secret::Secret;
use sei_monitor::sei::{
    ErrorKind, Options, Org, SeiError, Session, Target, Unit, connect, discover, list_orgs,
    resolve_org,
};
use support::fake_sei::{
    API_PATH, FakeSei, ORG_ACRONYM, ORG_ID, PASSWORD, ROUTE_LOGIN_PAGE, ROUTE_VERSION, USERNAME,
    default_orgs, fixture,
};

const USER_AGENT: &str = "sei-monitor/test";
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn options(zone: Tz) -> Options {
    Options {
        user_agent: USER_AGENT.to_owned(),
        zone,
        timeout: TEST_TIMEOUT,
    }
}

async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(TEST_TIMEOUT, work)
        .await
        .expect("the call finished within the test timeout")
}

fn org(id: i64, acronym: &str) -> Org {
    Org {
        id,
        acronym: acronym.to_owned(),
    }
}

fn password() -> Arc<dyn PasswordSource> {
    Arc::new(StaticPassword(Secret::new(PASSWORD.to_owned())))
}

fn target(fake: &FakeSei, api_url: String, org: &str) -> Target {
    Target {
        sei_url: fake.url(),
        api_url,
        org: org.to_owned(),
        username: USERNAME.to_owned(),
        password: password(),
    }
}

struct HtmlNotFound {
    url: String,
    server: JoinHandle<()>,
}

impl HtmlNotFound {
    async fn start() -> HtmlNotFound {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the HTML 404 listener");
        let address = listener.local_addr().expect("read the HTML 404 address");
        let router = Router::new().fallback(|| async {
            let html = [(CONTENT_TYPE, "text/html; charset=utf-8")];
            let body = "<html><body><h1>Página não encontrada</h1></body></html>";
            (StatusCode::NOT_FOUND, html, body).into_response()
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve the HTML 404 pages");
        });
        HtmlNotFound {
            url: format!("http://{address}"),
            server,
        }
    }
}

impl Drop for HtmlNotFound {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn discover_and_resolve_org() {
    let fake = FakeSei::start();
    let options = options(fake.zone());
    let url = fake.url();

    let api_url = bounded(discover(&url, &options)).await.expect("discover");
    assert_eq!(api_url, fake.api_url());

    let by_id = bounded(resolve_org(&url, "63", &options))
        .await
        .expect("org by id");
    assert_eq!(by_id, org(ORG_ID, ""));
    assert_eq!(fake.calls(ROUTE_LOGIN_PAGE), 0);

    let by_acronym = bounded(resolve_org(&url, "sedemo", &options))
        .await
        .expect("org by acronym");
    assert_eq!(by_acronym, org(ORG_ID, ORG_ACRONYM));

    let orgs = bounded(list_orgs(&url, &options)).await.expect("orgs");
    let every_org: Vec<Org> = default_orgs()
        .into_iter()
        .map(|(id, acronym, _)| org(id, acronym))
        .collect();
    assert_eq!(orgs, every_org);
    assert_eq!(every_org.len(), 12);

    let missing = bounded(resolve_org(&url, "NOSUCHORG", &options))
        .await
        .expect_err("an unknown agency");
    assert_eq!(missing.kind(), Some(ErrorKind::OrgNotFound));
    assert_eq!(
        missing.to_string(),
        "agency not found on the SEI login page: \"NOSUCHORG\" \
         (the orgs subcommand lists the available acronyms)"
    );
}

#[tokio::test]
async fn list_orgs_reads_the_latin1_login_page_after_redirect() {
    let fake = FakeSei::start();
    fake.respond_next(
        ROUTE_LOGIN_PAGE,
        200,
        &[("content-type", "text/html; charset=iso-8859-1")],
        fixture("login.html"),
    );

    let orgs = bounded(list_orgs(&fake.url(), &options(fake.zone())))
        .await
        .expect("orgs");

    assert_eq!(
        orgs,
        [
            org(34, "ADEMO"),
            org(48, "BDEMO"),
            org(12, "CDEMO"),
            org(63, "SEDEMO"),
            org(77, "ÓRGÃODEMO"),
        ]
    );
    assert_eq!(fake.calls(ROUTE_LOGIN_PAGE), 1);
    assert_eq!(fake.last_user_agent(), USER_AGENT);
}

#[tokio::test]
async fn disabled_module() {
    let fake = FakeSei::start();
    fake.set_module_inactive(true);

    let error = bounded(discover(&fake.url(), &options(fake.zone())))
        .await
        .expect_err("a disabled module");

    assert_eq!(error.kind(), Some(ErrorKind::ModuleUnavailable), "{error}");
    let SeiError::Api(api) = &error else {
        panic!("error = {error}, want an API error");
    };
    assert_eq!(api.route, ROUTE_VERSION);
    assert_eq!(api.status, 401);
    assert_eq!(error.to_string(), "sei: /versao: Módulo inativo.");
}

#[tokio::test]
async fn module_not_found_names_both_candidates() {
    let server = HtmlNotFound::start().await;

    let error = bounded(discover(&server.url, &options(chrono_tz::UTC)))
        .await
        .expect_err("no module");

    assert_eq!(error.kind(), Some(ErrorKind::ModuleNotFound), "{error}");
    assert_eq!(
        error.to_string(),
        format!(
            "the mod-wssei module was not found on this SEI: tried \
             {url}/sei/modulos/wssei/controlador_ws.php/api/v2 and \
             {url}/sei/modulos/mod-wssei/controlador_ws.php/api/v2; \
             set SEI_API_URL if the module lives elsewhere",
            url = server.url
        )
    );
}

#[tokio::test]
async fn connect_discovers_the_module_and_resolves_the_org() {
    let fake = FakeSei::start();
    let options = options(fake.zone());

    let connection = bounded(connect(
        &target(&fake, String::new(), ORG_ACRONYM),
        &options,
    ))
    .await
    .expect("connect");

    assert_eq!(connection.api_url, fake.api_url());
    assert_eq!(connection.org, org(ORG_ID, ORG_ACRONYM));
    assert_eq!(connection.client.base_url(), fake.api_url());
    assert_eq!(fake.logins(), 0);
    bounded(connection.client.login())
        .await
        .expect("login through the connection");
    assert_eq!(fake.logins(), 1);
}

#[tokio::test]
async fn connect_uses_the_given_api_url() {
    let fake = FakeSei::start();
    let api_url = format!("{}{API_PATH}", fake.url());

    let connection = bounded(connect(
        &target(&fake, api_url.clone(), "63"),
        &options(fake.zone()),
    ))
    .await
    .expect("connect");

    assert_eq!(connection.api_url, api_url);
    assert_eq!(connection.org, org(ORG_ID, ""));
    assert_eq!(fake.calls(ROUTE_VERSION), 0);
    assert_eq!(fake.calls(ROUTE_LOGIN_PAGE), 0);
}

fn unit(id: i64, acronym: &str) -> Unit {
    Unit {
        id,
        acronym: acronym.to_owned(),
        description: String::new(),
    }
}

fn session(units: Vec<Unit>) -> Session {
    Session {
        user_id: 101,
        user_acronym: USERNAME.to_owned(),
        user_name: String::new(),
        current_unit_id: 2,
        units,
    }
}

fn unit_ids(units: &[Unit]) -> Vec<i64> {
    units.iter().map(|unit| unit.id).collect()
}

#[test]
fn select_units() {
    let session = session(vec![
        unit(1, "SEDEMO/NUPROC"),
        unit(2, "SEDEMO/GAB"),
        unit(3, "SEDEMO/CPL"),
    ]);
    let cases: [(&str, Vec<String>, bool, Vec<i64>); 3] = [
        ("current unit by default", Vec::new(), false, vec![2]),
        ("every unit", Vec::new(), true, vec![1, 2, 3]),
        (
            "chosen units in the given order",
            vec!["sedemo/cpl".to_owned(), "SEDEMO/NUPROC".to_owned()],
            false,
            vec![3, 1],
        ),
    ];
    for (name, acronyms, all, ids) in cases {
        let units = session
            .select_units(&acronyms, all)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(unit_ids(&units), ids, "{name}");
    }
}

#[test]
fn select_units_keeps_an_unknown_current_unit_by_id() {
    let session = session(vec![unit(1, "SEDEMO/NUPROC")]);

    let units = session.select_units(&[], false).expect("the current unit");

    assert_eq!(units, [unit(2, "")]);
}

#[test]
fn select_units_names_the_user_units_when_one_is_unknown() {
    let session = session(vec![unit(1, "SEDEMO/NUPROC"), unit(2, "SEDEMO/GAB")]);

    let error = session
        .select_units(&["SEDEMO/XYZ".to_owned()], false)
        .expect_err("an unknown unit");

    assert_eq!(error.kind(), Some(ErrorKind::UnknownUnit));
    assert_eq!(
        error.to_string(),
        "unit not found among the user's units: \"SEDEMO/XYZ\" \
         (yours are SEDEMO/NUPROC, SEDEMO/GAB)"
    );
}

struct RedirectChain {
    url: String,
    requests: Arc<AtomicUsize>,
    server: JoinHandle<()>,
}

impl RedirectChain {
    async fn start(redirects: usize) -> RedirectChain {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the redirect chain listener");
        let address = listener
            .local_addr()
            .expect("read the redirect chain address");
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        let router = Router::new().fallback(move |uri: Uri| {
            counted.fetch_add(1, Ordering::SeqCst);
            async move { chain_step(&uri, redirects) }
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve the redirect chain");
        });
        RedirectChain {
            url: format!("http://{address}"),
            requests,
            server,
        }
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for RedirectChain {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn chain_step(uri: &Uri, redirects: usize) -> axum::response::Response {
    let step = uri
        .path()
        .strip_prefix("/hop/")
        .map_or(0, |step| step.parse().expect("a numbered hop"));
    if step < redirects {
        let next = format!("/hop/{}", step + 1);
        return (StatusCode::FOUND, [(LOCATION, next)]).into_response();
    }
    let latin1 = [(CONTENT_TYPE, "text/html; charset=iso-8859-1")];
    (latin1, fixture("login.html")).into_response()
}

#[tokio::test]
async fn login_page_follows_at_most_nine_redirects() {
    let nine = RedirectChain::start(9).await;
    let orgs = bounded(list_orgs(&nine.url, &options(Tz::UTC)))
        .await
        .expect("orgs after nine redirects");
    assert_eq!(orgs.len(), 5);
    assert_eq!(orgs[3], org(ORG_ID, ORG_ACRONYM));
    assert_eq!(nine.requests(), 10);

    let ten = RedirectChain::start(10).await;
    let error = bounded(list_orgs(&ten.url, &options(Tz::UTC)))
        .await
        .expect_err("a tenth redirect");
    assert!(
        matches!(&error, SeiError::Transport { context, .. } if context == "open the login page"),
        "{error:?}"
    );
    assert_eq!(ten.requests(), 10);
}

type RawAnswer = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

struct RawServer {
    url: String,
    server: JoinHandle<()>,
}

impl RawServer {
    async fn start(answer: RawAnswer) -> RawServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the raw listener");
        let address = listener.local_addr().expect("read the raw address");
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let answer = Arc::clone(&answer);
                tokio::spawn(async move {
                    let mut request = [0; 4096];
                    let read = stream.read(&mut request).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..read]).into_owned();
                    if let Some(bytes) = answer(&request) {
                        let _ = stream.write_all(&bytes).await;
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });
        RawServer {
            url: format!("http://{address}"),
            server,
        }
    }

    async fn always(answer: Vec<u8>) -> RawServer {
        RawServer::start(Arc::new(move |_| Some(answer.clone()))).await
    }
}

impl Drop for RawServer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn announced_longer(page: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{page}",
        page.len() + 5000
    )
    .into_bytes()
}

#[tokio::test]
async fn login_page_cut_short_in_its_first_kilobyte_is_read_with_what_arrived() {
    let page = r#"<select id="selOrgao"><option value="1">A</option><option value="2">B</option></select>"#;
    let server = RawServer::always(announced_longer(page)).await;

    let orgs = bounded(list_orgs(&server.url, &options(chrono_tz::UTC)))
        .await
        .expect("the agencies that arrived");

    assert_eq!(orgs, [org(1, "A"), org(2, "B")]);
}

#[tokio::test]
async fn login_page_cut_short_before_any_byte_has_no_agency_list() {
    let server = RawServer::always(announced_longer("")).await;

    let error = bounded(list_orgs(&server.url, &options(chrono_tz::UTC)))
        .await
        .expect_err("no agency list");

    assert_eq!(
        error.to_string(),
        "sei: the login page has no agency list (selOrgao field)"
    );
}

#[tokio::test]
async fn login_page_deeper_than_go_allows_is_refused_with_go_text() {
    let fake = FakeSei::start();
    let page = format!(
        r#"{}<select id="selOrgao"><option value="1">A</option></select>"#,
        "<div>".repeat(600)
    );
    fake.respond_next(ROUTE_LOGIN_PAGE, 200, &[], page.into_bytes());

    let error = bounded(list_orgs(&fake.url(), &options(fake.zone())))
        .await
        .expect_err("a page too deep");

    assert_eq!(
        error.to_string(),
        "sei: read the login page: html: open stack of elements exceeds 512 nodes"
    );
}

#[tokio::test]
async fn login_page_parse_stops_at_the_request_deadline() {
    let fake = FakeSei::start();
    let attributes: String = (0..90_000).map(|n| format!(" a{n}")).collect();
    let page =
        format!(r#"<select id="selOrgao"{attributes}><option value="1">A</option></select>"#);
    fake.respond_next(ROUTE_LOGIN_PAGE, 200, &[], page.into_bytes());
    let options = Options {
        timeout: Duration::from_millis(300),
        ..options(fake.zone())
    };
    let started = Instant::now();

    let error = bounded(list_orgs(&fake.url(), &options))
        .await
        .expect_err("a page slower than the request deadline");

    assert!(
        started.elapsed() < Duration::from_millis(1300),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        error.to_string(),
        "sei: read the login page: the time limit of the request was reached"
    );
}

const WSSEI_VERSION: &str = "/sei/modulos/wssei/controlador_ws.php/api/v2/versao";
const MOD_WSSEI_VERSION: &str = "/sei/modulos/mod-wssei/controlador_ws.php/api/v2/versao";
const MEETING_BOUND: Duration = Duration::from_secs(3);
const VERSION_ANSWER: &str = r#"{"sucesso":true,"data":{"versao":"5.0.5","versaoModulo":"3.0.4"}}"#;
const ONE_AGENCY_PAGE: &str =
    r#"<select id="selOrgao"><option value="63">SEDEMO</option></select>"#;

#[derive(Default)]
struct Rendezvous {
    arrived: [Notify; 2],
}

impl Rendezvous {
    async fn meet(&self, side: usize) -> bool {
        self.arrived[side].notify_one();
        tokio::time::timeout(MEETING_BOUND, self.arrived[1 - side].notified())
            .await
            .is_ok()
    }
}

struct Routes {
    url: String,
    server: JoinHandle<()>,
}

impl Routes {
    async fn start(router: Router) -> Routes {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the routes listener");
        let address = listener.local_addr().expect("read the routes address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve the routes");
        });
        Routes {
            url: format!("http://{address}"),
            server,
        }
    }
}

impl Drop for Routes {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn html_not_found() -> axum::response::Response {
    let html = [(CONTENT_TYPE, "text/html")];
    (StatusCode::NOT_FOUND, html, "<html><body>404</body></html>").into_response()
}

fn version_answer() -> axum::response::Response {
    let json = [(CONTENT_TYPE, "application/json")];
    (StatusCode::OK, json, VERSION_ANSWER).into_response()
}

#[tokio::test]
async fn both_discovery_probes_are_in_flight_at_the_same_time() {
    let meeting = Arc::new(Rendezvous::default());
    let first = Arc::clone(&meeting);
    let second = Arc::clone(&meeting);
    let router = Router::new()
        .route(
            WSSEI_VERSION,
            get(move || async move {
                if first.meet(0).await {
                    return version_answer();
                }
                html_not_found()
            }),
        )
        .route(
            MOD_WSSEI_VERSION,
            get(move || async move {
                second.meet(1).await;
                html_not_found()
            }),
        );
    let server = Routes::start(router).await;

    let api_url = bounded(discover(&server.url, &options(chrono_tz::UTC)))
        .await
        .expect("both probes met");

    assert_eq!(api_url, format!("{}{API_PATH}", server.url));
}

#[tokio::test]
async fn connect_discovers_and_reads_the_login_page_at_the_same_time() {
    let meeting = Arc::new(Rendezvous::default());
    let probe = Arc::clone(&meeting);
    let page = Arc::clone(&meeting);
    let router = Router::new()
        .route(
            WSSEI_VERSION,
            get(move || async move {
                if probe.meet(0).await {
                    return version_answer();
                }
                html_not_found()
            }),
        )
        .route(MOD_WSSEI_VERSION, get(|| async { html_not_found() }))
        .route(
            "/sei/",
            get(move || async move {
                if page.meet(1).await {
                    let html = [(CONTENT_TYPE, "text/html")];
                    return (StatusCode::OK, html, ONE_AGENCY_PAGE).into_response();
                }
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }),
        );
    let server = Routes::start(router).await;
    let target = Target {
        sei_url: server.url.clone(),
        api_url: String::new(),
        org: ORG_ACRONYM.to_owned(),
        username: USERNAME.to_owned(),
        password: password(),
    };

    let connection = bounded(connect(&target, &options(chrono_tz::UTC)))
        .await
        .expect("discovery and the login page met");

    assert_eq!(connection.api_url, format!("{}{API_PATH}", server.url));
    assert_eq!(connection.org, org(ORG_ID, ORG_ACRONYM));
}

#[tokio::test]
async fn disabled_module_wins_over_a_connection_failure_on_the_other_candidate() {
    let inactive = r#"{"sucesso":false,"mensagem":"Módulo inativo.","exception":null}"#;
    let answer = format!(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{inactive}",
        inactive.len()
    )
    .into_bytes();
    let server = RawServer::start(Arc::new(move |request: &str| {
        request
            .starts_with(&format!("GET {MOD_WSSEI_VERSION} "))
            .then(|| answer.clone())
    }))
    .await;

    let error = bounded(discover(&server.url, &options(chrono_tz::UTC)))
        .await
        .expect_err("a disabled module");

    assert_eq!(error.kind(), Some(ErrorKind::ModuleUnavailable), "{error}");
    assert_eq!(error.to_string(), "sei: /versao: Módulo inativo.");
}

fn stalled_after(first: Vec<u8>) -> axum::response::Response {
    let body = stream::iter([Ok::<_, Infallible>(Bytes::from(first))]).chain(stream::pending());
    let html = [(CONTENT_TYPE, "text/html")];
    (html, Body::from_stream(body)).into_response()
}

#[tokio::test]
async fn login_page_read_failures_are_named_like_go() {
    let router = Router::new()
        .route(
            "/short/sei/",
            get(|| async { stalled_after(b"<html><head>".to_vec()) }),
        )
        .route(
            "/long/sei/",
            get(|| async { stalled_after(vec![b'a'; 2000]) }),
        );
    let server = Routes::start(router).await;
    let options = Options {
        timeout: Duration::from_millis(300),
        ..options(chrono_tz::UTC)
    };
    let cases = [
        ("short", "sei: decode the login page: "),
        ("long", "sei: read the login page: "),
    ];
    for (path, prefix) in cases {
        let error = bounded(list_orgs(&format!("{}/{path}", server.url), &options))
            .await
            .expect_err("a stalled login page");

        assert!(
            matches!(error, SeiError::Transport { .. }),
            "{path}: {error}"
        );
        assert!(error.to_string().starts_with(prefix), "{path}: {error}");
    }
}

#[tokio::test]
async fn login_page_past_the_cap_is_cut_at_the_cap() {
    let mut page = br#"<select id="selOrgao"><option value="1">A</option>"#.to_vec();
    page.resize(17 << 20, b'x');
    page.extend_from_slice(br#"<option value="2">B</option></select>"#);
    let page = Bytes::from(page);
    let router = Router::new().route(
        "/sei/",
        get(move || {
            let page = page.clone();
            async move { ([(CONTENT_TYPE, "text/html")], page).into_response() }
        }),
    );
    let server = Routes::start(router).await;

    let orgs = bounded(list_orgs(&server.url, &options(chrono_tz::UTC)))
        .await
        .expect("the agencies before the cap");

    assert_eq!(orgs, [org(1, "A")]);
}
