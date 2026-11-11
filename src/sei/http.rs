use std::error::Error;
use std::time::Duration;

use chrono_tz::Tz;
use reqwest::redirect::Policy;
use reqwest::{Certificate, Response};
use webpki_root_certs::TLS_SERVER_ROOT_CERTS;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_BODY_SIZE: usize = 16 << 20;
const MAX_PAGE_REDIRECTS: usize = 10;

#[derive(Clone)]
pub struct Options {
    pub user_agent: String,
    pub zone: Tz,
    pub timeout: Duration,
}

pub(crate) fn api_client(options: &Options) -> reqwest::Client {
    client(options, Policy::none())
}

#[expect(
    dead_code,
    reason = "the login page reader of the discovery task is its first caller"
)]
pub(crate) fn page_client(options: &Options) -> reqwest::Client {
    client(options, Policy::limited(MAX_PAGE_REDIRECTS))
}

fn client(options: &Options, redirects: Policy) -> reqwest::Client {
    build(options, redirects)
        .expect("the HTTP client is built from the bundled roots and a constant user agent")
}

fn build(options: &Options, redirects: Policy) -> Result<reqwest::Client, reqwest::Error> {
    let roots = TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|root| Certificate::from_der(root))
        .collect::<Result<Vec<_>, _>>()?;
    reqwest::Client::builder()
        .user_agent(options.user_agent.as_str())
        .timeout(options.timeout)
        .gzip(true)
        .redirect(redirects)
        .tls_certs_only(roots)
        .build()
}

pub(crate) async fn read_capped(mut response: Response) -> Result<Vec<u8>, reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let room = MAX_BODY_SIZE - body.len();
        if chunk.len() >= room {
            append(&mut body, &chunk[..room]);
            return Ok(body);
        }
        append(&mut body, &chunk);
    }
    Ok(body)
}

fn append(body: &mut Vec<u8>, bytes: &[u8]) {
    let wanted = (body.len() + bytes.len())
        .max(body.capacity() * 2)
        .min(MAX_BODY_SIZE);
    body.reserve_exact(wanted - body.len());
    body.extend_from_slice(bytes);
}

pub(crate) fn describe_transport(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(current) = cause {
        text.push_str(": ");
        text.push_str(&current.to_string());
        cause = current.source();
    }
    text
}
