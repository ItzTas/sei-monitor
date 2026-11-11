use base64::prelude::{BASE64_STANDARD, Engine as _};
use encoding_rs::{Encoding, KOI8_R, UTF_8, UTF_16LE, WINDOWS_1252};
use serde::Deserialize;

use super::*;
use crate::golden;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageCase {
    name: String,
    content_type: String,
    body_base64: String,
    orgs: Option<Vec<GoOrg>>,
    error: Option<String>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct GoOrg {
    id: i64,
    acronym: String,
}

fn content_type(case: &PageCase) -> Option<&str> {
    Some(case.content_type.as_str()).filter(|value| !value.is_empty())
}

fn outcome(result: Result<Vec<Org>, SeiError>) -> (Option<Vec<GoOrg>>, Option<String>) {
    match result {
        Ok(orgs) => {
            let orgs = orgs
                .into_iter()
                .map(|org| GoOrg {
                    id: org.id,
                    acronym: org.acronym,
                })
                .collect();
            (Some(orgs), None)
        }
        Err(error) => (None, Some(error.to_string())),
    }
}

#[test]
fn login_page_matches_go() {
    for case in golden::cases::<PageCase>("orgs_page", "") {
        let body = BASE64_STANDARD
            .decode(&case.body_base64)
            .unwrap_or_else(|err| panic!("{}: body_base64: {err}", case.name));
        let (orgs, error) = outcome(parse_login_page(content_type(&case), &body, None));
        assert_eq!(orgs, case.orgs, "{}", case.name);
        assert_eq!(error, case.error, "{}", case.name);
    }
}

#[test]
fn single_byte_pages_turn_c1_controls_into_replacement_characters_like_go() {
    let page = |text: &[u8]| {
        let mut body = b"<select id=\"selOrgao\"><option value=\"1\">".to_vec();
        body.extend_from_slice(text);
        body.extend_from_slice(b"</option></select>");
        body
    };
    let cases: [(&str, &[u8], &str); 4] = [
        (
            "text/html; charset=iso-8859-1",
            b"A\x81B\x80\xc3",
            "A\u{FFFD}B\u{20AC}\u{C3}",
        ),
        (
            "text/html; charset=iso-8859-2",
            b"A\x85B\xa9",
            "A\u{FFFD}B\u{160}",
        ),
        ("text/html; charset=koi8-r", b"\x80", "\u{2500}"),
        ("text/html; charset=utf-8", b"\xc2\x81", "\u{81}"),
    ];
    for (content_type, text, acronym) in cases {
        let orgs = parse_login_page(Some(content_type), &page(text), None).expect(content_type);
        assert_eq!(orgs[0].acronym, acronym, "{content_type}");
    }
}

type PageOutcome = Result<Vec<(i64, &'static str)>, &'static str>;

#[test]
fn pages_are_built_like_the_go_parser() {
    let cases: [(&str, &str, PageOutcome); 5] = [
        (
            "a template in foreign content ends the page",
            "<select id=\"selOrgao\"><option value=\"1\">A</option></select>\
             <svg><desc><template>x</template></desc></svg>\
             <select name=\"selOrgao\"><option value=\"2\">B</option></select>",
            Ok(vec![(1, "A")]),
        ),
        (
            "nothing after that template is read",
            "<svg><desc><template></template></desc></svg>\
             <select id=\"selOrgao\"><option value=\"2\">B</option></select>",
            Err("sei: the login page has no agency list (selOrgao field)"),
        ),
        (
            "annotation-xml for HTML is an integration point",
            "<math><annotation-xml encoding=\"TEXT/HTML\"><select id=\"selOrgao\">\
             <option value=\"3\">C<option value=\"4\">D</select></annotation-xml></math>",
            Ok(vec![(3, "C"), (4, "D")]),
        ),
        (
            "other annotation-xml keeps MathML rules",
            "<math><annotation-xml><select id=\"selOrgao\">\
             <option value=\"3\">C<option value=\"4\">D</select></annotation-xml></math>",
            Ok(vec![(3, "CD"), (4, "D")]),
        ),
        (
            "a shadow root template is an ordinary template",
            "<select id=\"selOrgao\"><template shadowrootmode=\"open\">\
             <option value=\"5\">E</option></template></select>",
            Ok(vec![(5, "E")]),
        ),
    ];
    for (name, page, want) in cases {
        let found = parse_login_page(None, page.as_bytes(), None)
            .map(|orgs| {
                orgs.into_iter()
                    .map(|org| (org.id, org.acronym))
                    .collect::<Vec<_>>()
            })
            .map_err(|error| error.to_string());
        let want = want
            .map(|orgs| {
                orgs.into_iter()
                    .map(|(id, acronym)| (id, acronym.to_owned()))
                    .collect::<Vec<_>>()
            })
            .map_err(str::to_owned);
        assert_eq!(found, want, "{name}");
    }
}

type EncodingCase = (
    &'static str,
    Option<&'static str>,
    Vec<u8>,
    &'static Encoding,
    bool,
);

#[test]
fn encoding_follows_go_order() {
    let ascii_padding = "a".repeat(1100);
    let late_meta = format!("<!--{ascii_padding}--><meta charset=\"koi8-r\">");
    let cases: [EncodingCase; 10] = [
        (
            "byte order mark before the header",
            Some("text/html; charset=utf-8"),
            b"\xff\xfe<\x00p\x00>\x00".to_vec(),
            UTF_16LE,
            true,
        ),
        (
            "header before the meta tag",
            Some("text/html; charset=ISO-8859-1"),
            b"<meta charset=\"koi8-r\">".to_vec(),
            WINDOWS_1252,
            true,
        ),
        (
            "unknown header label leaves it to the meta tag",
            Some("text/html; charset=x-unknown"),
            b"<meta charset=\"koi8-r\">".to_vec(),
            KOI8_R,
            true,
        ),
        (
            "content attribute without the pragma",
            None,
            b"<meta content=\"text/html; charset=koi8-r\">".to_vec(),
            WINDOWS_1252,
            true,
        ),
        (
            "content attribute with the pragma",
            None,
            b"<META CONTENT='text/html;charset = KOI8-R' HTTP-EQUIV=Content-Type>".to_vec(),
            KOI8_R,
            true,
        ),
        (
            "meta tag naming UTF-16",
            None,
            b"<meta charset=\"utf-16le\"><p>\xe9".to_vec(),
            UTF_8,
            false,
        ),
        (
            "meta tag hidden in a script",
            None,
            b"<script><meta charset=\"koi8-r\"></script>".to_vec(),
            WINDOWS_1252,
            true,
        ),
        (
            "meta tag after the first 1024 bytes",
            None,
            late_meta.into_bytes(),
            WINDOWS_1252,
            true,
        ),
        (
            "UTF-8 declared by the header",
            Some("text/html; charset=utf-8"),
            "<p>ÓRGÃO</p>".as_bytes().to_vec(),
            UTF_8,
            true,
        ),
        (
            "undeclared UTF-8",
            None,
            "<p>ÓRGÃO</p>".as_bytes().to_vec(),
            UTF_8,
            false,
        ),
    ];
    for (name, content_type, body, encoding, transcode) in cases {
        let detected = charset::determine(content_type, &body);
        assert_eq!(
            (detected.encoding.name(), detected.transcode),
            (encoding.name(), transcode),
            "{name}"
        );
    }
}

#[test]
fn the_open_element_stack_stops_at_go_limit() {
    let select = r#"<select id="selOrgao"><option value="1">A</option></select>"#;
    let too_deep = "sei: read the login page: html: open stack of elements exceeds 512 nodes";
    let divs = |count: usize| "<div>".repeat(count);
    let cases = [
        (
            "html, body and 510 elements",
            format!("{select}{}", divs(510)),
            None,
        ),
        (
            "511 elements",
            format!("{select}{}", divs(511)),
            Some(too_deep),
        ),
        (
            "an empty element at 513",
            format!("{select}{}<br>", divs(510)),
            Some(too_deep),
        ),
        (
            "an agency list below the limit",
            format!("{}{select}", divs(600)),
            Some(too_deep),
        ),
        (
            "foster parenting at 512",
            format!("{select}{}<table><p>", divs(508)),
            None,
        ),
        (
            "foster parenting at 513",
            format!("{select}{}<table><p>", divs(509)),
            Some(too_deep),
        ),
    ];
    for (name, page, error) in cases {
        let found =
            parse_login_page(None, page.as_bytes(), None).map_err(|error| error.to_string());
        match error {
            Some(text) => assert_eq!(found, Err(text.to_owned()), "{name}"),
            None => assert_eq!(found.map(|orgs| orgs.len()), Ok(1), "{name}"),
        }
    }
}
