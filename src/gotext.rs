mod tables;

use std::fmt::Write;

use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

pub fn go_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        push_quoted(&mut quoted, c);
    }
    quoted.push('"');
    quoted
}

fn push_quoted(out: &mut String, c: char) {
    if c == '"' || c == '\\' {
        out.push('\\');
        out.push(c);
        return;
    }
    if is_print(c) {
        out.push(c);
        return;
    }
    const NAMED: [(char, &str); 7] = [
        ('\u{7}', "\\a"),
        ('\u{8}', "\\b"),
        ('\u{c}', "\\f"),
        ('\n', "\\n"),
        ('\r', "\\r"),
        ('\t', "\\t"),
        ('\u{b}', "\\v"),
    ];
    if let Some((_, escape)) = NAMED.iter().find(|(named, _)| *named == c) {
        out.push_str(escape);
        return;
    }
    let _ = match u32::from(c) {
        code @ (..0x20 | 0x7f) => write!(out, "\\x{code:02x}"),
        code @ ..0x10000 => write!(out, "\\u{code:04x}"),
        code => write!(out, "\\U{code:08x}"),
    };
}

fn is_print(c: char) -> bool {
    c == ' '
        || matches!(
            c.general_category_group(),
            GeneralCategoryGroup::Letter
                | GeneralCategoryGroup::Mark
                | GeneralCategoryGroup::Number
                | GeneralCategoryGroup::Punctuation
                | GeneralCategoryGroup::Symbol
        )
}

pub fn equal_fold(a: &str, b: &str) -> bool {
    let (mut left, mut right) = (a.chars(), b.chars());
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if chars_fold(x, y) => {}
            _ => return false,
        }
    }
}

fn chars_fold(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    let (low, high) = if a < b { (a, b) } else { (b, a) };
    if high.is_ascii() {
        return low.is_ascii_uppercase() && high == low.to_ascii_lowercase();
    }
    let mut folded = simple_fold(low);
    while folded != low && folded < high {
        folded = simple_fold(folded);
    }
    folded == high
}

fn simple_fold(c: char) -> char {
    if let Some(folded) = exception(&tables::FOLD, c) {
        return folded;
    }
    let lower = simple_lower(c);
    if lower != c {
        return lower;
    }
    simple_upper(c)
}

fn simple_lower(c: char) -> char {
    exception(&tables::LOWER, c).unwrap_or_else(|| c.to_lowercase().next().unwrap_or(c))
}

fn simple_upper(c: char) -> char {
    if let Some(upper) = exception(&tables::UPPER, c) {
        return upper;
    }
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

fn exception(table: &[(char, char)], c: char) -> Option<char> {
    table
        .binary_search_by_key(&c, |(from, _)| *from)
        .ok()
        .map(|index| table[index].1)
}

pub fn is_go_space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

pub(crate) fn trim(text: &str) -> &str {
    text.trim_matches(is_go_space)
}

pub fn fields(text: &str) -> Vec<&str> {
    text.split(is_go_space)
        .filter(|field| !field.is_empty())
        .collect()
}

pub fn to_lower(text: &str) -> String {
    text.chars().map(simple_lower).collect()
}

pub(crate) fn to_upper(text: &str) -> String {
    text.chars().map(simple_upper).collect()
}

pub fn atoi(text: &str) -> Option<i64> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::golden;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TextCase {
        input: String,
        output: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FoldCase {
        a: String,
        b: String,
        equal: bool,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FieldsCase {
        input: String,
        output: Vec<String>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AtoiCase {
        input: String,
        value: Option<i64>,
    }

    #[test]
    fn go_text_matches_go() {
        for case in golden::cases::<TextCase>("go_text", "/quote") {
            assert_eq!(go_quote(&case.input), case.output, "quote {:?}", case.input);
        }
        for case in golden::cases::<FoldCase>("go_text", "/equal_fold") {
            assert_eq!(
                (equal_fold(&case.a, &case.b), equal_fold(&case.b, &case.a)),
                (case.equal, case.equal),
                "equal_fold {:?} {:?}",
                case.a,
                case.b
            );
        }
        for case in golden::cases::<FieldsCase>("go_text", "/fields") {
            assert_eq!(fields(&case.input), case.output, "fields {:?}", case.input);
        }
        for case in golden::cases::<TextCase>("go_text", "/lower") {
            assert_eq!(to_lower(&case.input), case.output, "lower {:?}", case.input);
        }
        for case in golden::cases::<AtoiCase>("go_text", "/atoi") {
            assert_eq!(atoi(&case.input), case.value, "atoi {:?}", case.input);
        }
    }

    #[test]
    fn upper_case_follows_go_simple_mappings() {
        let cases = [
            ("ınfo", "INFO"),
            ("ſ", "S"),
            ("ß", "ß"),
            ("ᾳ", "ᾼ"),
            ("\u{a7cf}", "\u{a7ce}"),
            ("SEDEMO/nuproc", "SEDEMO/NUPROC"),
        ];
        for (input, want) in cases {
            assert_eq!(to_upper(input), want, "{input:?}");
        }
    }
}
