use std::collections::HashMap;
use std::sync::OnceLock;

const LONGEST_ENTITY_WITHOUT_SEMICOLON: usize = 6;

const WINDOWS_1252: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

fn entities() -> &'static HashMap<String, String> {
    static ENTITIES: OnceLock<HashMap<String, String>> = OnceLock::new();
    ENTITIES.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/golden/html_entities.json"
        )))
        .expect("testdata/golden/html_entities.json holds Go's entity table")
    })
}

pub(crate) fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let consumed = unescape_entity(&rest[at..], &mut out);
        rest = &rest[at + consumed..];
    }
    out.push_str(rest);
    out
}

fn unescape_entity(reference: &str, out: &mut String) -> usize {
    let bytes = reference.as_bytes();
    if bytes.len() <= 1 {
        out.push('&');
        return 1;
    }
    if bytes[1] == b'#' {
        return numeric(bytes, out);
    }
    named(reference, out)
}

fn numeric(bytes: &[u8], out: &mut String) -> usize {
    if bytes.len() <= 3 {
        out.push('&');
        return 1;
    }
    let hex = matches!(bytes[2], b'x' | b'X');
    let radix: u32 = if hex { 16 } else { 10 };
    let mut i = if hex { 3 } else { 2 };
    let mut value: i32 = 0;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        if let Some(digit) = char::from(c).to_digit(radix) {
            value = value.wrapping_mul(radix as i32).wrapping_add(digit as i32);
            continue;
        }
        if c != b';' {
            i -= 1;
        }
        break;
    }
    if i <= 3 {
        out.push('&');
        return 1;
    }
    out.push(numeric_char(value));
    i
}

fn numeric_char(value: i32) -> char {
    if (0x80..=0x9F).contains(&value) {
        return WINDOWS_1252[(value - 0x80) as usize];
    }
    if value == 0 {
        return '\u{FFFD}';
    }
    u32::try_from(value)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or('\u{FFFD}')
}

fn named(reference: &str, out: &mut String) -> usize {
    let bytes = reference.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        if c.is_ascii_alphanumeric() {
            continue;
        }
        if c != b';' {
            i -= 1;
        }
        break;
    }
    let name = &reference[1..i];
    if name.is_empty() {
        out.push('&');
        return 1;
    }
    if let Some(value) = entities().get(name) {
        out.push_str(value);
        return i;
    }
    let longest = (name.len() - 1).min(LONGEST_ENTITY_WITHOUT_SEMICOLON);
    let prefix = (2..=longest)
        .rev()
        .find_map(|end| single_rune(&name[..end]).map(|value| (end, value)));
    if let Some((end, value)) = prefix {
        out.push(value);
        return end + 1;
    }
    out.push_str(&reference[..i]);
    i
}

fn single_rune(name: &str) -> Option<char> {
    let mut chars = entities().get(name)?.chars();
    match (chars.next(), chars.next()) {
        (Some(value), None) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::golden;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct UnescapeCase {
        input: String,
        output: String,
    }

    #[test]
    fn html_unescape_matches_go() {
        for case in golden::cases::<UnescapeCase>("html_unescape", "") {
            assert_eq!(unescape(&case.input), case.output, "{:?}", case.input);
        }
    }

    #[test]
    fn numeric_references_follow_go() {
        let cases = [
            ("&#x;", "\u{FFFD}"),
            ("&#X;", "\u{FFFD}"),
            ("&#4294967361;", "A"),
            ("&#2147483648;", "\u{FFFD}"),
            ("&#x80000041;", "\u{FFFD}"),
            ("&#x9F;", "\u{178}"),
            ("&#xDFFF;", "\u{FFFD}"),
            ("&#x10FFFF;", "\u{10FFFF}"),
            ("&#5x", "&#5x"),
            ("&#5;", "\u{5}"),
            ("&#x0d;", "\r"),
        ];
        for (input, output) in cases {
            assert_eq!(unescape(input), output, "{input:?}");
        }
    }

    #[test]
    fn names_follow_go() {
        let cases = [
            ("&notin", "\u{AC}in"),
            ("&nGt;", "&nGt;"),
            ("&nLt;", "&nLt;"),
            ("&nGt", "&nGt"),
            ("&Eacute", "\u{C9}"),
            ("&eacutex", "\u{E9}x"),
            ("&frac12x", "\u{BD}x"),
            ("&frac123", "\u{BD}3"),
            ("&ab", "&ab"),
        ];
        for (input, output) in cases {
            assert_eq!(unescape(input), output, "{input:?}");
        }
    }

    #[test]
    fn the_table_is_go_own() {
        let table = entities();
        assert_eq!(table.len(), 2229);
        assert!(!table.contains_key("nGt;"));
        assert!(!table.contains_key("nLt;"));
        let longest_bare = table
            .keys()
            .filter(|name| !name.ends_with(';'))
            .map(|name| name.len())
            .max();
        assert_eq!(longest_bare, Some(LONGEST_ENTITY_WITHOUT_SEMICOLON));
    }
}
