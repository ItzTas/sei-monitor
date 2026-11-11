use serde_json::Value;

use super::{nested, object, or_fallback, whole};
use crate::gotext::{capped, trim};
use crate::sei::html::unescape;
use crate::sei::json::string_field;
use crate::sei::types::Document;

pub(super) fn document(item: &Value, procedure_id: i64) -> Result<Document, String> {
    let item = object(Some(item))?;
    let id = whole(item, "id")?;
    let attributes = nested(item, "atributos")?;
    let own_procedure_id = whole(attributes, "idProcedimento")?;
    let protocol = string_field(attributes, "protocoloFormatado")?;
    let number = string_field(attributes, "titulo")?;
    let document_type = string_field(attributes, "tipo")?;
    let file_name = string_field(attributes, "nome")?;
    let label = string_field(attributes, "nomeComposto")?;
    Ok(Document {
        id,
        procedure_id: or_fallback(own_procedure_id, procedure_id),
        protocol: capped(trim(&protocol)).to_owned(),
        name: capped(&name(&label, &document_type, &number, &file_name)).to_owned(),
        document_type: capped(&unescape(trim(&document_type))).to_owned(),
    })
}

fn name(label: &str, document_type: &str, number: &str, file_name: &str) -> String {
    let label = trim(label);
    if !label.is_empty() {
        return unescape(label);
    }
    let composed = format!("{} {}", trim(document_type), trim(number));
    let composed = trim(&composed);
    if !composed.is_empty() {
        return unescape(composed);
    }
    unescape(trim(file_name))
}
