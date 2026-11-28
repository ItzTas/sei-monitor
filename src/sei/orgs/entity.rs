use std::collections::HashMap;
use std::sync::OnceLock;

const WINDOWS_1252: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

const PARSER_ONLY_ENTITIES: [(&str, &str); 2] =
    [("nLt;", "\u{226A}\u{20D2}"), ("nGt;", "\u{226B}\u{20D2}")];

fn entities() -> &'static HashMap<String, String> {
    static ENTITIES: OnceLock<HashMap<String, String>> = OnceLock::new();
    ENTITIES.get_or_init(|| {
        let mut table: HashMap<String, String> = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/golden/html_entities.json"
        )))
        .expect("testdata/golden/html_entities.json holds Go's entity table");
        table.extend(
            PARSER_ONLY_ENTITIES
                .iter()
                .map(|(name, text)| ((*name).to_owned(), (*text).to_owned())),
        );
        table
    })
}

pub(super) fn reference(s: &[u8]) -> Option<(String, usize)> {
    if s.len() <= 1 {
        return None;
    }
    if s[1] == b'#' {
        return numeric(s);
    }
    named(s)
}

fn numeric(s: &[u8]) -> Option<(String, usize)> {
    if s.len() <= 2 {
        return None;
    }
    let hex = matches!(s[2], b'x' | b'X');
    let digits_start = if hex { 3 } else { 2 };
    let radix = if hex { 16 } else { 10 };
    let mut index = digits_start;
    let mut value: u32 = 0;
    while let Some(digit) = s.get(index).and_then(|c| char::from(*c).to_digit(radix)) {
        if value <= 0x10FFFF {
            value = value * radix + digit;
        }
        index += 1;
    }
    if index == digits_start {
        return None;
    }
    if s.get(index) == Some(&b';') {
        index += 1;
    }
    Some((numeric_char(value).to_string(), index))
}

fn numeric_char(value: u32) -> char {
    if let Some(c) = value
        .checked_sub(0x80)
        .and_then(|offset| WINDOWS_1252.get(usize::try_from(offset).ok()?))
    {
        return *c;
    }
    match value {
        0 => '\u{FFFD}',
        _ => char::from_u32(value).unwrap_or('\u{FFFD}'),
    }
}

fn named(s: &[u8]) -> Option<(String, usize)> {
    let mut end = 1;
    while end < s.len() {
        let c = s[end];
        end += 1;
        if c.is_ascii_alphanumeric() {
            continue;
        }
        if c != b';' {
            end -= 1;
        }
        break;
    }
    let name = std::str::from_utf8(&s[1..end]).ok()?;
    if name.is_empty() || (!name.ends_with(';') && s.get(end) == Some(&b'=')) {
        return None;
    }
    entities().get(name).map(|text| (text.clone(), end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_read_like_the_go_parser_in_attributes() {
        let cases: [(&str, Option<(&str, usize)>); 14] = [
            ("&", None),
            ("&#", None),
            ("&#;", None),
            ("&#45;x", Some(("-", 5))),
            ("&#x2d", Some(("-", 5))),
            ("&#128;", Some(("\u{20AC}", 6))),
            ("&#0;", Some(("\u{FFFD}", 4))),
            ("&#xD800;", Some(("\u{FFFD}", 8))),
            ("&#99999999999;", Some(("\u{FFFD}", 14))),
            ("&amp;", Some(("&", 5))),
            ("&amp=", None),
            ("&notit;", None),
            ("&nGt;", Some(("\u{226B}\u{20D2}", 5))),
            ("&quot x", Some(("\"", 5))),
        ];
        for (input, want) in cases {
            let found = reference(input.as_bytes());
            let found = found.as_ref().map(|(text, len)| (text.as_str(), *len));
            assert_eq!(found, want, "{input:?}");
        }
    }
}
