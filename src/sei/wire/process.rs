use chrono::NaiveDate;
use serde_json::Value;

use super::{layout, nested, object, or_fallback, whole};
use crate::gotext::{capped, trim};
use crate::sei::html::unescape;
use crate::sei::json::{field, flex_int, string_field};
use crate::sei::types::{Process, User};

pub(super) fn process(item: &Value) -> Result<Process, String> {
    let item = object(Some(item))?;
    let id = whole(item, "id")?;
    let attributes = nested(item, "atributos")?;
    let procedure_id = whole(attributes, "idProcedimento")?;
    let number = string_field(attributes, "numero")?;
    let process_type = string_field(attributes, "tipoProcesso")?;
    let description = string_field(attributes, "descricao")?;
    let status = nested(attributes, "status")?;
    let scheduled = string_field(status, "retornoProgramado")?;
    let overdue = string_field(status, "retornoAtrasado")?;
    let origin = string_field(status, "processoGeradoRecebido")?;
    Ok(Process {
        procedure_id: or_fallback(procedure_id, id),
        number: capped(trim(&number)),
        process_type: capped(&unescape(trim(&process_type))),
        description: capped(&unescape(trim(&description))),
        assignee: assignee(field(attributes, "usuarioAtribuido")),
        return_date: (scheduled == "S")
            .then(|| return_date(field(status, "retornoData")))
            .flatten(),
        return_overdue: overdue == "S",
        origin,
    })
}

fn assignee(raw: Option<&Value>) -> Option<User> {
    let raw = raw.filter(|raw| raw.is_object())?;
    let id = flex_int(field(raw, "idUsuario")).ok()?;
    let acronym = string_field(raw, "sigla").ok()?;
    let name = string_field(raw, "nome").ok()?;
    let acronym = trim(&acronym);
    if id == 0 && acronym.is_empty() {
        return None;
    }
    Some(User {
        id,
        acronym: capped(acronym),
        name: capped(&unescape(trim(&name))),
    })
}

fn return_date(raw: Option<&Value>) -> Option<NaiveDate> {
    if let Some(scheduled) = scheduled_date(raw) {
        return layout::parse_date(trim(&scheduled));
    }
    let Some(Value::String(text)) = raw else {
        return None;
    };
    layout::parse_date(trim(text))
}

fn scheduled_date(raw: Option<&Value>) -> Option<String> {
    raw.filter(|raw| raw.is_object())
        .and_then(|raw| string_field(raw, "dataProgramada").ok())
        .filter(|date| !date.is_empty())
}
