use chrono_tz::Tz;
use serde_json::Value;

use super::{layout, nested, object, or_fallback, whole};
use crate::gotext::{capped, trim};
use crate::sei::html::unescape;
use crate::sei::json::string_field;
use crate::sei::types::Activity;
use crate::timezone::local_to_utc;

pub(super) fn activity(item: &Value, procedure_id: i64, zone: Tz) -> Result<Activity, String> {
    let item = object(Some(item))?;
    let id = whole(item, "id")?;
    let attributes = nested(item, "atributos")?;
    let own_procedure_id = whole(attributes, "idProcesso")?;
    let user = string_field(attributes, "usuario")?;
    let date = string_field(attributes, "data")?;
    let time = string_field(attributes, "hora")?;
    let unit = string_field(attributes, "unidade")?;
    let text = string_field(attributes, "informacao")?;
    let local = layout::parse_date_time(&format!("{} {}", trim(&date), trim(&time)));
    Ok(Activity {
        id,
        procedure_id: or_fallback(own_procedure_id, procedure_id),
        user: capped(trim(&user)).to_owned(),
        unit: capped(trim(&unit)).to_owned(),
        text: capped(&unescape(trim(&text))).to_owned(),
        at: local.map(|local| local_to_utc(local, zone)),
    })
}
