use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use chrono::{Datelike, Timelike};
use chrono_tz::Tz;
use serde_json::{Value, json};

use super::gohttp::{Values, json, post_form, query};
use super::handlers::Incoming;
use super::state::{Account, State};
use super::{
    FakeActivity, FakeDocument, FakeProcess, FakeUnit, MODULE_VERSION, ORG_ACRONYM, SEI_VERSION,
    default_orgs,
};

const DEFAULT_LIMIT: i64 = 10;
const ORG_PAGE_SIZE: usize = 10;

pub(super) fn login(state: &mut State, incoming: &Incoming, _: &FakeUnit) -> Response {
    let form = post_form(
        incoming.header(CONTENT_TYPE.as_str()),
        &incoming.query,
        &incoming.body,
    );
    let Some(form) = form else {
        return json(StatusCode::OK, failure("Requisição inválida."));
    };
    if !accepts(&state.account, &form) {
        state.failed_logins += 1;
        return json(StatusCode::OK, failure("Usuário ou Senha Inválida."));
    }
    let token = state.open_session();
    let data = login_data(&state.account, &token);
    json(StatusCode::OK, success(Some(data), None))
}

fn accepts(account: &Account, form: &Values) -> bool {
    form.get("usuario") == account.username.as_bytes()
        && form.get("senha") == account.password.as_bytes()
        && form.get("orgao") == account.org_id.to_string().as_bytes()
}

fn login_data(account: &Account, token: &str) -> Value {
    let units: Vec<Value> = account
        .units
        .iter()
        .map(
            |unit| json!({"id": id(unit.id), "sigla": unit.acronym, "descricao": unit.description}),
        )
        .collect();
    json!({
        "loginData": {
            "IdSistema": "100000100",
            "IdContexto": null,
            "IdUsuario": id(account.user.id),
            "IdUnidadeAtual": id(account.units[0].id),
            "sigla": account.user.acronym,
            "nome": account.user.name,
        },
        "perfis": [{"IdPerfil": "100000938", "Nome": "Básico"}],
        "unidades": units,
        "identificador": format!("{}@{ORG_ACRONYM}", account.user.acronym),
        "token": token,
    })
}

pub(super) fn version(_: &mut State, _: &Incoming, _: &FakeUnit) -> Response {
    let versions = json!({"sei": SEI_VERSION, "wssei": MODULE_VERSION});
    json(StatusCode::OK, success(Some(versions), None))
}

pub(super) fn org_list(_: &mut State, _: &Incoming, _: &FakeUnit) -> Response {
    let mut orgs = default_orgs();
    orgs.sort_by(|a, b| a.1.cmp(b.1));
    let items = orgs
        .iter()
        .take(ORG_PAGE_SIZE)
        .map(|(org_id, acronym, description)| {
            json!({"id": id(*org_id), "sigla": acronym, "descricao": description})
        })
        .collect();
    listing(items, orgs.len())
}

pub(super) fn process_list(state: &mut State, incoming: &Incoming, unit: &FakeUnit) -> Response {
    let query = query(&incoming.query);
    let (page, limit) = pagination(&query);
    let kind = query.get("tipo");
    let processes: Vec<&FakeProcess> = state
        .processes
        .get(&unit.id)
        .into_iter()
        .flatten()
        .filter(|process| listed(kind, process))
        .collect();
    let mut carried = Value::Null;
    let items = paginate(&processes, page, limit)
        .iter()
        .map(|process| process_item(process, unit, &mut carried))
        .collect();
    listing(items, processes.len())
}

fn listed(kind: &[u8], process: &FakeProcess) -> bool {
    let hidden = (kind == b"G" && !process.generated) || (kind == b"R" && process.generated);
    !hidden
}

fn process_item(process: &FakeProcess, unit: &FakeUnit, carried: &mut Value) -> Value {
    let return_flag = match process.return_date.as_str() {
        "" => "N",
        date => {
            *carried = json!({"dataProgramada": date, "unidade": "SEDEMO/GAB"});
            "S"
        }
    };
    let assignee = process.assignee.as_ref().map(|user| {
        json!({
            "idAtividade": "700",
            "idUsuario": id(user.id),
            "sigla": user.acronym,
            "nome": user.name,
            "nomeformatado": format!("{} - {}", user.acronym, user.name),
        })
    });
    let (origin, generating_unit) = origin(process, unit);
    json!({
        "id": id(process.id),
        "status": "P",
        "seiNumMaxDocsPasta": "20",
        "atributos": {
            "idProcedimento": id(process.id),
            "idProtocolo": id(process.id),
            "numero": process.number,
            "tipoProcesso": process.process_type,
            "descricao": process.description,
            "usuarioAtribuido": assignee,
            "unidade": {"idUnidade": id(generating_unit.id), "sigla": generating_unit.acronym},
            "ciencias": [],
            "marcador": [],
            "dadosAbertura": [],
            "anotacoes": [],
            "status": {
                "documentoSigiloso": "",
                "documentoRestrito": "N",
                "documentoNovo": "N",
                "retornoProgramado": return_flag,
                "retornoData": carried.clone(),
                "retornoAtrasado": if process.return_overdue { "S" } else { "N" },
                "processoAberto": "S",
                "processoSobrestado": "N",
                "tipo": null,
                "processoGeradoRecebido": origin,
            },
        },
    })
}

fn origin(process: &FakeProcess, unit: &FakeUnit) -> (&'static str, FakeUnit) {
    if process.generated {
        return ("G", unit.clone());
    }
    let protocol = FakeUnit {
        id: 1500,
        acronym: "SEDEMO/PROTOCOLO".to_owned(),
        description: String::new(),
    };
    let generating_unit = process
        .generating_unit
        .clone()
        .filter(|generating_unit| generating_unit.id != 0)
        .unwrap_or(protocol);
    ("R", generating_unit)
}

pub(super) fn activity_list(state: &mut State, incoming: &Incoming, _: &FakeUnit) -> Response {
    let query = query(&incoming.query);
    let Some(procedure_id) = parse_int(query.get("procedimento")) else {
        return json(
            StatusCode::OK,
            failure("O procedimento deve ser informado!"),
        );
    };
    let (page, limit) = pagination(&query);
    let newest_first: Vec<&FakeActivity> = state
        .activities
        .get(&procedure_id)
        .into_iter()
        .flatten()
        .rev()
        .collect();
    let items = paginate(&newest_first, page, limit)
        .iter()
        .map(|activity| activity_item(activity, procedure_id, state.zone))
        .collect();
    listing(items, newest_first.len())
}

fn activity_item(activity: &FakeActivity, procedure_id: i64, zone: Tz) -> Value {
    let local = activity.at.with_timezone(&zone);
    let user = Some(&activity.user).filter(|user| !user.is_empty());
    json!({
        "id": id(activity.id),
        "atributos": {
            "idProcesso": id(procedure_id),
            "usuario": user,
            "data": format!("{:02}/{:02}/{:04}", local.day(), local.month(), local.year()),
            "hora": format!("{:02}:{:02}:{:02}", local.hour(), local.minute(), local.second()),
            "unidade": activity.unit,
            "informacao": activity.text,
        },
    })
}

pub(super) fn document_list(state: &mut State, incoming: &Incoming, _: &FakeUnit) -> Response {
    let procedure_id = incoming.id.as_deref().and_then(|id| id.parse::<i64>().ok());
    let Some(procedure_id) = procedure_id else {
        return json(
            StatusCode::OK,
            failure("O procedimento deve ser informado!"),
        );
    };
    let query = query(&incoming.query);
    let (page, limit) = pagination(&query);
    let documents = state
        .documents
        .get(&procedure_id)
        .map_or(&[][..], Vec::as_slice);
    let items = paginate(documents, page, limit)
        .iter()
        .map(|document| document_item(document, procedure_id))
        .collect();
    listing(items, documents.len())
}

fn document_item(document: &FakeDocument, procedure_id: i64) -> Value {
    json!({
        "id": id(document.id),
        "atributos": {
            "idProcedimento": id(procedure_id),
            "idProtocolo": id(document.id),
            "protocoloFormatado": document.protocol,
            "nome": null,
            "titulo": document.number,
            "tipo": document.document_type,
            "tipoDocumento": "I",
            "mimeType": "html",
            "nomeComposto": document.label,
            "status": {"documentoAssinado": "S"},
        },
    })
}

pub(super) fn login_page_body() -> Vec<u8> {
    let options: String = default_orgs()
        .iter()
        .map(|(org_id, acronym, _)| format!(r#"<option value="{org_id}">{acronym}</option>"#))
        .collect();
    let page = format!(
        concat!(
            r#"<!DOCTYPE html><html><head><meta http-equiv="Content-Type" content="text/html; charset=iso-8859-1">"#,
            r#"<title>SIP - Autenticação de Usuário</title></head><body><form id="frmLogin">"#,
            r#"<select id="selOrgao" name="selOrgao" class="infraSelect"><option value="null">&nbsp;</option>{}</select></form></body></html>"#,
        ),
        options
    );
    let (bytes, _, _) = encoding_rs::WINDOWS_1252.encode(&page);
    bytes.into_owned()
}

fn pagination(query: &Values) -> (i64, i64) {
    let limit = parse_int(query.get("limit"))
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_LIMIT);
    let page = parse_int(query.get("start"))
        .filter(|page| *page >= 0)
        .unwrap_or(0);
    (page, limit)
}

fn paginate<T>(items: &[T], page: i64, limit: i64) -> &[T] {
    let start = page
        .checked_mul(limit)
        .and_then(|start| usize::try_from(start).ok())
        .unwrap_or(usize::MAX);
    if start >= items.len() {
        return &[];
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    &items[start..start.saturating_add(limit).min(items.len())]
}

fn parse_int(text: &[u8]) -> Option<i64> {
    std::str::from_utf8(text).ok()?.parse().ok()
}

fn listing(items: Vec<Value>, total: usize) -> Response {
    let data = (!items.is_empty()).then_some(Value::Array(items));
    json(StatusCode::OK, success(data, Some(total)))
}

fn success(data: Option<Value>, total: Option<usize>) -> Value {
    let mut body = json!({"sucesso": true});
    if let Some(data) = data {
        body["data"] = data;
    }
    if let Some(total) = total {
        body["total"] = Value::String(total.to_string());
    }
    body
}

pub(super) fn failure(message: &str) -> Value {
    json!({"sucesso": false, "mensagem": message, "exception": "Exceção salva no SEI"})
}

fn id(value: i64) -> String {
    value.to_string()
}
