use std::error::Error;
use std::mem;
use std::time::Duration;

use chrono_tz::Tz;
use reqwest::redirect::Policy;
use reqwest::{Certificate, Response};
use webpki_root_certs::TLS_SERVER_ROOT_CERTS;
use zeroize::Zeroizing;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_BODY_SIZE: usize = 16 << 20;
const MAX_PAGE_REDIRECTS: usize = 9;

#[derive(Clone)]
pub struct Options {
    pub user_agent: String,
    pub zone: Tz,
    pub timeout: Duration,
}

pub(crate) fn api_client(options: &Options) -> reqwest::Client {
    client(options, Policy::none())
}

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
        .http1_title_case_headers()
        .redirect(redirects)
        .tls_certs_only(roots)
        .build()
}

pub(crate) async fn read_capped(response: Response) -> Result<Vec<u8>, reqwest::Error> {
    read_body(response).await.map_err(|partial| partial.error)
}

pub(crate) struct PartialBody {
    received: CappedBody,
    pub(crate) error: reqwest::Error,
}

impl PartialBody {
    pub(crate) fn received(&self) -> usize {
        self.received.bytes.len()
    }
}

pub(crate) async fn read_body(mut response: Response) -> Result<Vec<u8>, PartialBody> {
    let mut body = CappedBody::new(response.content_length());
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                return Err(PartialBody {
                    received: body,
                    error,
                });
            }
        };
        if body.push(&chunk) {
            break;
        }
    }
    Ok(body.into_bytes())
}

struct CappedBody {
    bytes: Zeroizing<Vec<u8>>,
}

impl CappedBody {
    fn new(length: Option<u64>) -> CappedBody {
        let hint = length.map_or(0, |length| {
            usize::try_from(length).map_or(MAX_BODY_SIZE, |length| length.min(MAX_BODY_SIZE))
        });
        CappedBody {
            bytes: Zeroizing::new(Vec::with_capacity(hint)),
        }
    }

    fn push(&mut self, chunk: &[u8]) -> bool {
        let room = MAX_BODY_SIZE - self.bytes.len();
        let kept = &chunk[..chunk.len().min(room)];
        self.reserve(kept.len());
        self.bytes.extend_from_slice(kept);
        self.bytes.len() == MAX_BODY_SIZE
    }

    fn reserve(&mut self, additional: usize) {
        let needed = self.bytes.len() + additional;
        if needed <= self.bytes.capacity() {
            return;
        }
        let capacity = needed.max(self.bytes.capacity() * 2).min(MAX_BODY_SIZE);
        let mut grown = Zeroizing::new(Vec::with_capacity(capacity));
        grown.extend_from_slice(&self.bytes);
        self.bytes = grown;
    }

    fn into_bytes(mut self) -> Vec<u8> {
        mem::take(&mut *self.bytes)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = 8 << 10;

    fn fill(body: &mut CappedBody, total: usize) -> bool {
        let chunk = [b'a'; CHUNK];
        let mut full = false;
        let mut sent = 0;
        while sent < total && !full {
            let size = CHUNK.min(total - sent);
            full = body.push(&chunk[..size]);
            sent += size;
        }
        full
    }

    #[test]
    fn small_body_keeps_a_small_buffer() {
        let mut body = CappedBody::new(None);

        let full = fill(&mut body, 200_000);

        assert!(!full);
        assert_eq!(body.bytes.len(), 200_000);
        assert!(body.bytes.capacity() < 1 << 20, "{}", body.bytes.capacity());
        assert!(body.into_bytes().iter().all(|byte| *byte == b'a'));
    }

    #[test]
    fn announced_length_sizes_the_buffer_once() {
        let mut body = CappedBody::new(Some(200_000));

        fill(&mut body, 200_000);

        assert_eq!(body.bytes.capacity(), 200_000);
    }

    #[test]
    fn body_at_the_cap_ends_at_the_cap() {
        let mut body = CappedBody::new(None);
        body.push(b"abc");

        let full = fill(&mut body, MAX_BODY_SIZE - 3);

        assert!(full);
        assert_eq!(body.bytes.len(), MAX_BODY_SIZE);
        assert_eq!(body.bytes.capacity(), MAX_BODY_SIZE);
    }

    #[test]
    fn body_past_the_cap_is_cut_at_the_cap() {
        let mut body = CappedBody::new(Some(u64::MAX));

        let full = fill(&mut body, MAX_BODY_SIZE + 3 * CHUNK + 5);

        assert!(full);
        assert!(body.push(b"more"));
        assert_eq!(body.bytes.capacity(), MAX_BODY_SIZE);
        assert_eq!(body.into_bytes().len(), MAX_BODY_SIZE);
    }
}
