use std::collections::HashMap;

use axum::http::header::{ALLOW, CONTENT_TYPE, LOCATION, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

const FORM: &str = "application/x-www-form-urlencoded";
const MAX_FORM_SIZE: usize = 10 << 20;
const MAX_QUERY_PARAMS: usize = 10_000;

#[derive(Default)]
pub(super) struct Values(HashMap<Vec<u8>, Vec<Vec<u8>>>);

impl Values {
    pub(super) fn get(&self, key: &str) -> &[u8] {
        self.0
            .get(key.as_bytes())
            .and_then(|values| values.first())
            .map_or(&[], Vec::as_slice)
    }
}

pub(super) fn query(raw: &str) -> Values {
    parse_query(raw.as_bytes()).0
}

pub(super) fn post_form(content_type: &[u8], raw_query: &str, body: &[u8]) -> Option<Values> {
    let content_type = String::from_utf8_lossy(content_type);
    let content_type = match content_type.as_ref() {
        "" => "application/octet-stream",
        given => given,
    };
    let values = match media_type(content_type)?.as_str() {
        FORM if body.len() > MAX_FORM_SIZE => return None,
        FORM => checked(parse_query(body))?,
        _ => Values::default(),
    };
    checked(parse_query(raw_query.as_bytes())).map(|_| values)
}

fn checked((values, valid): (Values, bool)) -> Option<Values> {
    valid.then_some(values)
}

fn parse_query(raw: &[u8]) -> (Values, bool) {
    let mut values = Values::default();
    if raw.iter().filter(|byte| **byte == b'&').count() + 1 > MAX_QUERY_PARAMS {
        return (values, false);
    }
    let mut valid = true;
    for pair in raw.split(|byte| *byte == b'&') {
        if pair.contains(&b';') {
            valid = false;
            continue;
        }
        if pair.is_empty() {
            continue;
        }
        let (key, value) = split_once(pair, b'=');
        let (Some(key), Some(value)) = (unescape(key), unescape(value)) else {
            valid = false;
            continue;
        };
        values.0.entry(key).or_default().push(value);
    }
    (values, valid)
}

fn split_once(bytes: &[u8], separator: u8) -> (&[u8], &[u8]) {
    match bytes.iter().position(|byte| *byte == separator) {
        Some(at) => (&bytes[..at], &bytes[at + 1..]),
        None => (bytes, &[]),
    }
}

fn unescape(escaped: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(escaped.len());
    let mut rest = escaped;
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        match byte {
            b'%' => {
                let (high, low) = (hex(*rest.first()?)?, hex(*rest.get(1)?)?);
                decoded.push(high << 4 | low);
                rest = &rest[2..];
            }
            b'+' => decoded.push(b' '),
            other => decoded.push(other),
        }
    }
    Some(decoded)
}

fn hex(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

fn media_type(value: &str) -> Option<String> {
    let base = value.split(';').next().unwrap_or_default();
    let media = base.to_lowercase().trim().to_owned();
    valid_disposition(&media)?;
    let mut params: HashMap<String, Vec<u8>> = HashMap::new();
    let mut rest = &value[base.len()..];
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Some(media);
        }
        let Some((key, param, tail)) = media_param(rest) else {
            return (rest.trim() == ";").then_some(media);
        };
        if params.get(&key).is_some_and(|known| *known != param) {
            return None;
        }
        params.insert(key, param);
        rest = tail;
    }
}

fn valid_disposition(media: &str) -> Option<()> {
    let (kind, rest) = token(media);
    if kind.is_empty() {
        return None;
    }
    if rest.is_empty() {
        return Some(());
    }
    let (subtype, rest) = token(rest.strip_prefix('/')?);
    (!subtype.is_empty() && rest.is_empty()).then_some(())
}

fn media_param(text: &str) -> Option<(String, Vec<u8>, &str)> {
    let rest = text.trim_start().strip_prefix(';')?.trim_start();
    let (key, rest) = token(rest);
    if key.is_empty() {
        return None;
    }
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let (param, tail) = param_value(rest);
    if param.is_empty() && tail.len() == rest.len() {
        return None;
    }
    Some((key.to_lowercase(), param, tail))
}

fn param_value(text: &str) -> (Vec<u8>, &str) {
    let Some(quoted) = text.strip_prefix('"') else {
        let (value, rest) = token(text);
        return (value.as_bytes().to_vec(), rest);
    };
    let bytes = quoted.as_bytes();
    let mut value = Vec::new();
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' => return (value, &quoted[at + 1..]),
            b'\\' if bytes.get(at + 1).is_some_and(|next| is_tspecial(*next)) => {
                value.push(bytes[at + 1]);
                at += 1;
            }
            b'\r' | b'\n' => return (Vec::new(), text),
            other => value.push(other),
        }
        at += 1;
    }
    (Vec::new(), text)
}

fn token(text: &str) -> (&str, &str) {
    let end = text
        .bytes()
        .position(|byte| !is_token_byte(byte))
        .unwrap_or(text.len());
    text.split_at(end)
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_graphic() && !is_tspecial(byte)
}

fn is_tspecial(byte: u8) -> bool {
    b"()<>@,;:\\\"/[]?=".contains(&byte)
}

pub(super) fn clean_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            name => segments.push(name),
        }
    }
    let clean = format!("/{}", segments.join("/"));
    if path.ends_with('/') && clean != "/" {
        return format!("{clean}/");
    }
    clean
}

pub(super) fn json(status: StatusCode, mut body: Value) -> Response {
    body.sort_all_objects();
    let escapes = [
        ('<', "\\u003c"),
        ('>', "\\u003e"),
        ('&', "\\u0026"),
        ('\u{2028}', "\\u2028"),
        ('\u{2029}', "\\u2029"),
    ];
    let text = escape(&body.to_string(), &escapes) + "\n";
    (status, [(CONTENT_TYPE, "application/json")], text).into_response()
}

pub(super) fn redirect(method: &Method, status: StatusCode, location: &str) -> Response {
    let mut response = (status, [(LOCATION, location.to_owned())]).into_response();
    if method != Method::GET && method != Method::HEAD {
        return response;
    }
    let escapes = [
        ('&', "&amp;"),
        ('\'', "&#39;"),
        ('<', "&lt;"),
        ('>', "&gt;"),
        ('"', "&#34;"),
    ];
    let href = escape(location, &escapes);
    let text = status.canonical_reason().unwrap_or_default();
    *response.body_mut() = format!("<a href=\"{href}\">{text}</a>.\n\n").into();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
}

fn escape(text: &str, escapes: &[(char, &str)]) -> String {
    text.chars()
        .fold(String::with_capacity(text.len()), |mut out, c| {
            match escapes.iter().find(|(from, _)| *from == c) {
                Some((_, escaped)) => out.push_str(escaped),
                None => out.push(c),
            }
            out
        })
}

pub(super) fn plain_error(status: StatusCode, message: &str) -> Response {
    let headers = [
        (CONTENT_TYPE, "text/plain; charset=utf-8"),
        (X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ];
    (status, headers, format!("{message}\n")).into_response()
}

pub(super) fn method_not_allowed(allow: &'static str) -> Response {
    let mut response = plain_error(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed");
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    response
}
