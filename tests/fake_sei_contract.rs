mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use reqwest::header::{ALLOW, CONTENT_TYPE, LOCATION};
use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};

use support::fake_sei::{
    FakeActivity, FakeDocument, FakeProcess, FakeSei, FakeUnit, FakeUser, ORG_ID, PASSWORD,
    ROUTE_LOGIN, ROUTE_LOGIN_PAGE, ROUTE_ORGS, ROUTE_PROCESSES, ROUTE_VERSION, UNIT_NUPROC,
    USER_ID, USER_NAME, USERNAME, default_orgs, fixture, process_number,
};

const CYCLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/golden/cycle.json"
));
const COMMANDS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/golden/commands.json"
));
const USER_AGENT: &str = "sei-monitor/test";

type FieldTypes = BTreeMap<String, BTreeSet<&'static str>>;

#[tokio::test]
async fn fake_items_use_only_fields_of_the_real_format() {
    let fake = FakeSei::start();
    fake.set_processes(
        UNIT_NUPROC,
        vec![FakeProcess {
            id: 9001,
            number: process_number(1),
            process_type: "Contrato".to_owned(),
            return_date: "06/11/2026".to_owned(),
            assignee: Some(FakeUser {
                id: USER_ID,
                acronym: USERNAME.to_owned(),
                name: USER_NAME.to_owned(),
            }),
            ..FakeProcess::default()
        }],
    );
    fake.add_activities(
        9001,
        vec![activity(50001, USERNAME, utc(2026, 11, 4, 17, 13, 5))],
    );
    fake.add_documents(9001, vec![document(80001)]);
    let client = client();
    let login = login(&client, &fake, PASSWORD).await.json();
    let token = token_of(&login);
    let mut answers = vec![("/autenticar", "autenticar.json", login.clone())];
    for (path, name) in [
        ("/processo/listar", "processo_listar.json"),
        (
            "/atividade/listar?procedimento=9001",
            "atividade_listar.json",
        ),
        ("/documento/listar/9001", "documento_listar.json"),
        ("/versao", "versao.json"),
    ] {
        let body = get(&client, &fake, path, &[("token", &token)]).await;
        answers.push((path, name, body.json()));
    }
    for (path, name, answer) in answers {
        let fake_types = field_types(&answer);
        let real_types = field_types(&serde_json::from_slice(&fixture(name)).unwrap());
        let unknown = fields_missing_from(&fake_types, &real_types);
        assert!(
            unknown.is_empty(),
            "{path}: fake fields whose type does not appear in {name}: {unknown:?}"
        );
    }
}

#[tokio::test]
async fn fake_behaves_like_seitest() {
    let fake = FakeSei::start();
    let processes = (1..=120).map(process).collect();
    fake.set_processes(UNIT_NUPROC, processes);
    let client = client();

    let login = login(&client, &fake, PASSWORD).await.json();
    assert_eq!(login["sucesso"], true);
    let token = token_of(&login);
    assert_eq!(fake.issued_tokens(), [token.as_str()]);

    let mut sizes = Vec::new();
    for start in 0..3 {
        let path = format!("/processo/listar?limit=50&start={start}");
        let page = get(&client, &fake, &path, &[("token", &token)])
            .await
            .json();
        assert_eq!(page["total"], "120");
        sizes.push(page["data"].as_array().map_or(0, Vec::len));
    }
    assert_eq!(sizes, [50, 50, 20]);

    let anonymous = get(&client, &fake, "/processo/listar", &[]).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.json()["mensagem"], "Acesso negado!");

    fake.expire_tokens();
    let expired = get(&client, &fake, "/processo/listar", &[("token", &token)]).await;
    assert_eq!(expired.status, StatusCode::FORBIDDEN);
    assert_eq!(expired.json()["mensagem"], "Token inválido!");

    let redirect = fetch(&client, &format!("{}/sei/", fake.url())).await;
    assert_eq!(redirect.status, StatusCode::FOUND);
    let location = redirect.header(LOCATION);
    assert_eq!(
        location,
        "/sip/login.php?sigla_orgao_sistema=SEDEMO&sigla_sistema=SEI"
    );
    let page = fetch(&client, &format!("{}{location}", fake.url())).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.header(CONTENT_TYPE), "text/html; charset=iso-8859-1");
    assert!(
        page.body.contains(&0xE7),
        "no Latin-1 byte for ç in the page"
    );
    let (text, _, malformed) = encoding_rs::WINDOWS_1252.decode(&page.body);
    assert!(!malformed);
    assert!(text.contains("<title>SIP - Autenticação de Usuário</title>"));
    assert!(text.contains(concat!(
        r#"<select id="selOrgao" name="selOrgao" class="infraSelect">"#,
        r#"<option value="null">&nbsp;</option><option value="63">SEDEMO</option>"#
    )));
}

#[tokio::test]
async fn process_listing_follows_seitest() {
    let fake = FakeSei::start();
    let assignee = FakeUser {
        id: USER_ID,
        acronym: USERNAME.to_owned(),
        name: USER_NAME.to_owned(),
    };
    fake.set_processes(
        UNIT_NUPROC,
        vec![
            FakeProcess {
                description: "a <b> & c".to_owned(),
                assignee: Some(assignee),
                ..process(1)
            },
            FakeProcess {
                return_date: "06/11/2026".to_owned(),
                generating_unit: Some(FakeUnit {
                    id: 2000,
                    acronym: "SEDEMO/X".to_owned(),
                    description: String::new(),
                }),
                ..process(2)
            },
            FakeProcess {
                generated: true,
                return_overdue: true,
                ..process(3)
            },
        ],
    );
    let client = client();
    let token = token_of(&login(&client, &fake, PASSWORD).await.json());
    let in_unit = |unit: &'static str| [("token", token.as_str()), ("unidade", unit)];

    let answer = get(&client, &fake, "/processo/listar", &in_unit("")).await;
    let raw = String::from_utf8_lossy(&answer.body).into_owned();
    assert!(
        raw.contains(r#""descricao":"a \u003cb\u003e \u0026 c""#),
        "{raw}"
    );
    assert!(raw.ends_with("\"total\":\"3\"}\n"), "{raw}");
    let page = answer.json();
    let carried = json!({"dataProgramada": "06/11/2026", "unidade": "SEDEMO/GAB"});
    assert_eq!(column(&page, "/id"), ["9001", "9002", "9003"]);
    assert_eq!(
        column(&page, "/atributos/idProcedimento"),
        ["9001", "9002", "9003"]
    );
    assert_eq!(
        column(&page, "/atributos/status/retornoProgramado"),
        ["N", "S", "N"]
    );
    assert_eq!(
        column(&page, "/atributos/status/retornoData"),
        [Value::Null, carried.clone(), carried]
    );
    assert_eq!(
        column(&page, "/atributos/status/retornoAtrasado"),
        ["N", "N", "S"]
    );
    assert_eq!(
        column(&page, "/atributos/status/processoGeradoRecebido"),
        ["R", "R", "G"]
    );
    assert_eq!(
        column(&page, "/atributos/unidade"),
        [
            json!({"idUnidade": "1500", "sigla": "SEDEMO/PROTOCOLO"}),
            json!({"idUnidade": "2000", "sigla": "SEDEMO/X"}),
            json!({"idUnidade": "1001", "sigla": "SEDEMO/NUPROC"}),
        ]
    );
    assert_eq!(
        column(&page, "/atributos/usuarioAtribuido"),
        [
            json!({"idAtividade": "700", "idUsuario": "101", "sigla": USERNAME, "nome": USER_NAME,
                   "nomeformatado": "fulano.tal - Fulano de Tal"}),
            Value::Null,
            Value::Null,
        ]
    );

    let generated = get(&client, &fake, "/processo/listar?tipo=G", &in_unit(""))
        .await
        .json();
    assert_eq!(
        (column(&generated, "/id"), &generated["total"]),
        (vec![json!("9003")], &json!("1"))
    );
    let received = get(&client, &fake, "/processo/listar?tipo=R", &in_unit(""))
        .await
        .json();
    assert_eq!(column(&received, "/id"), ["9001", "9002"]);
    let defaults = get(
        &client,
        &fake,
        "/processo/listar?limit=0&start=x&limit=1",
        &in_unit(""),
    )
    .await
    .json();
    assert_eq!(column(&defaults, "/id"), ["9001", "9002", "9003"]);

    let past_the_end = get(
        &client,
        &fake,
        "/processo/listar?limit=2&start=2",
        &in_unit(""),
    )
    .await;
    assert_eq!(past_the_end.text(), "{\"sucesso\":true,\"total\":\"3\"}\n");
    let other_unit = get(&client, &fake, "/processo/listar", &in_unit("1002")).await;
    assert_eq!(other_unit.text(), "{\"sucesso\":true,\"total\":\"0\"}\n");
    let unknown_unit = get(&client, &fake, "/processo/listar", &in_unit("999")).await;
    assert_eq!(unknown_unit.status, StatusCode::OK);
    assert_eq!(
        unknown_unit.json(),
        json!({"sucesso": false, "mensagem": "Unidade não permitida para o usuário.",
               "exception": "Exceção salva no SEI"})
    );
}

#[tokio::test]
async fn activity_and_document_listings_follow_seitest() {
    let fake = FakeSei::start();
    fake.add_activities(
        9001,
        vec![
            activity(50001, "beltrano", utc(2026, 11, 1, 3, 30, 0)),
            activity(50002, "", utc(2026, 11, 2, 12, 0, 5)),
        ],
    );
    fake.add_activities(
        9001,
        vec![activity(50003, USERNAME, utc(2026, 11, 3, 18, 0, 0))],
    );
    fake.add_documents(9001, vec![document(80001), document(80002)]);
    fake.add_documents(9001, vec![document(80003)]);
    fake.remove_document(9001, 80002);
    let client = client();
    let token = token_of(&login(&client, &fake, PASSWORD).await.json());
    let token = [("token", token.as_str())];

    let activities = get(
        &client,
        &fake,
        "/atividade/listar?procedimento=9001",
        &token,
    )
    .await
    .json();
    assert_eq!(activities["total"], "3");
    assert_eq!(column(&activities, "/id"), ["50003", "50002", "50001"]);
    assert_eq!(
        column(&activities, "/atributos/idProcesso"),
        ["9001", "9001", "9001"]
    );
    assert_eq!(
        column(&activities, "/atributos/usuario"),
        [json!(USERNAME), Value::Null, json!("beltrano")]
    );
    assert_eq!(
        column(&activities, "/atributos/data"),
        ["03/11/2026", "02/11/2026", "31/10/2026"]
    );
    assert_eq!(
        column(&activities, "/atributos/hora"),
        ["14:00:00", "08:00:05", "23:30:00"]
    );
    let second_page = "/atividade/listar?procedimento=9001&limit=2&start=1";
    let second_page = get(&client, &fake, second_page, &token).await.json();
    assert_eq!(column(&second_page, "/id"), ["50001"]);
    let unknown = get(&client, &fake, "/atividade/listar?procedimento=7", &token).await;
    assert_eq!(unknown.text(), "{\"sucesso\":true,\"total\":\"0\"}\n");
    let missing = get(&client, &fake, "/atividade/listar?procedimento=x", &token).await;
    assert_eq!(
        missing.json()["mensagem"],
        "O procedimento deve ser informado!"
    );

    let documents = get(&client, &fake, "/documento/listar/9001", &token)
        .await
        .json();
    assert_eq!(documents["total"], "2");
    assert_eq!(column(&documents, "/id"), ["80001", "80003"]);
    assert_eq!(
        documents["data"][1]["atributos"],
        json!({
            "idProcedimento": "9001", "idProtocolo": "80003", "protocoloFormatado": "0080003",
            "nome": null, "titulo": "3", "tipo": "Ofício", "tipoDocumento": "I", "mimeType": "html",
            "nomeComposto": "Ofício 3 (0080003)", "status": {"documentoAssinado": "S"},
        })
    );
    let invalid = get(&client, &fake, "/documento/listar/abc", &token).await;
    assert_eq!(invalid.status, StatusCode::OK);
    assert_eq!(
        invalid.json()["mensagem"],
        "O procedimento deve ser informado!"
    );
}

#[tokio::test]
async fn logins_and_counters_follow_seitest() {
    let fake = FakeSei::start();
    let client = client();

    let refused = login(&client, &fake, "wrong").await;
    assert_eq!(refused.status, StatusCode::OK);
    assert_eq!(
        refused.text(),
        "{\"exception\":\"Exceção salva no SEI\",\"mensagem\":\"Usuário ou Senha Inválida.\",\"sucesso\":false}\n"
    );
    let first = token_of(&login(&client, &fake, PASSWORD).await.json());
    fake.set_password("nova-senha");
    let after_change = get(&client, &fake, "/versao", &[("token", &first)]).await;
    assert_eq!(after_change.status, StatusCode::FORBIDDEN);
    let old_password = login(&client, &fake, PASSWORD).await.json();
    assert_eq!(old_password["mensagem"], "Usuário ou Senha Inválida.");
    let second = token_of(&login(&client, &fake, "nova-senha").await.json());

    let plain_text = post_login(&client, &fake, "text/plain", "usuario=fulano.tal").await;
    assert_eq!(plain_text.json()["mensagem"], "Usuário ou Senha Inválida.");
    let bad_escape = post_login(
        &client,
        &fake,
        "application/x-www-form-urlencoded",
        "senha=%zz",
    )
    .await;
    assert_eq!(bad_escape.json()["mensagem"], "Requisição inválida.");
    let bad_media = post_login(&client, &fake, "application/x-www-form-urlencoded; a", "").await;
    assert_eq!(bad_media.json()["mensagem"], "Requisição inválida.");

    assert_eq!(fake.issued_tokens(), [first.clone(), second.clone()]);
    assert_ne!(first, second);
    assert!(
        [&first, &second]
            .iter()
            .all(|token| token.len() == 48 && token.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "tokens are not 24 random bytes in hex: {first} {second}"
    );
    assert_eq!((fake.logins(), fake.failed_logins()), (2, 3));
    assert_eq!(fake.calls(ROUTE_LOGIN), 7);
    assert_eq!(fake.calls(ROUTE_VERSION), 1);
    assert_eq!(fake.last_user_agent(), USER_AGENT);
}

#[tokio::test]
async fn armed_failures_follow_seitest() {
    let fake = FakeSei::start();
    let client = client();
    let token = token_of(&login(&client, &fake, PASSWORD).await.json());
    let token = [("token", token.as_str())];
    fake.fail_next(ROUTE_VERSION, 502, 2);
    fake.fail_next(ROUTE_VERSION, 403, 1);

    for _ in 0..2 {
        let failed = get(&client, &fake, "/versao", &token).await;
        assert_eq!(failed.status, StatusCode::BAD_GATEWAY);
        assert_eq!(failed.header(CONTENT_TYPE), "text/html; charset=iso-8859-1");
        assert_eq!(
            failed.text(),
            "<html><body><h1>Erro interno</h1></body></html>"
        );
    }
    let refused = get(&client, &fake, "/versao", &token).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.header(CONTENT_TYPE), "application/json");
    assert_eq!(
        refused.json(),
        json!({"sucesso": false, "mensagem": "Falha simulada pelo seitest.", "exception": "Exceção salva no SEI"})
    );
    let recovered = get(&client, &fake, "/versao", &token).await.json();
    assert_eq!(recovered["data"], json!({"sei": "5.0.5", "wssei": "3.0.4"}));

    fake.fail_next(ROUTE_ORGS, 500, 1);
    fake.set_module_inactive(true);
    for path in ["/orgao/listar", "/versao"] {
        let inactive = get(&client, &fake, path, &[]).await;
        assert_eq!(inactive.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            inactive.text(),
            "{\"exception\":null,\"mensagem\":\"Módulo inativo.\",\"sucesso\":false}\n"
        );
    }
    let page = fetch(&client, &format!("{}{ROUTE_LOGIN_PAGE}", fake.url())).await;
    assert_eq!(page.status, StatusCode::OK);
    fake.set_module_inactive(false);
    let queued = get(&client, &fake, "/orgao/listar", &[]).await;
    assert_eq!(queued.status, StatusCode::INTERNAL_SERVER_ERROR);
    let orgs = get(&client, &fake, "/orgao/listar", &[]).await.json();
    assert_eq!(orgs["total"], "12");
    assert_eq!(
        column(&orgs, "/sigla"),
        [
            "ADEMO", "BDEMO", "CDEMO", "DDEMO", "EDEMO", "FDEMO", "GDEMO", "HDEMO", "IDEMO",
            "JDEMO"
        ]
    );
    assert_eq!(fake.calls(ROUTE_ORGS), 3);
}

#[tokio::test]
async fn stalled_route_sends_headers_and_never_finishes() {
    let fake = FakeSei::start();
    let client = client();
    let token = token_of(&login(&client, &fake, PASSWORD).await.json());
    fake.stall(ROUTE_PROCESSES);

    let mut stalled = client
        .get(format!("{}/processo/listar", fake.api_url()))
        .header("token", &token)
        .send()
        .await
        .expect("the stalled route sends its headers");
    assert_eq!(stalled.status(), StatusCode::OK);
    assert_eq!(stalled.headers()[CONTENT_TYPE], "application/json");
    let first = stalled
        .chunk()
        .await
        .expect("a first chunk")
        .expect("a first chunk");
    assert_eq!(&first[..], br#"{"sucesso":true,"data":["#);
    let rest = tokio::time::timeout(Duration::from_millis(300), stalled.chunk()).await;
    assert!(rest.is_err(), "the stalled answer went on: {rest:?}");
    let version = get(&client, &fake, "/versao", &[("token", &token)]).await;
    assert_eq!(version.status, StatusCode::OK);
    assert_eq!(fake.calls(ROUTE_PROCESSES), 1);

    let dropping = Instant::now();
    drop(fake);
    assert!(dropping.elapsed() < Duration::from_secs(2));
    let after = tokio::time::timeout(Duration::from_secs(2), stalled.chunk())
        .await
        .expect("the connection closes with the fake");
    assert!(after.is_err(), "the stalled answer finished: {after:?}");
}

#[tokio::test]
async fn respond_next_answers_once_before_the_seitest_behavior() {
    let fake = FakeSei::start();
    let client = client();
    fake.respond_next(ROUTE_LOGIN, 302, &[("location", "/elsewhere")], Vec::new());
    let redirected = login(&client, &fake, PASSWORD).await;
    assert_eq!(redirected.status, StatusCode::FOUND);
    assert_eq!(redirected.header(LOCATION), "/elsewhere");
    assert!(redirected.body.is_empty());
    assert_eq!((fake.calls(ROUTE_LOGIN), fake.logins()), (1, 0));
    let token = token_of(&login(&client, &fake, PASSWORD).await.json());
    let token = [("token", token.as_str())];

    fake.fail_next(ROUTE_VERSION, 500, 1);
    let headers = [
        ("content-type", "application/json"),
        ("x-a", "1"),
        ("x-a", "2"),
    ];
    fake.respond_next(ROUTE_VERSION, 200, &headers, b"{\"sucesso\":true}".to_vec());
    let canned = get(&client, &fake, "/versao", &token).await;
    assert_eq!(canned.text(), "{\"sucesso\":true}");
    let values: Vec<_> = canned.headers.get_all("x-a").iter().collect();
    assert_eq!(values, ["1", "2"]);
    let failed = get(&client, &fake, "/versao", &token).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    let normal = get(&client, &fake, "/versao", &token).await.json();
    assert_eq!(normal["data"]["sei"], "5.0.5");

    let latin1 = [("content-type", "text/html; charset=iso-8859-1")];
    fake.respond_next(ROUTE_LOGIN_PAGE, 200, &latin1, fixture("login.html"));
    let page_url = format!("{}{ROUTE_LOGIN_PAGE}", fake.url());
    assert_eq!(fetch(&client, &page_url).await.body, fixture("login.html"));
    assert_ne!(fetch(&client, &page_url).await.body, fixture("login.html"));
    assert_eq!(fake.calls(ROUTE_LOGIN_PAGE), 2);

    fake.stall(ROUTE_ORGS);
    fake.respond_next(ROUTE_ORGS, 418, &[], b"teapot".to_vec());
    let teapot = get(&client, &fake, "/orgao/listar", &[]).await;
    assert_eq!(
        (teapot.status, teapot.text().as_str()),
        (StatusCode::IM_A_TEAPOT, "teapot")
    );
    let mut stalled = client
        .get(format!("{}/orgao/listar", fake.api_url()))
        .send()
        .await
        .unwrap();
    stalled.chunk().await.expect("a first chunk");
    let rest = tokio::time::timeout(Duration::from_millis(300), stalled.chunk()).await;
    assert!(rest.is_err(), "the stalled answer went on: {rest:?}");
}

#[tokio::test]
async fn unknown_paths_and_methods_answer_like_go_serve_mux() {
    let fake = FakeSei::start();
    let client = client();
    let url = fake.url();

    let missing = fetch(&client, &format!("{url}/nope")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.header(CONTENT_TYPE), "text/plain; charset=utf-8");
    assert_eq!(missing.header("x-content-type-options"), "nosniff");
    assert_eq!(missing.text(), "404 page not found\n");
    let no_id = fetch(&client, &format!("{}/documento/listar/", fake.api_url())).await;
    assert_eq!(no_id.status, StatusCode::NOT_FOUND);

    let get_login = fetch(&client, &format!("{}/autenticar", fake.api_url())).await;
    assert_eq!(get_login.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        (get_login.header(ALLOW), get_login.text()),
        ("POST".to_owned(), "Method Not Allowed\n".to_owned())
    );
    let post_sei = Answer::read(client.post(format!("{url}/sei/")).send().await.unwrap()).await;
    assert_eq!(post_sei.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(post_sei.header(ALLOW), "GET, HEAD");

    let redirect = fetch(&client, &format!("{url}/sei/")).await;
    assert_eq!(redirect.header(CONTENT_TYPE), "text/html; charset=utf-8");
    assert_eq!(
        redirect.text(),
        "<a href=\"/sip/login.php?sigla_orgao_sistema=SEDEMO&amp;sigla_sistema=SEI\">Found</a>.\n\n"
    );
    let no_slash = fetch(&client, &format!("{url}/sei?x=1")).await;
    assert_eq!(no_slash.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(no_slash.header(LOCATION), "/sei/?x=1");
    assert_eq!(
        no_slash.text(),
        "<a href=\"/sei/?x=1\">Temporary Redirect</a>.\n\n"
    );
    let unclean = fetch(&client, &format!("{url}/sip//login.php")).await;
    assert_eq!(unclean.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(unclean.header(LOCATION), ROUTE_LOGIN_PAGE);
    assert_eq!(fake.calls(ROUTE_LOGIN_PAGE), 0);
}

#[test]
fn every_recorded_setup_call_applies() {
    for (name, text) in [("cycle.json", CYCLE), ("commands.json", COMMANDS)] {
        let vector: Value = serde_json::from_str(text).unwrap();
        let mut setups = Vec::new();
        setup_lists(&vector, &mut setups);
        for calls in &setups {
            FakeSei::start()
                .apply(calls)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        let applied: usize = setups.iter().map(|calls| calls.len()).sum();
        assert!(applied > 0, "{name}: no setup call found");
        assert_eq!(
            applied,
            count_calls(&vector),
            "{name}: calls outside a setup list"
        );
    }
}

#[tokio::test]
async fn setup_calls_reject_unknown_names_and_fields() {
    let fake = FakeSei::start();
    let refused = [
        json!({"call": "client_login", "args": {}}),
        json!({"call": "fail_next", "args": {"route": "/versao", "status": 500, "times": 1, "delay": 3}}),
        json!({"call": "set_module_inactive", "args": {"inactive": true}, "when": "later"}),
        json!({"call": "set_processes", "args": {"unit_id": 1001, "processes": [{"id": 1, "generating_unit": null}]}}),
        json!({"call": "add_activities", "args": {"procedure_id": 1, "activities": [{"id": 1, "at": "yesterday"}]}}),
    ];
    for call in refused {
        let applied = fake.apply(&[
            json!({"call": "set_module_inactive", "args": {"inactive": true}}),
            call.clone(),
        ]);
        assert!(applied.is_err(), "accepted {call}");
    }
    let orgs = get(&client(), &fake, "/orgao/listar", &[]).await;
    assert_eq!(orgs.status, StatusCode::OK);
}

#[tokio::test]
async fn setup_calls_change_the_fake_like_seisetup() {
    let fake = FakeSei::start();
    let calls = [
        json!({"call": "set_processes", "args": {"unit_id": 1001, "processes": [
            {"id": 9001, "number": "00000.000001/2026-00", "type": "Ofício", "description": "Compra",
             "assignee": {"id": 202, "acronym": "beltrano.silva", "name": "Beltrano da Silva"},
             "return_date": "06/11/2026", "return_overdue": true, "generated": true}]}}),
        json!({"call": "add_activities", "args": {"procedure_id": 9001, "activities": [
            {"id": 50001, "user": "beltrano.silva", "unit": "SEDEMO/PROTOCOLO", "text": "Processo gerado",
             "at": "2026-11-01T18:00:00.5Z"}]}}),
        json!({"call": "add_documents", "args": {"procedure_id": 9001, "documents": [
            {"id": 80001, "protocol": "0080001", "type": "Ofício", "number": "1", "label": "Ofício 1 (0080001)"}]}}),
        json!({"call": "fail_next", "args": {"route": "/processo/listar", "status": 502, "times": 2}}),
        json!({"call": "set_module_inactive", "args": {"inactive": false}}),
    ];
    fake.apply(&calls).expect("apply the setup calls");
    let client = client();
    let token = token_of(&login(&client, &fake, PASSWORD).await.json());
    let token = [("token", token.as_str())];
    for _ in 0..2 {
        let failed = get(&client, &fake, "/processo/listar", &token).await;
        assert_eq!(failed.status, StatusCode::BAD_GATEWAY);
    }
    let processes = get(&client, &fake, "/processo/listar", &token).await.json();
    let process = &processes["data"][0]["atributos"];
    assert_eq!(
        [
            &process["numero"],
            &process["tipoProcesso"],
            &process["descricao"]
        ],
        ["00000.000001/2026-00", "Ofício", "Compra"]
    );
    assert_eq!(
        process["usuarioAtribuido"]["nomeformatado"],
        "beltrano.silva - Beltrano da Silva"
    );
    assert_eq!(
        process["status"]["retornoData"]["dataProgramada"],
        "06/11/2026"
    );
    assert_eq!(
        [
            &process["status"]["retornoAtrasado"],
            &process["status"]["processoGeradoRecebido"]
        ],
        ["S", "G"]
    );
    let activities = "/atividade/listar?procedimento=9001";
    let activities = get(&client, &fake, activities, &token).await.json();
    let activity = &activities["data"][0]["atributos"];
    assert_eq!(
        [
            &activity["usuario"],
            &activity["unidade"],
            &activity["informacao"],
            &activity["data"],
            &activity["hora"]
        ],
        [
            "beltrano.silva",
            "SEDEMO/PROTOCOLO",
            "Processo gerado",
            "01/11/2026",
            "14:00:00"
        ]
    );
    let documents = get(&client, &fake, "/documento/listar/9001", &token)
        .await
        .json();
    let document = &documents["data"][0]["atributos"];
    assert_eq!(
        [
            &document["protocoloFormatado"],
            &document["tipo"],
            &document["titulo"],
            &document["nomeComposto"]
        ],
        ["0080001", "Ofício", "1", "Ofício 1 (0080001)"]
    );
}

#[tokio::test]
async fn fresh_fake_matches_the_recorded_defaults() {
    for (name, text) in [("cycle.json", CYCLE), ("commands.json", COMMANDS)] {
        let recorded = serde_json::from_str::<Value>(text).unwrap()["defaults"].clone();
        let fake = FakeSei::start();
        assert_eq!(observed_defaults(&fake).await, recorded, "{name}");
    }
}

async fn observed_defaults(fake: &FakeSei) -> Value {
    let client = client();
    let session = login(&client, fake, PASSWORD).await.json()["data"].clone();
    let token = [("token", session["token"].as_str().expect("a session token"))];
    let versions = get(&client, fake, "/versao", &token).await.json()["data"].clone();
    let org_page = get(&client, fake, "/orgao/listar", &[]).await.json();
    let number = |value: &Value| value.as_str().and_then(|text| text.parse::<i64>().ok());
    let units: Vec<Value> = session["unidades"]
        .as_array()
        .expect("the session units")
        .iter()
        .map(|unit| {
            json!({"id": number(&unit["id"]), "acronym": unit["sigla"], "description": unit["descricao"]})
        })
        .collect();
    let orgs: Vec<Value> = default_orgs()
        .into_iter()
        .map(|(id, acronym, description)| {
            json!({"id": id, "acronym": acronym, "description": description})
        })
        .collect();
    let user = &session["loginData"];
    json!({
        "account": {
            "username": USERNAME,
            "password": PASSWORD,
            "org_id": ORG_ID,
            "user": {"id": number(&user["IdUsuario"]), "acronym": user["sigla"], "name": user["nome"]},
            "units": units,
        },
        "current_unit_id": number(&user["IdUnidadeAtual"]),
        "orgs": orgs,
        "org_list_total": number(&org_page["total"]),
        "org_list_page_size": org_page["data"].as_array().map(Vec::len),
        "versions": {"sei": versions["sei"], "module": versions["wssei"]},
        "time_zone": fake.zone().name(),
        "api_path": fake.api_url().strip_prefix(&fake.url()),
    })
}

fn setup_lists<'a>(value: &'a Value, lists: &mut Vec<&'a [Value]>) {
    match value {
        Value::Object(fields) => fields.iter().for_each(|(key, item)| match item {
            Value::Array(calls) if key == "setup" => lists.push(calls),
            _ => setup_lists(item, lists),
        }),
        Value::Array(items) => items.iter().for_each(|item| setup_lists(item, lists)),
        _ => {}
    }
}

fn count_calls(value: &Value) -> usize {
    match value {
        Value::Object(fields) => {
            let own = usize::from(fields.contains_key("call"));
            own + fields.values().map(count_calls).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(count_calls).sum(),
        _ => 0,
    }
}

fn fields_missing_from(fake: &FieldTypes, real: &FieldTypes) -> Vec<String> {
    let empty = BTreeSet::new();
    fake.iter()
        .flat_map(|(field, kinds)| {
            let real_kinds = real.get(field).unwrap_or(&empty);
            let only_null = real_kinds.len() == 1 && real_kinds.contains("null");
            kinds
                .iter()
                .filter(move |kind| !real_kinds.contains(*kind) && **kind != "null" && !only_null)
                .map(move |kind| format!("{field} ({kind}), fixture has {real_kinds:?}"))
        })
        .collect()
}

fn field_types(value: &Value) -> FieldTypes {
    let mut types = FieldTypes::new();
    collect(String::new(), value, &mut types);
    types
}

fn collect(prefix: String, value: &Value, types: &mut FieldTypes) {
    types.entry(prefix.clone()).or_default().insert(kind(value));
    match value {
        Value::Object(fields) => fields
            .iter()
            .for_each(|(key, item)| collect(format!("{prefix}.{key}"), item, types)),
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect(format!("{prefix}[]"), item, types)),
        _ => {}
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "object",
        Value::Array(_) => "list",
        Value::String(_) => "string",
        Value::Null => "null",
        Value::Bool(_) | Value::Number(_) => "number or boolean",
    }
}

fn column(page: &Value, pointer: &str) -> Vec<Value> {
    page["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.pointer(pointer).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .unwrap_or_default()
}

fn process(n: u32) -> FakeProcess {
    FakeProcess {
        id: 9000 + i64::from(n),
        number: process_number(n),
        process_type: "Contrato".to_owned(),
        ..FakeProcess::default()
    }
}

fn activity(id: i64, user: &str, at: DateTime<Utc>) -> FakeActivity {
    FakeActivity {
        id,
        user: user.to_owned(),
        unit: "SEDEMO/NUPROC".to_owned(),
        text: "Processo recebido na unidade".to_owned(),
        at,
    }
}

fn document(id: i64) -> FakeDocument {
    let number = id - 80000;
    let protocol = format!("{id:07}");
    FakeDocument {
        id,
        label: format!("Ofício {number} ({protocol})"),
        protocol,
        document_type: "Ofício".to_owned(),
        number: number.to_string(),
    }
}

fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .expect("a valid UTC time")
}

struct Answer {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    async fn read(response: Response) -> Answer {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .expect("read the fake's answer")
            .to_vec();
        Answer {
            status,
            headers,
            body,
        }
    }

    fn header(&self, name: impl reqwest::header::AsHeaderName) -> String {
        self.headers
            .get(name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the fake answered HTTP {} with something that is not JSON ({error}): {}",
                self.status,
                self.text()
            )
        })
    }
}

fn client() -> Client {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .build()
        .expect("build the test HTTP client")
}

fn token_of(login: &Value) -> String {
    login["data"]["token"]
        .as_str()
        .expect("a session token")
        .to_owned()
}

async fn login(client: &Client, fake: &FakeSei, password: &str) -> Answer {
    let form = format!("usuario={USERNAME}&senha={password}&orgao={ORG_ID}&contexto=");
    post_login(client, fake, "application/x-www-form-urlencoded", &form).await
}

async fn post_login(client: &Client, fake: &FakeSei, content_type: &str, body: &str) -> Answer {
    let response = client
        .post(format!("{}/autenticar", fake.api_url()))
        .header(CONTENT_TYPE, content_type)
        .body(body.to_owned())
        .send()
        .await
        .expect("log in to the fake");
    Answer::read(response).await
}

async fn get(client: &Client, fake: &FakeSei, path: &str, headers: &[(&str, &str)]) -> Answer {
    let request = headers.iter().filter(|(_, value)| !value.is_empty()).fold(
        client.get(format!("{}{path}", fake.api_url())),
        |request, (name, value)| request.header(*name, *value),
    );
    Answer::read(request.send().await.expect("call the fake")).await
}

async fn fetch(client: &Client, url: &str) -> Answer {
    Answer::read(client.get(url).send().await.expect("call the fake")).await
}
