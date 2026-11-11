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
    let (Some((first, _)), Some((last, _))) = (table.first(), table.last()) else {
        return None;
    };
    if c < *first || c > *last {
        return None;
    }
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

pub(crate) const TEXT_LIMIT: usize = 500;

pub(crate) fn capped(text: &str) -> &str {
    text.char_indices()
        .nth(TEXT_LIMIT)
        .map_or(text, |(end, _)| &text[..end])
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

    #[test]
    fn capped_keeps_the_first_characters_up_to_the_limit() {
        let short = "ç".repeat(TEXT_LIMIT);
        assert_eq!(capped(&short), short);
        let long = format!("{short}ãé");
        assert_eq!(capped(&long), short);
        assert_eq!(capped(""), "");
    }

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

    const GO_CASE: &str = include_str!("gotext/testdata/simple_case.txt");
    const REGENERATE: &str =
        "go run src/gotext/testdata/simple_case.go > src/gotext/testdata/simple_case.txt";

    struct GoCase {
        toolchain: &'static str,
        mappings: Vec<(char, [char; 3])>,
    }

    fn go_case() -> GoCase {
        let mut lines = GO_CASE.lines();
        let toolchain = lines.next().unwrap_or_default();
        let mappings = lines
            .map(|line| {
                let chars: Vec<char> = line
                    .split(' ')
                    .map(|hex| char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap())
                    .collect();
                (chars[0], [chars[1], chars[2], chars[3]])
            })
            .collect();
        GoCase {
            toolchain,
            mappings,
        }
    }

    fn std_lower(c: char) -> char {
        c.to_lowercase().next().unwrap_or(c)
    }

    fn std_upper(c: char) -> char {
        let mut upper = c.to_uppercase();
        match (upper.next(), upper.next()) {
            (Some(single), None) => single,
            _ => c,
        }
    }

    fn table_source(name: &str, entries: &[(char, char)]) -> String {
        if entries.is_empty() {
            return format!("pub(super) const {name}: [(char, char); 0] = [];\n");
        }
        let rows: String = entries
            .iter()
            .map(|(from, to)| {
                format!(
                    "    ('\\u{{{:x}}}', '\\u{{{:x}}}'),\n",
                    u32::from(*from),
                    u32::from(*to)
                )
            })
            .collect();
        format!(
            "pub(super) const {name}: [(char, char); {}] = [\n{rows}];\n",
            entries.len()
        )
    }

    fn tables_for_this_toolchain(mappings: &[(char, [char; 3])]) -> String {
        let pick = |rule: &dyn Fn(char, [char; 3]) -> Option<char>| -> Vec<(char, char)> {
            mappings
                .iter()
                .filter_map(|(c, go)| rule(*c, *go).map(|to| (*c, to)))
                .collect()
        };
        let lower = pick(&|c, [lower, _, _]| (std_lower(c) != lower).then_some(lower));
        let upper = pick(&|c, [_, upper, _]| (std_upper(c) != upper).then_some(upper));
        let fold = pick(&|c, [lower, upper, fold]| {
            let fallback = if lower != c { lower } else { upper };
            (fold != fallback).then_some(fold)
        });
        [
            table_source("LOWER", &lower),
            table_source("UPPER", &upper),
            table_source("FOLD", &fold),
        ]
        .join("\n")
    }

    #[test]
    fn case_mappings_match_go_for_every_char() {
        let go = go_case();
        let mut listed = go.mappings.iter().peekable();
        let mut differing = Vec::new();
        for c in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            let want = match listed.next_if(|(listed, _)| *listed == c) {
                Some((_, mappings)) => *mappings,
                None => [c, c, c],
            };
            if [simple_lower(c), simple_upper(c), simple_fold(c)] != want {
                differing.push(c);
            }
        }
        assert!(
            listed.next().is_none(),
            "src/gotext/testdata/simple_case.txt is not sorted; regenerate it with `{REGENERATE}`"
        );
        assert!(
            differing.is_empty(),
            "{} chars map differently from Go's unicode.ToLower, ToUpper or SimpleFold \
             (expected list from {}), first {:?}. If Go changed, regenerate the list from the \
             repository root with `{REGENERATE}`. Then replace src/gotext/tables.rs with these \
             exception tables, computed for this Rust toolchain:\n\n{}",
            differing.len(),
            go.toolchain,
            &differing[..differing.len().min(10)],
            tables_for_this_toolchain(&go.mappings)
        );
    }

    #[test]
    fn exception_tables_are_strictly_sorted() {
        let tables: [(&str, &[(char, char)]); 3] = [
            ("LOWER", &tables::LOWER),
            ("UPPER", &tables::UPPER),
            ("FOLD", &tables::FOLD),
        ];
        for (name, table) in tables {
            assert!(
                table.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "{name} in src/gotext/tables.rs is not strictly sorted"
            );
        }
    }
}
