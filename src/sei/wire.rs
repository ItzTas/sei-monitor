mod activity;
mod document;
mod layout;
mod process;
#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use chrono_tz::Tz;
use serde_json::{Map, Value};

use super::errors::{ApiError, ErrorKind, SeiError, classify};
use super::html::unescape;
use super::json::{field, flex_int, go_lossy_utf8, is_empty_json, kind_of, parse, string_field};
use super::types::{Activity, Document, Page, Process, Session, Unit, Version};
use crate::gotext::{capped, equal_fold, trim};
use crate::secret::Secret;

pub(crate) const ROUTE_LOGIN: &str = "/autenticar";
pub(crate) const ROUTE_VERSION: &str = "/versao";
pub(crate) const ROUTE_PROCESSES: &str = "/processo/listar";
pub(crate) const ROUTE_ACTIVITIES: &str = "/atividade/listar";
pub(crate) const ROUTE_DOCUMENTS: &str = "/documento/listar";

const REDIRECT_MESSAGE: &str = "SEI answered with a redirect, which sei-monitor does not follow on API calls; check SEI_URL and SEI_API_URL";

static EMPTY_OBJECT: LazyLock<Value> = LazyLock::new(|| Value::Object(Map::new()));

pub(crate) struct Envelope {
    pub data: Option<Value>,
    pub total: Option<Value>,
    message: Option<Value>,
}

impl Envelope {
    fn message(&self) -> String {
        match &self.message {
            Some(Value::String(text)) => unescape(trim(text)),
            Some(Value::Array(items)) => list_message(items).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

fn list_message(items: &[Value]) -> Option<String> {
    let parts = items
        .iter()
        .map(|item| match item {
            Value::Null => Some(String::new()),
            Value::String(text) => Some(unescape(trim(text))),
            _ => None,
        })
        .collect::<Option<Vec<String>>>()?;
    Some(parts.join("; "))
}

pub(crate) fn read_envelope(route: &str, status: u16, body: &[u8]) -> Result<Envelope, SeiError> {
    if (300..400).contains(&status) {
        return Err(api_error(
            route,
            status,
            REDIRECT_MESSAGE.to_owned(),
            ErrorKind::Redirect,
        ));
    }
    let Some((success, envelope)) = decode(body) else {
        return Err(unexpected(route, status));
    };
    if success && status < 300 {
        return Ok(envelope);
    }
    let message = envelope.message();
    let kind = classify(route, status, &message);
    Err(api_error(route, status, capped(&message).to_owned(), kind))
}

fn decode(body: &[u8]) -> Option<(bool, Envelope)> {
    let Ok(Value::Object(mut members)) = parse(&go_lossy_utf8(body)) else {
        return None;
    };
    let success = take(&mut members, "sucesso")?.as_bool()?;
    let envelope = Envelope {
        data: take(&mut members, "data"),
        total: take(&mut members, "total"),
        message: take(&mut members, "mensagem"),
    };
    Some((success, envelope))
}

fn take(members: &mut Map<String, Value>, name: &str) -> Option<Value> {
    if let Some(value) = members.remove(name) {
        return Some(value);
    }
    let key = members.keys().find(|key| equal_fold(key, name))?.clone();
    members.remove(&key)
}

fn api_error(route: &str, status: u16, message: String, kind: ErrorKind) -> SeiError {
    SeiError::Api(ApiError {
        route: route.to_owned(),
        status,
        message,
        kind,
    })
}

fn unexpected(route: &str, status: u16) -> SeiError {
    let message = format!("answer outside the API format (HTTP {status})");
    if status >= 500 {
        return api_error(route, status, message, ErrorKind::Server);
    }
    api_error(route, status, message, ErrorKind::UnexpectedResponse)
}

fn decode_error(route: &str, detail: String) -> SeiError {
    SeiError::Decode {
        route: route.to_owned(),
        detail,
        timed_out: false,
    }
}

pub(crate) fn process_page(route: &str, envelope: &Envelope) -> Result<Page<Process>, SeiError> {
    page(route, envelope, process::process)
}

pub(crate) fn activity_page(
    route: &str,
    envelope: &Envelope,
    procedure_id: i64,
    zone: Tz,
) -> Result<Page<Activity>, SeiError> {
    page(route, envelope, |item| {
        activity::activity(item, procedure_id, zone)
    })
}

pub(crate) fn document_page(
    route: &str,
    envelope: &Envelope,
    procedure_id: i64,
) -> Result<Page<Document>, SeiError> {
    page(route, envelope, |item| {
        document::document(item, procedure_id)
    })
}

fn page<T>(
    route: &str,
    envelope: &Envelope,
    convert: impl Fn(&Value) -> Result<T, String>,
) -> Result<Page<T>, SeiError> {
    let items =
        list(envelope.data.as_ref(), convert).map_err(|detail| decode_error(route, detail))?;
    Ok(Page {
        items,
        total: flex_int(envelope.total.as_ref()).unwrap_or(0),
    })
}

fn list<T>(
    data: Option<&Value>,
    convert: impl Fn(&Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    if is_empty_json(data) {
        return Ok(Vec::new());
    }
    let Some(Value::Array(items)) = data else {
        return Err(format!(
            "data: expected a list, got {}",
            data.map_or("nothing", kind_of)
        ));
    };
    items.iter().map(convert).collect()
}

pub(crate) fn login_session(envelope: &Envelope) -> Result<(Secret, Session), SeiError> {
    let (token, session) =
        login(envelope.data.as_ref()).map_err(|detail| decode_error(ROUTE_LOGIN, detail))?;
    if token.is_empty() {
        return Err(SeiError::NoToken);
    }
    Ok((Secret::new(token), session))
}

fn login(data: Option<&Value>) -> Result<(String, Session), String> {
    let data = present(data)?;
    let token = string_field(data, "token")?;
    let login = nested(data, "loginData")?;
    let session = Session {
        user_id: whole(login, "IdUsuario")?,
        user_acronym: capped(&string_field(login, "sigla")?).to_owned(),
        user_name: capped(&unescape(&string_field(login, "nome")?)).to_owned(),
        current_unit_id: whole(login, "IdUnidadeAtual")?,
        units: units(field(data, "unidades"))?,
    };
    Ok((token, session))
}

fn units(value: Option<&Value>) -> Result<Vec<Unit>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().map(unit).collect(),
        Some(other) => Err(format!("unidades: expected a list, got {}", kind_of(other))),
    }
}

fn unit(item: &Value) -> Result<Unit, String> {
    let item = object(Some(item))?;
    Ok(Unit {
        id: whole(item, "id")?,
        acronym: capped(&string_field(item, "sigla")?).to_owned(),
        description: capped(&unescape(&string_field(item, "descricao")?)).to_owned(),
    })
}

pub(crate) fn version(envelope: &Envelope) -> Result<Version, SeiError> {
    read_version(envelope.data.as_ref()).map_err(|detail| decode_error(ROUTE_VERSION, detail))
}

fn read_version(data: Option<&Value>) -> Result<Version, String> {
    let data = present(data)?;
    Ok(Version {
        sei: capped(&string_field(data, "sei")?).to_owned(),
        module: capped(&string_field(data, "wssei")?).to_owned(),
    })
}

fn present(data: Option<&Value>) -> Result<&Value, String> {
    let data = data.ok_or_else(|| "the answer has no data".to_owned())?;
    object(Some(data)).map_err(|error| format!("data: {error}"))
}

fn object(value: Option<&Value>) -> Result<&Value, String> {
    match value {
        None | Some(Value::Null) => Ok(&EMPTY_OBJECT),
        Some(object @ Value::Object(_)) => Ok(object),
        Some(other) => Err(format!("expected an object, got {}", kind_of(other))),
    }
}

fn nested<'a>(object_value: &'a Value, name: &str) -> Result<&'a Value, String> {
    object(field(object_value, name)).map_err(|error| format!("{name}: {error}"))
}

fn whole(object_value: &Value, name: &str) -> Result<i64, String> {
    flex_int(field(object_value, name)).map_err(|error| format!("{name}: {error}"))
}

fn or_fallback(id: i64, fallback: i64) -> i64 {
    if id == 0 {
        return fallback;
    }
    id
}
