use base64::prelude::{BASE64_STANDARD, Engine as _};
use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::golden;
use crate::sei::errors::tests::kind_named;
use crate::sei::types::User;
use crate::timezone::load_zone;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCase {
    name: String,
    route: String,
    zone: String,
    status: u16,
    body_base64: String,
    result: Option<Value>,
    error: Option<WireError>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    kind: String,
    message: String,
}

type Call = fn(&Envelope, Tz) -> Result<Value, SeiError>;

fn call(route: &str) -> (&'static str, Call) {
    let calls: [(&str, &'static str, Call); 5] = [
        ("processes", ROUTE_PROCESSES, |envelope, _| {
            process_page(ROUTE_PROCESSES, envelope).map(|page| page_json(page, process_json))
        }),
        ("activities", ROUTE_ACTIVITIES, |envelope, zone| {
            activity_page(ROUTE_ACTIVITIES, envelope, 9001, zone)
                .map(|page| page_json(page, activity_json))
        }),
        ("documents", ROUTE_DOCUMENTS, |envelope, _| {
            document_page(ROUTE_DOCUMENTS, envelope, 9001)
                .map(|page| page_json(page, document_json))
        }),
        ("version", ROUTE_VERSION, |envelope, _| {
            version(envelope).map(|version| json!({"sei": version.sei, "module": version.module}))
        }),
        ("login", ROUTE_LOGIN, |envelope, _| {
            login_session(envelope).map(|(_, session)| session_json(session))
        }),
    ];
    calls
        .iter()
        .find(|(name, _, _)| *name == route)
        .map(|(_, path, call)| (*path, *call))
        .unwrap_or_else(|| panic!("unknown route {route:?} in wire.json"))
}

fn page_json<T>(page: Page<T>, item: fn(T) -> Value) -> Value {
    let items: Vec<Value> = page.items.into_iter().map(item).collect();
    json!({"items": items, "total": page.total})
}

fn user_json(user: User) -> Value {
    json!({"id": user.id, "acronym": user.acronym, "name": user.name})
}

fn process_json(process: Process) -> Value {
    json!({
        "procedure_id": process.procedure_id,
        "number": process.number,
        "type": process.process_type,
        "description": process.description,
        "assignee": process.assignee.map(user_json),
        "return_date": process
            .return_date
            .map(go_date)
            .unwrap_or_default(),
        "return_overdue": process.return_overdue,
        "origin": process.origin,
    })
}

fn go_date(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn go_time(at: Option<DateTime<Utc>>) -> String {
    let Some(at) = at else {
        return "0001-01-01T00:00:00Z".to_owned();
    };
    let fraction = format!(".{:09}", at.nanosecond());
    let fraction = fraction.trim_end_matches('0').trim_end_matches('.');
    format!(
        "{}T{:02}:{:02}:{:02}{fraction}Z",
        go_date(at.date_naive()),
        at.hour(),
        at.minute(),
        at.second()
    )
}

fn activity_json(activity: Activity) -> Value {
    json!({
        "id": activity.id,
        "procedure_id": activity.procedure_id,
        "user": activity.user,
        "unit": activity.unit,
        "text": activity.text,
        "at": go_time(activity.at),
    })
}

fn document_json(document: Document) -> Value {
    json!({
        "id": document.id,
        "procedure_id": document.procedure_id,
        "protocol": document.protocol,
        "type": document.document_type,
        "name": document.name,
    })
}

fn session_json(session: Session) -> Value {
    let units: Vec<Value> = session
        .units
        .into_iter()
        .map(
            |unit| json!({"id": unit.id, "acronym": unit.acronym, "description": unit.description}),
        )
        .collect();
    json!({
        "user_id": session.user_id,
        "user_acronym": session.user_acronym,
        "user_name": session.user_name,
        "current_unit_id": session.current_unit_id,
        "units": units,
    })
}

fn answer(case: &WireCase) -> Result<Value, SeiError> {
    let zone = load_zone(&case.zone).unwrap_or_else(|| panic!("zone {} is missing", case.zone));
    let body = BASE64_STANDARD
        .decode(&case.body_base64)
        .unwrap_or_else(|err| panic!("{}: body_base64: {err}", case.name));
    let (path, call) = call(&case.route);
    let envelope = read_envelope(path, case.status, &body)?;
    call(&envelope, zone)
}

fn check_error(case: &WireCase, error: &SeiError) {
    let Some(want) = &case.error else {
        panic!(
            "{}: Go answered {:?}, Rust failed with {error}",
            case.name, case.result
        );
    };
    let text = error.to_string();
    match want.kind.as_str() {
        "decode" => {
            assert!(
                matches!(error, SeiError::Decode { .. }),
                "{}: {error:?}",
                case.name
            );
            assert!(text.starts_with(&want.message), "{}: {text}", case.name);
        }
        "no_token" => {
            assert!(
                matches!(error, SeiError::NoToken),
                "{}: {error:?}",
                case.name
            );
            assert_eq!(text, want.message, "{}", case.name);
        }
        kind => {
            assert_eq!(
                error.kind(),
                Some(kind_named(kind)),
                "{}: {error:?}",
                case.name
            );
            assert_eq!(text, want.message, "{}", case.name);
        }
    }
}

#[test]
fn wire_matches_go() {
    for case in golden::cases::<WireCase>("wire", "") {
        match answer(&case) {
            Ok(result) => {
                assert!(
                    case.error.is_none(),
                    "{}: Go failed, Rust answered {result}",
                    case.name
                );
                assert_eq!(Some(&result), case.result.as_ref(), "{}", case.name);
            }
            Err(error) => check_error(&case, &error),
        }
    }
}

#[test]
fn fixtures_read_like_go() {
    let body = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/sei/processo_listar.json"
    ));
    let envelope = read_envelope(ROUTE_PROCESSES, 200, body).unwrap();
    let page = process_page(ROUTE_PROCESSES, &envelope).unwrap();

    let go = golden::cases::<WireCase>("wire", "")
        .into_iter()
        .find(|case| case.name == "fixture processo_listar.json on processes with HTTP 200")
        .expect("the processo_listar.json case in wire.json");
    let items = go
        .result
        .as_ref()
        .and_then(|result| result["items"].as_array())
        .unwrap();
    assert_eq!(page.items.len(), items.len());
    assert_eq!(
        Some(page.items[0].procedure_id),
        items[0]["procedure_id"].as_i64()
    );
}

fn refusal(result: Result<Envelope, SeiError>) -> SeiError {
    match result {
        Ok(_) => panic!("the answer was accepted"),
        Err(error) => error,
    }
}

#[test]
fn messages_are_trimmed_then_unescaped() {
    let error = refusal(read_envelope(
        ROUTE_PROCESSES,
        200,
        br#"{"sucesso":false,"mensagem":"  A &amp; B  "}"#,
    ));

    assert_eq!(error.to_string(), "sei: /processo/listar: A & B");
}

#[test]
fn login_never_shows_the_token() {
    let token = "tok-3f9a77";
    let accepted = format!(
        r#"{{"sucesso":true,"data":{{"token":"{token}","loginData":{{"IdUsuario":"101"}}}}}}"#
    );
    let envelope = read_envelope(ROUTE_LOGIN, 200, accepted.as_bytes()).unwrap();
    let (secret, session) = login_session(&envelope).unwrap();
    assert_eq!(secret.reveal(), token);
    assert!(!format!("{secret:?} {secret} {session:?}").contains(token));

    let rejected = format!(
        r#"{{"sucesso":true,"data":{{"token":"{token}","loginData":{{"IdUsuario":"{token}"}},"unidades":"{token}"}}}}"#
    );
    let envelope = read_envelope(ROUTE_LOGIN, 200, rejected.as_bytes()).unwrap();
    let error = login_session(&envelope).unwrap_err();
    assert!(matches!(error, SeiError::Decode { .. }), "{error:?}");
    for text in [error.to_string(), format!("{error:?}")] {
        assert!(!text.contains(token), "{text}");
    }
}
