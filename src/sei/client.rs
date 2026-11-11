use std::collections::HashSet;
use std::sync::Arc;

use bytes::Bytes;
use chrono_tz::Tz;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use reqwest::{Method, Request, Response};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use super::errors::{ErrorKind, SeiError};
use super::http::{Options, api_client, describe_transport, read_capped};
use super::types::{Activity, Document, Page, Process, ProcessQuery, RawPage, Session, Version};
use super::wire::{
    self, Envelope, ROUTE_ACTIVITIES, ROUTE_DOCUMENTS, ROUTE_LOGIN, ROUTE_PROCESSES, ROUTE_VERSION,
    login_session, read_envelope,
};
use crate::password::PasswordSource;
use crate::secret::Secret;

pub const PROCESS_PAGE_SIZE: usize = 50;
const MAX_PAGES: usize = 400;
const FORM: &str = "application/x-www-form-urlencoded";
const HEX: &[u8; 16] = b"0123456789ABCDEF";

#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: Arc<dyn PasswordSource>,
    pub org_id: i64,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    base: String,
    credentials: Credentials,
    zone: Tz,
    http: reqwest::Client,
    state: Mutex<TokenState>,
}

#[derive(Default)]
struct TokenState {
    current: Option<SignedIn>,
    generation: u64,
}

#[derive(Clone)]
struct SignedIn {
    token: Secret,
    session: Session,
}

struct Call<'a> {
    method: Method,
    route: &'a str,
    path: String,
    query: String,
    unit: i64,
    form: Option<Bytes>,
}

impl<'a> Call<'a> {
    fn get(route: &'a str, query: &[(&str, &str)], unit: i64) -> Call<'a> {
        Call {
            method: Method::GET,
            route,
            path: route.to_owned(),
            query: encode(query),
            unit,
            form: None,
        }
    }
}

impl Client {
    pub fn new(api_base: &str, credentials: Credentials, options: &Options) -> Client {
        let inner = Inner {
            base: api_base.trim_end_matches('/').to_owned(),
            credentials,
            zone: options.zone,
            http: api_client(options),
            state: Mutex::new(TokenState::default()),
        };
        Client {
            inner: Arc::new(inner),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.inner.base
    }

    pub async fn logins(&self) -> u64 {
        self.inner.state.lock().await.generation
    }

    pub async fn login(&self) -> Result<Session, SeiError> {
        let mut state = self.inner.state.lock().await;
        Ok(self.login_locked(&mut state).await?.session)
    }

    pub async fn session(&self) -> Result<Session, SeiError> {
        let mut state = self.inner.state.lock().await;
        if let Some(current) = &state.current {
            return Ok(current.session.clone());
        }
        Ok(self.login_locked(&mut state).await?.session)
    }

    pub async fn version(&self) -> Result<Version, SeiError> {
        let call = Call::get(ROUTE_VERSION, &[], 0);
        wire::version(&self.authorized(&call).await?)
    }

    pub async fn processes(&self, unit_id: i64) -> Result<Vec<Process>, SeiError> {
        let mut all = Vec::new();
        let mut seen = HashSet::new();
        for page in 0..MAX_PAGES {
            let query = ProcessQuery {
                page,
                limit: PROCESS_PAGE_SIZE,
                kind: String::new(),
            };
            let result = self.process_page(unit_id, &query).await?;
            let listed = result.items.len();
            let before = all.len();
            all.extend(
                result.items.into_iter().filter(|process| {
                    process.procedure_id != 0 && seen.insert(process.procedure_id)
                }),
            );
            if last_page(all.len() - before, listed, all.len(), result.total) {
                return Ok(all);
            }
        }
        Err(SeiError::TooManyPages)
    }

    pub async fn process_page(
        &self,
        unit_id: i64,
        query: &ProcessQuery,
    ) -> Result<Page<Process>, SeiError> {
        let (start, limit) = (query.page.to_string(), query.limit.to_string());
        let mut values = vec![("limit", limit.as_str()), ("start", start.as_str())];
        if !query.kind.is_empty() {
            values.push(("tipo", query.kind.as_str()));
        }
        let call = Call::get(ROUTE_PROCESSES, &values, unit_id);
        wire::process_page(ROUTE_PROCESSES, &self.authorized(&call).await?)
    }

    pub async fn activity_page(
        &self,
        unit_id: i64,
        procedure_id: i64,
        page: usize,
        limit: usize,
    ) -> Result<Page<Activity>, SeiError> {
        let (start, limit, procedure) = (
            page.to_string(),
            limit.to_string(),
            procedure_id.to_string(),
        );
        let values = [
            ("limit", limit.as_str()),
            ("start", start.as_str()),
            ("procedimento", procedure.as_str()),
        ];
        let call = Call::get(ROUTE_ACTIVITIES, &values, unit_id);
        let envelope = self.authorized(&call).await?;
        wire::activity_page(ROUTE_ACTIVITIES, &envelope, procedure_id, self.inner.zone)
    }

    pub async fn document_page(
        &self,
        unit_id: i64,
        procedure_id: i64,
        page: usize,
        limit: usize,
    ) -> Result<Page<Document>, SeiError> {
        let (start, limit) = (page.to_string(), limit.to_string());
        let values = [("limit", limit.as_str()), ("start", start.as_str())];
        let call = Call {
            path: format!("{ROUTE_DOCUMENTS}/{procedure_id}"),
            ..Call::get(ROUTE_DOCUMENTS, &values, unit_id)
        };
        wire::document_page(
            ROUTE_DOCUMENTS,
            &self.authorized(&call).await?,
            procedure_id,
        )
    }

    pub async fn raw(
        &self,
        unit_id: i64,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<RawPage, SeiError> {
        let values: Vec<(&str, &str)> = query
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let envelope = self.authorized(&Call::get(path, &values, unit_id)).await?;
        Ok(RawPage {
            data: envelope.data,
            total: envelope.total,
        })
    }

    async fn authorized(&self, call: &Call<'_>) -> Result<Envelope, SeiError> {
        let (token, generation) = self.current_token().await?;
        let answer = self.send(call, Some(&token)).await;
        if !unauthorized(&answer) {
            return answer;
        }
        let token = self.renew_token(generation).await?;
        self.send(call, Some(&token)).await
    }

    async fn current_token(&self) -> Result<(Secret, u64), SeiError> {
        let mut state = self.inner.state.lock().await;
        if let Some(current) = &state.current {
            return Ok((current.token.clone(), state.generation));
        }
        let current = self.login_locked(&mut state).await?;
        Ok((current.token, state.generation))
    }

    async fn renew_token(&self, seen: u64) -> Result<Secret, SeiError> {
        let mut state = self.inner.state.lock().await;
        if state.generation != seen
            && let Some(current) = &state.current
        {
            return Ok(current.token.clone());
        }
        Ok(self.login_locked(&mut state).await?.token)
    }

    async fn login_locked(&self, state: &mut TokenState) -> Result<SignedIn, SeiError> {
        state.current = None;
        let envelope = self.send(&self.login_call().await?, None).await?;
        let (token, session) = login_session(&envelope)?;
        let current = SignedIn { token, session };
        state.current = Some(current.clone());
        state.generation += 1;
        Ok(current)
    }

    async fn login_call(&self) -> Result<Call<'static>, SeiError> {
        let credentials = &self.inner.credentials;
        let password = credentials
            .password
            .password()
            .await
            .map_err(SeiError::Password)?;
        let org = credentials.org_id.to_string();
        let form = encode(&[
            ("usuario", credentials.username.as_str()),
            ("senha", password.reveal()),
            ("orgao", org.as_str()),
            ("contexto", ""),
        ]);
        Ok(Call {
            method: Method::POST,
            route: ROUTE_LOGIN,
            path: ROUTE_LOGIN.to_owned(),
            query: String::new(),
            unit: 0,
            form: Some(zeroizing(form.into_bytes())),
        })
    }

    async fn send(&self, call: &Call<'_>, token: Option<&Secret>) -> Result<Envelope, SeiError> {
        let request = self.request(call, token)?;
        let response = self
            .inner
            .http
            .execute(request)
            .await
            .map_err(|error| transport(call.route.to_owned(), error))?;
        let status = response.status().as_u16();
        let body = Zeroizing::new(answer_body(call.route, response).await?);
        read_envelope(call.route, status, &body)
    }

    fn request(&self, call: &Call<'_>, token: Option<&Secret>) -> Result<Request, SeiError> {
        let build_context = || format!("build the {} call", call.route);
        let mut builder = self
            .inner
            .http
            .request(call.method.clone(), self.target(call))
            .header(ACCEPT, "application/json");
        if let Some(form) = &call.form {
            builder = builder.header(CONTENT_TYPE, FORM).body(form.clone());
        }
        if let Some(token) = token {
            let value = token_header(token).ok_or_else(|| SeiError::Transport {
                context: build_context(),
                detail: "the session token is not a valid header value".to_owned(),
            })?;
            builder = builder.header("token", value);
        }
        if call.unit != 0 {
            builder = builder.header("unidade", call.unit.to_string());
        }
        builder
            .build()
            .map_err(|error| transport(build_context(), error))
    }

    fn target(&self, call: &Call<'_>) -> String {
        if call.query.is_empty() {
            return format!("{}{}", self.inner.base, call.path);
        }
        format!("{}{}?{}", self.inner.base, call.path, call.query)
    }
}

fn last_page(added: usize, listed: usize, collected: usize, total: i64) -> bool {
    let reached_total = total > 0 && usize::try_from(total).is_ok_and(|total| collected >= total);
    added == 0 || listed < PROCESS_PAGE_SIZE || reached_total
}

fn unauthorized(answer: &Result<Envelope, SeiError>) -> bool {
    matches!(answer, Err(error) if error.kind() == Some(ErrorKind::Unauthorized))
}

async fn answer_body(route: &str, response: Response) -> Result<Vec<u8>, SeiError> {
    if response.status().is_redirection() {
        return Ok(Vec::new());
    }
    read_capped(response)
        .await
        .map_err(|error| SeiError::Decode {
            route: route.to_owned(),
            detail: describe_transport(error),
        })
}

fn transport(context: String, error: reqwest::Error) -> SeiError {
    SeiError::Transport {
        context,
        detail: describe_transport(error),
    }
}

fn token_header(token: &Secret) -> Option<HeaderValue> {
    let mut value =
        HeaderValue::from_maybe_shared(zeroizing(token.reveal().as_bytes().to_vec())).ok()?;
    value.set_sensitive(true);
    Some(value)
}

fn zeroizing(bytes: Vec<u8>) -> Bytes {
    Bytes::from_owner(Zeroizing::new(bytes))
}

fn encode(pairs: &[(&str, &str)]) -> String {
    let mut sorted: Vec<&(&str, &str)> = pairs.iter().collect();
    sorted.sort_by_key(|(key, _)| *key);
    let capacity = sorted
        .iter()
        .map(|(key, value)| 3 * (key.len() + value.len()) + 2)
        .sum();
    let mut encoded = String::with_capacity(capacity);
    for (index, (key, value)) in sorted.into_iter().enumerate() {
        if index > 0 {
            encoded.push('&');
        }
        escape(key, &mut encoded);
        encoded.push('=');
        escape(value, &mut encoded);
    }
    encoded
}

fn escape(text: &str, out: &mut String) {
    for byte in text.bytes() {
        match byte {
            b' ' => out.push('+'),
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0xf)]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_matches_go() {
        let cases = [
            (
                "a+b&c=d*e~f %ç",
                "contexto=&orgao=63&senha=a%2Bb%26c%3Dd%2Ae~f+%25%C3%A7&usuario=fulano.tal",
            ),
            (
                "senha-de-teste",
                "contexto=&orgao=63&senha=senha-de-teste&usuario=fulano.tal",
            ),
            (
                "!'()*-._~",
                "contexto=&orgao=63&senha=%21%27%28%29%2A-._~&usuario=fulano.tal",
            ),
            (
                "€ /?#[]@$,;:",
                "contexto=&orgao=63&senha=%E2%82%AC+%2F%3F%23%5B%5D%40%24%2C%3B%3A&usuario=fulano.tal",
            ),
            (
                "\u{0}\u{7f}",
                "contexto=&orgao=63&senha=%00%7F&usuario=fulano.tal",
            ),
            (
                "日本",
                "contexto=&orgao=63&senha=%E6%97%A5%E6%9C%AC&usuario=fulano.tal",
            ),
        ];
        for (password, expected) in cases {
            let form = [
                ("usuario", "fulano.tal"),
                ("senha", password),
                ("orgao", "63"),
                ("contexto", ""),
            ];
            assert_eq!(encode(&form), expected, "{password:?}");
        }
    }

    #[test]
    fn query_keys_are_sorted_like_go() {
        let query = [
            ("start", "0"),
            ("limit", "50"),
            ("tipo", "G"),
            ("procedimento", "9001"),
        ];

        assert_eq!(encode(&query), "limit=50&procedimento=9001&start=0&tipo=G");
        assert_eq!(encode(&[]), "");
    }
}
