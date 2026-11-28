mod float;

use std::collections::HashMap;
use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::gotext::{equal_fold, trim};

const KELVIN: char = '\u{212A}';
const LONG_S: char = '\u{17F}';

pub(crate) fn go_lossy_utf8(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                return text;
            }
            Err(error) => {
                let (valid, invalid) = rest.split_at(error.valid_up_to());
                text.push_str(&String::from_utf8_lossy(valid));
                text.push('\u{FFFD}');
                rest = &invalid[1..];
            }
        }
    }
}

pub(crate) fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str::<GoJson>(text).map(|GoJson(value)| value)
}

struct GoJson(Value);

impl<'de> Deserialize<'de> for GoJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<GoJson, D::Error> {
        deserializer.deserialize_any(GoJsonVisitor).map(GoJson)
    }
}

struct GoJsonVisitor;

impl<'de> Visitor<'de> for GoJsonVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Value, E> {
        Ok(Number::from_f64(value).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Value, A::Error> {
        let mut list = Vec::new();
        while let Some(GoJson(item)) = items.next_element()? {
            list.push(item);
        }
        Ok(Value::Array(list))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Value, A::Error> {
        let mut members = Members::default();
        while let Some((name, GoJson(value))) = entries.next_entry::<String, GoJson>()? {
            members.insert(name, value);
        }
        Ok(Value::Object(members.into_map()))
    }
}

#[derive(Default)]
struct Members {
    entries: Vec<Option<(String, Value)>>,
    positions: HashMap<String, usize>,
}

impl Members {
    fn insert(&mut self, name: String, value: Value) {
        let key = folded_ascii(&name).unwrap_or_else(|| name.clone());
        if let Some(previous) = self.positions.insert(key, self.entries.len()) {
            self.entries[previous] = None;
        }
        self.entries.push(Some((name, value)));
    }

    fn into_map(self) -> Map<String, Value> {
        self.entries.into_iter().flatten().collect()
    }
}

fn folded_ascii(name: &str) -> Option<String> {
    name.chars()
        .map(|c| match c {
            KELVIN => Some('k'),
            LONG_S => Some('s'),
            _ if c.is_ascii() => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

pub(crate) fn field<'a>(object: &'a Value, name: &str) -> Option<&'a Value> {
    let members = object.as_object()?;
    members.get(name).or_else(|| {
        members
            .iter()
            .find(|(key, _)| equal_fold(key, name))
            .map(|(_, value)| value)
    })
}

pub(crate) fn flex_int(value: Option<&Value>) -> Result<i64, String> {
    match value {
        None | Some(Value::Null) => Ok(0),
        Some(Value::String(text)) => whole_number_text(trim(text)),
        Some(Value::Number(number)) => whole_number(number),
        Some(other) => Err(format!("expected a whole number, got {}", kind_of(other))),
    }
}

fn whole_number(number: &Number) -> Result<i64, String> {
    if let Some(value) = number.as_i64() {
        return Ok(value);
    }
    let float = number
        .as_u64()
        .map(|value| value as f64)
        .or_else(|| number.as_f64());
    float.map_or_else(|| Err(not_whole()), whole_float)
}

fn whole_number_text(text: &str) -> Result<i64, String> {
    if text.is_empty() || text == "null" {
        return Ok(0);
    }
    if let Ok(value) = text.parse::<i64>() {
        return Ok(value);
    }
    float::parse_float(text).map_or_else(|| Err(not_whole()), whole_float)
}

fn whole_float(value: f64) -> Result<i64, String> {
    if value != value.trunc() || value.abs() > i64::MAX as f64 {
        return Err(not_whole());
    }
    Ok(value as i64)
}

fn not_whole() -> String {
    "expected a whole number".to_owned()
}

pub(crate) fn is_empty_json(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(_) => false,
    }
}

pub(crate) fn string_field(object: &Value, name: &str) -> Result<String, String> {
    match field(object, name) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Err(format!("{name}: expected text, got {}", kind_of(other))),
    }
}

pub(crate) fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn lossy_utf8_replaces_each_invalid_byte() {
        let cases: [(&[u8], &str); 7] = [
            (b"a\xe2\x82b", "a\u{FFFD}\u{FFFD}b"),
            (b"\x80x", "\u{FFFD}x"),
            (b"\xc0\xaf", "\u{FFFD}\u{FFFD}"),
            (b"\xed\xa0\x80", "\u{FFFD}\u{FFFD}\u{FFFD}"),
            (b"ok\xf0\x9f\x98", "ok\u{FFFD}\u{FFFD}\u{FFFD}"),
            (b"\xf4\x90\x80\x80", "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"),
            ("çã\u{1F600}".as_bytes(), "çã\u{1F600}"),
        ];
        for (bytes, text) in cases {
            assert_eq!(go_lossy_utf8(bytes), text, "{bytes:x?}");
        }
    }

    #[test]
    fn fields_match_by_exact_name_then_by_go_case_folding() {
        let object = json!({"sigla": "exact", "SIGLA": "upper", "Nome": "folded", "\u{17F}ituacao": "long s", "id_usuario": 7});

        assert_eq!(field(&object, "sigla"), Some(&json!("exact")));
        assert_eq!(field(&object, "nome"), Some(&json!("folded")));
        assert_eq!(field(&object, "situacao"), Some(&json!("long s")));
        assert_eq!(field(&object, "idUsuario"), None);
        assert_eq!(field(&json!(["sigla"]), "sigla"), None);
    }

    #[test]
    fn the_last_of_keys_equal_under_go_folding_wins() {
        let parsed = parse(concat!(
            r#"{"a":1,"A":2,"\u212a":3,"k":4,"#,
            r#""Ação":5,"AÇÃO":6,"x":{"y":1,"Y":2}}"#
        ))
        .unwrap();

        assert_eq!(
            parsed,
            json!({"A": 2, "k": 4, "Ação": 5, "AÇÃO": 6, "x": {"Y": 2}})
        );
    }

    #[test]
    fn whole_numbers_are_read_like_flex_int() {
        let cases = [
            (json!("null"), Ok(0)),
            (json!("\t5\u{a0}"), Ok(5)),
            (json!("0x1p62"), Ok(1 << 62)),
            (json!("0x_1p4"), Ok(16)),
            (json!("1__0"), Err(())),
            (json!("1e1_0"), Ok(10_000_000_000)),
            (json!(".5e1"), Ok(5)),
            (json!("1e400"), Err(())),
            (json!("Inf"), Err(())),
            (json!(false), Err(())),
            (json!(18_446_744_073_709_551_615_u64), Err(())),
            (json!(-(2_f64.powi(63))), Ok(i64::MIN)),
            (json!("9223372036854775808"), Ok(i64::MAX)),
        ];
        for (value, want) in cases {
            assert_eq!(flex_int(Some(&value)).map_err(|_| ()), want, "{value}");
        }
        assert_eq!(flex_int(None), Ok(0));
    }

    #[test]
    fn numbers_in_the_body_are_rounded_like_go() {
        let cases = [
            ("12.000000000000001", Err(())),
            ("9007199254740993.0", Ok(9_007_199_254_740_992)),
            (
                "1023622181409669.000000000000001",
                Ok(1_023_622_181_409_669),
            ),
        ];
        for (raw, want) in cases {
            let value = parse(raw).unwrap();
            assert_eq!(flex_int(Some(&value)).map_err(|_| ()), want, "{raw}");
        }
    }

    #[test]
    fn text_fields_accept_text_and_null_only() {
        let object = json!({"a": "x", "b": null, "c": 1});

        assert_eq!(string_field(&object, "a"), Ok("x".to_owned()));
        assert_eq!(string_field(&object, "b"), Ok(String::new()));
        assert_eq!(string_field(&object, "missing"), Ok(String::new()));
        assert!(string_field(&object, "c").is_err());
    }
}
