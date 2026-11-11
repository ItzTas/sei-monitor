mod charset;
mod entity;
mod media_type;
#[cfg(test)]
mod tests;
mod tokenizer;
mod tree;

use std::borrow::Cow;
use std::error::Error;
use std::io;

use encoding_rs::Encoding;
use html5ever::driver::{ParseOpts, parse_document};
use html5ever::tendril::TendrilSink;
use html5ever::tokenizer::TokenizerOpts;
use reqwest::Response;
use reqwest::header::CONTENT_TYPE;
use scraper::{ElementRef, Html};

use self::charset::{Detected, PREVIEW_SIZE, determine};
use self::tree::GoTreeSink;
use super::errors::SeiError;
use super::http::{Options, PartialBody, describe_transport, page_client, read_body};
use super::json::go_lossy_utf8;
use super::types::Org;
use crate::gotext::{atoi, equal_fold, fields, trim};

const ORG_SELECT: &str = "selOrgao";

pub async fn list_orgs(sei_url: &str, options: &Options) -> Result<Vec<Org>, SeiError> {
    let client = page_client(options);
    let request = client
        .get(format!("{sei_url}/sei/"))
        .build()
        .map_err(|error| transport("build the login page request", error))?;
    let response = client
        .execute(request)
        .await
        .map_err(|error| transport("open the login page", error))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(SeiError::LoginPageStatus(status));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .map(|value| go_lossy_utf8(value.as_bytes()));
    let body = read_page(response).await?;
    parse_login_page(content_type.as_deref(), &body)
}

pub async fn resolve_org(sei_url: &str, value: &str, options: &Options) -> Result<Org, SeiError> {
    let value = trim(value);
    if let Some(id) = atoi(value).filter(|id| *id > 0) {
        return Ok(Org {
            id,
            acronym: String::new(),
        });
    }
    list_orgs(sei_url, options)
        .await?
        .into_iter()
        .find(|org| equal_fold(&org.acronym, value))
        .ok_or_else(|| SeiError::OrgNotFound(value.to_owned()))
}

pub(crate) fn parse_login_page(
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Vec<Org>, SeiError> {
    if body.is_empty() {
        return Err(SeiError::Transport {
            context: "decode the login page".to_owned(),
            detail: "EOF".to_owned(),
        });
    }
    let text = decode(determine(content_type, body), body);
    let orgs = agency_options(&parse(&text));
    if orgs.is_empty() {
        return Err(SeiError::NoAgencyList);
    }
    Ok(orgs)
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "parse_login_page also needs to know when Go leaves the bytes undecoded"
    )
)]
pub(crate) fn detect_encoding(content_type: Option<&str>, body: &[u8]) -> &'static Encoding {
    determine(content_type, body).encoding
}

async fn read_page(response: Response) -> Result<Vec<u8>, SeiError> {
    read_body(response).await.or_else(page_read_failure)
}

fn page_read_failure(partial: PartialBody) -> Result<Vec<u8>, SeiError> {
    if partial.received() >= PREVIEW_SIZE {
        return Err(transport("read the login page", partial.error));
    }
    if !ended_early(&partial.error) {
        return Err(transport("decode the login page", partial.error));
    }
    let received = partial.into_received();
    if received.is_empty() {
        return Err(SeiError::NoAgencyList);
    }
    Ok(received)
}

fn ended_early(error: &reqwest::Error) -> bool {
    let mut cause: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(current) = cause {
        let unexpected_eof = current
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::UnexpectedEof);
        if unexpected_eof {
            return true;
        }
        cause = current.source();
    }
    false
}

fn transport(context: &str, error: reqwest::Error) -> SeiError {
    SeiError::Transport {
        context: context.to_owned(),
        detail: describe_transport(error),
    }
}

fn decode(detected: Detected, body: &[u8]) -> Cow<'_, str> {
    if !detected.transcode {
        return Cow::Owned(go_lossy_utf8(body));
    }
    let text = detected.encoding.decode_without_bom_handling(body).0;
    if !detected.encoding.is_single_byte() || !text.contains(is_c1_control) {
        return text;
    }
    Cow::Owned(text.chars().map(without_c1_control).collect())
}

fn is_c1_control(c: char) -> bool {
    matches!(c, '\u{80}'..='\u{9F}')
}

fn without_c1_control(c: char) -> char {
    if is_c1_control(c) {
        return '\u{FFFD}';
    }
    c
}

fn parse(text: &str) -> Html {
    let options = ParseOpts {
        tokenizer: TokenizerOpts {
            discard_bom: false,
            ..TokenizerOpts::default()
        },
        ..ParseOpts::default()
    };
    parse_document(GoTreeSink::new(), options).one(text)
}

fn agency_options(document: &Html) -> Vec<Org> {
    document
        .tree
        .root()
        .descendants()
        .filter_map(ElementRef::wrap)
        .find(is_agency_select)
        .map(options)
        .unwrap_or_default()
}

fn is_agency_select(element: &ElementRef) -> bool {
    let element = element.value();
    &*element.name.local == "select"
        && (element.attr("id") == Some(ORG_SELECT) || element.attr("name") == Some(ORG_SELECT))
}

fn options(select: ElementRef) -> Vec<Org> {
    select
        .descendants()
        .filter_map(ElementRef::wrap)
        .filter(|element| &*element.value().name.local == "option")
        .filter_map(option_org)
        .collect()
}

fn option_org(option: ElementRef) -> Option<Org> {
    let value = option.value().attr("value").unwrap_or_default();
    let id = atoi(trim(value)).filter(|id| *id > 0)?;
    let text: String = option.text().collect();
    Some(Org {
        id,
        acronym: fields(&text).join(" "),
    })
}
