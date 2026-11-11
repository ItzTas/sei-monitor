use std::collections::HashMap;

use crate::gotext::{is_go_space, to_lower, trim};

type Params = HashMap<String, String>;

pub(super) fn charset(content_type: &str) -> Option<String> {
    parameters(content_type)?.remove("charset")
}

fn parameters(value: &str) -> Option<Params> {
    let (base, mut rest) = value.split_at(value.find(';').unwrap_or(value.len()));
    if !is_media_type(trim(&to_lower(base))) {
        return None;
    }
    let mut params = Params::new();
    let mut continuations: HashMap<String, Params> = HashMap::new();
    loop {
        rest = trim_start(rest);
        if rest.is_empty() {
            break;
        }
        let Some((key, value, after)) = parameter(rest) else {
            return (trim(rest) == ";").then(|| finish(params, continuations));
        };
        let map = match key.split_once('*') {
            Some((base_name, _)) => continuations.entry(base_name.to_owned()).or_default(),
            None => &mut params,
        };
        if map.get(&key).is_some_and(|existing| *existing != value) {
            return None;
        }
        map.insert(key, value);
        rest = after;
    }
    Some(finish(params, continuations))
}

fn finish(mut params: Params, continuations: HashMap<String, Params>) -> Params {
    for (key, pieces) in continuations {
        if let Some(value) = joined(&key, &pieces) {
            params.insert(key, value);
        }
    }
    params
}

fn joined(key: &str, pieces: &Params) -> Option<String> {
    if let Some(value) = pieces.get(&format!("{key}*")) {
        return decode_2231(value);
    }
    let mut text = Vec::new();
    let mut valid = false;
    for n in 0.. {
        let simple = format!("{key}*{n}");
        if let Some(value) = pieces.get(&simple) {
            valid = true;
            text.extend_from_slice(value.as_bytes());
            continue;
        }
        let Some(value) = pieces.get(&format!("{simple}*")) else {
            break;
        };
        valid = true;
        let decoded = match n {
            0 => decode_2231(value).map(String::into_bytes),
            _ => percent_unescape(value),
        };
        text.extend(decoded.unwrap_or_default());
    }
    valid.then(|| String::from_utf8_lossy(&text).into_owned())
}

fn decode_2231(value: &str) -> Option<String> {
    let (charset, rest) = value.split_once('\'')?;
    let (_, encoded) = rest.split_once('\'')?;
    if !matches!(to_lower(charset).as_str(), "us-ascii" | "utf-8") {
        return None;
    }
    percent_unescape(encoded).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn percent_unescape(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = hex(*bytes.get(index + 1)?)?;
        let low = hex(*bytes.get(index + 2)?)?;
        out.push(high << 4 | low);
        index += 3;
    }
    Some(out)
}

fn hex(c: u8) -> Option<u8> {
    char::from(c)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

fn is_media_type(text: &str) -> bool {
    let (kind, rest) = token(text);
    if kind.is_empty() {
        return false;
    }
    if rest.is_empty() {
        return true;
    }
    let Some(rest) = rest.strip_prefix('/') else {
        return false;
    };
    let (subtype, rest) = token(rest);
    !subtype.is_empty() && rest.is_empty()
}

fn parameter(text: &str) -> Option<(String, String, &str)> {
    let rest = trim_start(text.strip_prefix(';')?);
    let (key, rest) = token(rest);
    if key.is_empty() {
        return None;
    }
    let rest = trim_start(trim_start(rest).strip_prefix('=')?);
    let (value, after) = value(rest)?;
    Some((to_lower(key), value, after))
}

fn value(text: &str) -> Option<(String, &str)> {
    let Some(quoted) = text.strip_prefix('"') else {
        let (value, rest) = token(text);
        return (!value.is_empty()).then(|| (value.to_owned(), rest));
    };
    let mut value = String::new();
    let mut chars = quoted.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Some((value, &quoted[index + 1..])),
            '\\' if quoted[index + 1..].chars().next().is_some_and(is_special) => {
                value.extend(chars.next().map(|(_, escaped)| escaped));
            }
            '\r' | '\n' => return None,
            _ => value.push(c),
        }
    }
    None
}

fn token(text: &str) -> (&str, &str) {
    text.split_at(text.find(|c: char| !is_token_char(c)).unwrap_or(text.len()))
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_graphic() && !is_special(c)
}

fn is_special(c: char) -> bool {
    "()<>@,;:\\\"/[]?=".contains(c)
}

fn trim_start(text: &str) -> &str {
    text.trim_start_matches(is_go_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_reads_the_parameter_like_go() {
        let cases = [
            ("text/html; charset=ISO-8859-1", Some("ISO-8859-1")),
            ("text/html;charset=\"utf-8\"", Some("utf-8")),
            ("TEXT/HTML ; CHARSET = koi8-r ;", Some("koi8-r")),
            ("text/html; charset=\"a\\\"b\"", Some("a\"b")),
            ("text/html; charset*=utf-8''koi8%2Dr", Some("koi8-r")),
            ("text/html; charset*0=koi8; charset*1=-r", Some("koi8-r")),
            (
                "text/html; charset=utf-8; charset*=iso-8859-1''latin1",
                Some("utf-8"),
            ),
            ("text/html; charset=a; charset=b", None),
            ("text/html; charset=a; charset=a", Some("a")),
            ("text/html charset=utf-8", None),
            ("text/; charset=utf-8", None),
            ("text/html; charset=", None),
            ("text/html; charset=\"open", None),
            ("text/html; charset=utf-8; junk", None),
            ("texthtml; charset=utf-8", Some("utf-8")),
            ("te\u{212a}t/html; charset=utf-8", Some("utf-8")),
            ("", None),
        ];
        for (content_type, label) in cases {
            assert_eq!(charset(content_type).as_deref(), label, "{content_type:?}");
        }
    }
}
