use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, Path, Request, State as Extract};
use axum::handler::Handler;
use axum::http::header::{CONTENT_TYPE, USER_AGENT};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter, on};
use futures_util::stream::{self, StreamExt};
use serde_json::json;

use super::answers::{
    activity_list, document_list, failure, login, login_page_body, org_list, process_list, version,
};
use super::gohttp::{clean_path, json, method_not_allowed, plain_error, redirect};
use super::state::{Armed, Canned, Shared, State};
use super::{
    API_PATH, FakeUnit, ORG_ACRONYM, ROUTE_ACTIVITIES, ROUTE_DOCUMENTS, ROUTE_LOGIN,
    ROUTE_LOGIN_PAGE, ROUTE_ORGS, ROUTE_PROCESSES, ROUTE_VERSION,
};

const GET: &str = "GET, HEAD";
const POST: &str = "POST";

pub(super) struct Incoming {
    pub(super) headers: HeaderMap,
    pub(super) query: String,
    pub(super) id: Option<String>,
    pub(super) body: Bytes,
}

impl Incoming {
    async fn read(request: Request) -> Incoming {
        let (mut parts, body) = request.into_parts();
        let id = Path::<String>::from_request_parts(&mut parts, &())
            .await
            .ok()
            .map(|Path(id)| id);
        let body = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap_or_default();
        Incoming {
            query: parts.uri.query().unwrap_or_default().to_owned(),
            headers: parts.headers,
            id,
            body,
        }
    }

    pub(super) fn header(&self, name: &str) -> &[u8] {
        header(&self.headers, name)
    }
}

type ApiHandler = fn(&mut State, &Incoming, &FakeUnit) -> Response;

struct ApiRoute {
    route: &'static str,
    wildcard: &'static str,
    methods: MethodFilter,
    allow: &'static str,
    authenticated: bool,
    handler: ApiHandler,
}

static API_ROUTES: [ApiRoute; 6] = [
    ApiRoute {
        route: ROUTE_VERSION,
        wildcard: "",
        methods: MethodFilter::GET,
        allow: GET,
        authenticated: true,
        handler: version,
    },
    ApiRoute {
        route: ROUTE_LOGIN,
        wildcard: "",
        methods: MethodFilter::POST,
        allow: POST,
        authenticated: false,
        handler: login,
    },
    ApiRoute {
        route: ROUTE_ORGS,
        wildcard: "",
        methods: MethodFilter::GET,
        allow: GET,
        authenticated: false,
        handler: org_list,
    },
    ApiRoute {
        route: ROUTE_PROCESSES,
        wildcard: "",
        methods: MethodFilter::GET,
        allow: GET,
        authenticated: true,
        handler: process_list,
    },
    ApiRoute {
        route: ROUTE_ACTIVITIES,
        wildcard: "",
        methods: MethodFilter::GET,
        allow: GET,
        authenticated: true,
        handler: activity_list,
    },
    ApiRoute {
        route: ROUTE_DOCUMENTS,
        wildcard: "/{id}",
        methods: MethodFilter::GET,
        allow: GET,
        authenticated: true,
        handler: document_list,
    },
];

struct Stall {
    content_type: &'static str,
    first_chunk: &'static [u8],
}

const API_STALL: Stall = Stall {
    content_type: "application/json",
    first_chunk: br#"{"sucesso":true,"data":["#,
};

const PAGE_STALL: Stall = Stall {
    content_type: "text/html; charset=iso-8859-1",
    first_chunk: b"<!DOCTYPE html><html><head>",
};

pub(super) fn router(shared: Arc<Shared>) -> Router {
    let pages = Router::new()
        .route("/sei/", endpoint(MethodFilter::GET, GET, redirect_to_login))
        .route(
            ROUTE_LOGIN_PAGE,
            endpoint(MethodFilter::GET, GET, login_page),
        );
    API_ROUTES
        .iter()
        .fold(pages, |router, api| {
            let path = format!("{API_PATH}{}{}", api.route, api.wildcard);
            let serve = move |Extract(shared): Extract<Arc<Shared>>, request: Request| {
                serve_api(shared, api, request)
            };
            router.route(&path, endpoint(api.methods, api.allow, serve))
        })
        .fallback(unrouted)
        .with_state(shared)
}

fn endpoint<H, T>(
    methods: MethodFilter,
    allow: &'static str,
    handler: H,
) -> MethodRouter<Arc<Shared>>
where
    H: Handler<T, Arc<Shared>>,
    T: 'static,
{
    on(methods, handler).fallback(move || async move { method_not_allowed(allow) })
}

async fn serve_api(shared: Arc<Shared>, api: &'static ApiRoute, request: Request) -> Response {
    if clean_path(request.uri().path()) != request.uri().path() {
        return unrouted(request.method().clone(), request.uri().clone()).await;
    }
    let incoming = Incoming::read(request).await;
    let mut state = shared.lock();
    state.record_call(api.route, user_agent(&incoming.headers));
    if let Some(armed) = state.take_armed(api.route) {
        return armed_answer(armed, &API_STALL);
    }
    api_answer(&mut state, api, &incoming)
}

fn api_answer(state: &mut State, api: &ApiRoute, incoming: &Incoming) -> Response {
    if state.inactive {
        let inactive = json!({"sucesso": false, "mensagem": "Módulo inativo.", "exception": null});
        return json(StatusCode::UNAUTHORIZED, inactive);
    }
    if let Some(status) = state.pop_failure(api.route) {
        return simulated_failure(status);
    }
    if !api.authenticated {
        return (api.handler)(state, incoming, &FakeUnit::default());
    }
    match authenticate(state, incoming) {
        Ok(unit) => (api.handler)(state, incoming, &unit),
        Err((status, message)) => json(status, failure(message)),
    }
}

fn authenticate(
    state: &State,
    incoming: &Incoming,
) -> Result<FakeUnit, (StatusCode, &'static str)> {
    let token = incoming.header("token");
    if token.is_empty() {
        return Err((StatusCode::UNAUTHORIZED, "Acesso negado!"));
    }
    let known = std::str::from_utf8(token).is_ok_and(|token| state.sessions.contains(token));
    if !known {
        return Err((StatusCode::FORBIDDEN, "Token inválido!"));
    }
    let units = &state.account.units;
    let wanted = incoming.header("unidade");
    if wanted.is_empty() {
        return Ok(units[0].clone());
    }
    units
        .iter()
        .find(|unit| unit.id.to_string().as_bytes() == wanted)
        .cloned()
        .ok_or((StatusCode::OK, "Unidade não permitida para o usuário."))
}

fn simulated_failure(status: u16) -> Response {
    let code = StatusCode::from_u16(status).expect("a status checked by fail_next");
    if status < 500 {
        return json(code, failure("Falha simulada pelo seitest."));
    }
    let html = [(CONTENT_TYPE, "text/html; charset=iso-8859-1")];
    (
        code,
        html,
        "<html><body><h1>Erro interno</h1></body></html>",
    )
        .into_response()
}

fn armed_answer(armed: Armed, stall: &Stall) -> Response {
    match armed {
        Armed::Canned(canned) => canned_answer(canned),
        Armed::Stall => stalled_answer(stall),
    }
}

fn canned_answer(canned: Canned) -> Response {
    let mut response = Response::new(Body::from(canned.body));
    *response.status_mut() = canned.status;
    for (name, value) in canned.headers {
        response.headers_mut().append(name, value);
    }
    response
}

fn stalled_answer(stall: &Stall) -> Response {
    let first = stream::iter([Ok::<_, Infallible>(Bytes::from_static(stall.first_chunk))]);
    let body = Body::from_stream(first.chain(stream::pending()));
    ([(CONTENT_TYPE, stall.content_type)], body).into_response()
}

async fn redirect_to_login(method: Method) -> Response {
    let location =
        format!("{ROUTE_LOGIN_PAGE}?sigla_orgao_sistema={ORG_ACRONYM}&sigla_sistema=SEI");
    redirect(&method, StatusCode::FOUND, &location)
}

async fn login_page(Extract(shared): Extract<Arc<Shared>>, headers: HeaderMap) -> Response {
    let mut state = shared.lock();
    state.record_call(ROUTE_LOGIN_PAGE, user_agent(&headers));
    if let Some(armed) = state.take_armed(ROUTE_LOGIN_PAGE) {
        return armed_answer(armed, &PAGE_STALL);
    }
    let latin1 = [(CONTENT_TYPE, "text/html; charset=iso-8859-1")];
    (latin1, login_page_body()).into_response()
}

async fn unrouted(method: Method, uri: Uri) -> Response {
    let path = uri.path();
    let clean = clean_path(path);
    let query = uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let reads = method == Method::GET || method == Method::HEAD;
    if clean == "/sei" && reads {
        return redirect(
            &method,
            StatusCode::TEMPORARY_REDIRECT,
            &format!("/sei/{query}"),
        );
    }
    if clean != path {
        return redirect(
            &method,
            StatusCode::TEMPORARY_REDIRECT,
            &format!("{clean}{query}"),
        );
    }
    if clean == "/sei" {
        return method_not_allowed(GET);
    }
    plain_error(StatusCode::NOT_FOUND, "404 page not found")
}

fn user_agent(headers: &HeaderMap) -> String {
    String::from_utf8_lossy(header(headers, USER_AGENT.as_str())).into_owned()
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a [u8] {
    headers.get(name).map_or(&[], |value| value.as_bytes())
}
