use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::io::Write;

use anyhow::anyhow;
use serde_json::{Map, Value};
use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

use super::{open, units_error};
use crate::config::{SeiConfig, Source, path_escape};
use crate::gotext::{atoi, to_lower};
use crate::sei::{Client, ErrorKind, SeiError};

const PROCESSES: &str = "/processo/listar";
const ACTIVITIES: &str = "/atividade/listar";
const DOCUMENTS: &str = "/documento/listar";
const SHAPE_LIMIT: usize = 24;
const SAMPLE: [(&str, &str); 2] = [("limit", "10"), ("start", "0")];

type Item = Map<String, Value>;
type Fields<'a> = [(&'a str, &'a [&'a str])];

struct Listing {
    items: Vec<Item>,
    total: Option<Value>,
    has_data: bool,
}

struct Endpoint {
    path: String,
    query: Vec<(&'static str, String)>,
}

impl Endpoint {
    fn at(path: impl Into<String>) -> Endpoint {
        Endpoint {
            path: path.into(),
            query: Vec::new(),
        }
    }
}

struct Diagnosis<'a> {
    client: &'a Client,
    unit: i64,
    out: &'a mut dyn Write,
}

pub async fn diagnose(source: &Source, out: &mut dyn Write) -> anyhow::Result<()> {
    let config = SeiConfig::load(source)?;
    let connection = open(&config).await?;
    let session = connection.client.login().await?;
    let watched = session
        .select_units(&config.units, config.all_units)
        .map_err(units_error)?;
    let version = connection.client.version().await?;
    let unit = watched
        .first()
        .ok_or_else(|| anyhow!("SEI_UNITS: the user has no unit to check with"))?
        .id;

    writeln!(out, "sei-monitor diagnose")?;
    writeln!(
        out,
        "SEI {}, mod-wssei {}; units: {} available, {} watched (checks use the first one)",
        version.sei,
        version.module,
        session.units.len(),
        watched.len()
    )?;
    let mut diagnosis = Diagnosis {
        client: &connection.client,
        unit,
        out,
    };
    diagnosis.run().await?;
    Ok(())
}

impl Diagnosis<'_> {
    async fn run(&mut self) -> anyhow::Result<()> {
        let all = [("limit", "100"), ("start", "0")];
        let processes = match self.list(&Endpoint::at(PROCESSES), &all).await? {
            Ok(processes) => processes,
            Err(failure) => {
                let reason = describe_failure(&[failure]);
                return Ok(writeln!(
                    self.out,
                    "\ncould not list the processes: {reason}"
                )?);
            }
        };
        let procedure = scalar(
            processes
                .items
                .first()
                .and_then(|item| field(item, &["atributos", "idProcedimento"])),
        );
        let activities = procedure.as_ref().map(|procedure| Endpoint {
            path: ACTIVITIES.to_owned(),
            query: vec![("procedimento", procedure.clone())],
        });
        let documents = procedure
            .map(|procedure| Endpoint::at(format!("{DOCUMENTS}/{}", path_escape(&procedure))));
        let listing = Endpoint::at(PROCESSES);

        self.section("[1] Pagination with limit=2")?;
        for (name, endpoint) in [
            (PROCESSES, Some(&listing)),
            (ACTIVITIES, activities.as_ref()),
            (DOCUMENTS, documents.as_ref()),
        ] {
            self.pagination(name, endpoint).await?;
        }

        self.section("[2] Coverage of /processo/listar by processoGeradoRecebido")?;
        for kind in ["", "R", "G"] {
            self.coverage(kind).await?;
        }

        self.section("[3] Scheduled returns (first 100 processes)")?;
        self.returns(&processes.items)?;

        self.section("[4] Fields that may hold a link")?;
        self.links(&processes.items)?;

        self.section("[5] Field types")?;
        self.types(&processes, activities.as_ref(), documents.as_ref())
            .await?;

        self.section("[6] Order")?;
        for (name, endpoint) in [
            ("activities", activities.as_ref()),
            ("documents", documents.as_ref()),
        ] {
            self.order(name, endpoint).await?;
        }
        Ok(())
    }

    fn section(&mut self, title: &str) -> anyhow::Result<()> {
        Ok(writeln!(self.out, "\n{title}")?)
    }

    fn line(&mut self, text: impl Display) -> anyhow::Result<()> {
        Ok(writeln!(self.out, "    {text}")?)
    }

    async fn list(
        &self,
        endpoint: &Endpoint,
        query: &[(&str, &str)],
    ) -> anyhow::Result<Result<Listing, SeiError>> {
        let mut values: Vec<(&str, String)> = endpoint
            .query
            .iter()
            .filter(|(key, _)| !query.iter().any(|(given, _)| given == key))
            .map(|(key, value)| (*key, value.clone()))
            .collect();
        values.extend(query.iter().map(|(key, value)| (*key, (*value).to_owned())));
        let page = match self.client.raw(self.unit, &endpoint.path, &values).await {
            Ok(page) => page,
            Err(failure) => return stop_at_a_rejected_login(failure),
        };
        let items = match &page.data {
            Some(Value::Array(list)) => list
                .iter()
                .map(|value| value.as_object().cloned().unwrap_or_default())
                .collect(),
            _ => Vec::new(),
        };
        Ok(Ok(Listing {
            items,
            has_data: page.data.is_some(),
            total: page.total,
        }))
    }

    async fn pagination(&mut self, name: &str, endpoint: Option<&Endpoint>) -> anyhow::Result<()> {
        let Some(endpoint) = endpoint else {
            return self.line(format_args!("{name}: no process to check with"));
        };
        let first = self
            .list(endpoint, &[("limit", "2"), ("start", "0")])
            .await?;
        let second = self
            .list(endpoint, &[("limit", "2"), ("start", "1")])
            .await?;
        let (first, second) = match (first, second) {
            (Ok(first), Ok(second)) => (ids(&first.items), ids(&second.items)),
            (first, second) => {
                let failures: Vec<SeiError> =
                    [first.err(), second.err()].into_iter().flatten().collect();
                return self.line(format_args!("{name}: {}", describe_failure(&failures)));
            }
        };
        let shared = second.iter().filter(|id| first.contains(id)).count();
        self.line(format_args!(
            "{name}: page 0 has {} ids, page 1 has {}, {shared} in common -> {}",
            first.len(),
            second.len(),
            page_verdict(first.len(), second.len(), shared)
        ))
    }

    async fn coverage(&mut self, kind: &str) -> anyhow::Result<()> {
        let label = match kind {
            "" => "default call".to_owned(),
            kind => format!("tipo={kind}"),
        };
        let query: Vec<(&str, &str)> = [("limit", "100"), ("start", "0"), ("tipo", kind)]
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .collect();
        let listing = match self.list(&Endpoint::at(PROCESSES), &query).await? {
            Ok(listing) => listing,
            Err(failure) => {
                return self.line(format_args!("{label}: {}", describe_failure(&[failure])));
            }
        };
        let origins: Vec<String> = listing
            .items
            .iter()
            .map(|item| {
                scalar(field(
                    item,
                    &["atributos", "status", "processoGeradoRecebido"],
                ))
                .unwrap_or_default()
            })
            .collect();
        let generated = origins.iter().filter(|origin| *origin == "G").count();
        let received = origins.iter().filter(|origin| *origin == "R").count();
        self.line(format_args!(
            "{:<13} total {}; G={generated} R={received} other={}",
            format!("{label}:"),
            count(listing.total.as_ref()),
            origins.len() - generated - received
        ))
    }

    fn returns(&mut self, processes: &[Item]) -> anyhow::Result<()> {
        let (mut scheduled, mut carried) = (0, 0);
        let (mut kinds, mut shapes) = (BTreeMap::new(), BTreeMap::new());
        for item in processes {
            let flag = scalar(field(item, &["atributos", "status", "retornoProgramado"]));
            let data = field(item, &["atributos", "status", "retornoData"]);
            if flag.as_deref() != Some("S") {
                if !matches!(kind(data), "null" | "missing") {
                    carried += 1;
                }
                continue;
            }
            scheduled += 1;
            *kinds.entry(describe_kind(data)).or_insert(0) += 1;
            let date = data
                .and_then(Value::as_object)
                .and_then(|object| object.get("dataProgramada"));
            if let Some(date) = scalar(date) {
                *shapes.entry(shape(&date)).or_insert(0) += 1;
            }
        }
        self.line(format_args!(
            "retornoProgramado=S: {scheduled}; retornoData: {}",
            tally(&kinds)
        ))?;
        self.line(format_args!("dataProgramada: {}", tally(&shapes)))?;
        self.line(format_args!(
            "retornoProgramado=N with retornoData filled: {carried} (sei-monitor ignores those)"
        ))
    }

    fn links(&mut self, processes: &[Item]) -> anyhow::Result<()> {
        let mut found = BTreeSet::new();
        for item in processes {
            collect_link_keys("", item, &mut found);
        }
        let keys: Vec<String> = found.into_iter().collect();
        let listed = if keys.is_empty() {
            "none in the process items".to_owned()
        } else {
            keys.join(", ")
        };
        self.line(listed)?;
        self.line("a stable link to a process still needs a manual check in the browser")
    }

    async fn types(
        &mut self,
        processes: &Listing,
        activities: Option<&Endpoint>,
        documents: Option<&Endpoint>,
    ) -> anyhow::Result<()> {
        let process_fields: &Fields = &[
            ("id", &["id"]),
            ("idProcedimento", &["atributos", "idProcedimento"]),
            ("usuarioAtribuido", &["atributos", "usuarioAtribuido"]),
        ];
        self.line(format_args!(
            "process: {}; total {}",
            field_kinds(&processes.items, process_fields),
            kind(processes.total.as_ref())
        ))?;
        let activity_fields: &Fields = &[
            ("id", &["id"]),
            ("data", &["atributos", "data"]),
            ("hora", &["atributos", "hora"]),
            ("usuario", &["atributos", "usuario"]),
        ];
        let document_fields: &Fields = &[
            ("id", &["id"]),
            ("nomeComposto", &["atributos", "nomeComposto"]),
            ("protocoloFormatado", &["atributos", "protocoloFormatado"]),
        ];
        for (name, endpoint, fields) in [
            ("activity", activities, activity_fields),
            ("document", documents, document_fields),
        ] {
            let Some(endpoint) = endpoint else {
                continue;
            };
            let Ok(sample) = self.list(endpoint, &SAMPLE).await? else {
                continue;
            };
            self.line(format_args!(
                "{name}: {}",
                field_kinds(&sample.items, fields)
            ))?;
        }
        let beyond = [("limit", "1"), ("start", "100000")];
        let empty = match self.list(&Endpoint::at(PROCESSES), &beyond).await? {
            Ok(empty) => empty,
            Err(failure) => {
                return self.line(format_args!("empty page: {}", describe_failure(&[failure])));
            }
        };
        let data_key = if empty.has_data { "present" } else { "absent" };
        self.line(format_args!(
            "empty page: data key {data_key}, total {}",
            kind(empty.total.as_ref())
        ))
    }

    async fn order(&mut self, name: &str, endpoint: Option<&Endpoint>) -> anyhow::Result<()> {
        let Some(endpoint) = endpoint else {
            return self.line(format_args!("{name}: no process to check with"));
        };
        let verdict = match self.list(endpoint, &SAMPLE).await? {
            Ok(listing) => sequence(&ids(&listing.items)).to_owned(),
            Err(failure) => describe_failure(&[failure]),
        };
        self.line(format_args!("{name}: {verdict}"))
    }
}

fn stop_at_a_rejected_login(failure: SeiError) -> anyhow::Result<Result<Listing, SeiError>> {
    if failure.kind() == Some(ErrorKind::InvalidCredentials) {
        return Err(failure.into());
    }
    Ok(Err(failure))
}

fn page_verdict(first: usize, second: usize, shared: usize) -> &'static str {
    if first < 2 || second == 0 {
        return "not enough items to tell";
    }
    if shared > 0 {
        return "start looks like an offset";
    }
    "start is a page number"
}

fn describe_failure(failures: &[SeiError]) -> String {
    if failures.iter().any(SeiError::timed_out) {
        return "the call timed out".to_owned();
    }
    let reasons = [
        (ErrorKind::Unauthorized, "SEI rejected the session token"),
        (ErrorKind::Server, "SEI answered with an internal error"),
        (
            ErrorKind::UnexpectedResponse,
            "SEI answered outside the API format",
        ),
        (ErrorKind::Redirect, "SEI answered with a redirect"),
    ];
    for (kind, text) in reasons {
        if failures.iter().any(|failure| failure.kind() == Some(kind)) {
            return text.to_owned();
        }
    }
    let refused = failures.iter().find_map(|failure| match failure {
        SeiError::Api(error) => Some(error.status),
        _ => None,
    });
    if let Some(status) = refused {
        return format!("SEI refused the call (HTTP {status})");
    }
    if failures
        .iter()
        .any(|failure| matches!(failure, SeiError::Transport { .. }))
    {
        return "network error".to_owned();
    }
    "the call failed".to_owned()
}

fn field<'a>(item: &'a Item, path: &[&str]) -> Option<&'a Value> {
    let (last, parents) = path.split_last()?;
    let mut current = item;
    for key in parents {
        current = current.get(*key)?.as_object()?;
    }
    current.get(*last)
}

fn kind(value: Option<&Value>) -> &'static str {
    match value {
        None => "missing",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "list",
        Some(Value::Object(_)) => "object",
    }
}

fn describe_kind(value: Option<&Value>) -> String {
    let Some(Value::Object(object)) = value else {
        return kind(value).to_owned();
    };
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    format!("object{{{}}}", keys.join(","))
}

fn scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Null => Some(String::new()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn ids(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| scalar(item.get("id")))
        .collect()
}

fn field_kinds(items: &[Item], fields: &Fields) -> String {
    fields
        .iter()
        .map(|(name, path)| {
            let mut kinds = BTreeMap::new();
            for item in items {
                *kinds.entry(kind(field(item, path)).to_owned()).or_insert(0) += 1;
            }
            format!("{name} {}", tally(&kinds))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn tally(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "none".to_owned();
    }
    counts
        .iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn shape(text: &str) -> String {
    text.chars()
        .take(SHAPE_LIMIT)
        .map(|c| {
            if c.general_category() == GeneralCategory::DecimalNumber {
                return '9';
            }
            if c.general_category_group() == GeneralCategoryGroup::Letter {
                return 'A';
            }
            c
        })
        .collect()
}

fn count(total: Option<&Value>) -> String {
    scalar(total)
        .filter(|text| atoi(text).is_some())
        .unwrap_or_else(|| kind(total).to_owned())
}

fn sequence(ids: &[String]) -> &'static str {
    let Some(numbers) = ids.iter().map(|id| atoi(id)).collect::<Option<Vec<i64>>>() else {
        return "ids are not numbers";
    };
    if numbers.len() < 2 {
        return "too few items to tell";
    }
    let descending = numbers.windows(2).all(|pair| pair[0] >= pair[1]);
    let ascending = numbers.windows(2).all(|pair| pair[0] <= pair[1]);
    let verdicts = [
        ((true, false), "newest first (ids going down)"),
        ((false, true), "oldest first (ids going up)"),
    ];
    verdicts
        .into_iter()
        .find(|(order, _)| *order == (descending, ascending))
        .map_or("mixed order", |(_, verdict)| verdict)
}

fn collect_link_keys(prefix: &str, item: &Item, found: &mut BTreeSet<String>) {
    const MARKERS: [&str; 4] = ["link", "url", "hash", "href"];
    for (key, value) in item {
        let joined = format!("{prefix}.{}", shape(key));
        let path = joined.strip_prefix('.').unwrap_or(&joined).to_owned();
        let lower = to_lower(key);
        if MARKERS.iter().any(|marker| lower.contains(marker)) {
            found.insert(path.clone());
        }
        if let Value::Object(nested) = value {
            collect_link_keys(&path, nested, found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sei::ApiError;

    fn api(kind: ErrorKind, status: u16) -> SeiError {
        SeiError::Api(ApiError {
            route: PROCESSES.to_owned(),
            status,
            message: String::new(),
            kind,
        })
    }

    fn transport(timed_out: bool) -> SeiError {
        SeiError::Transport {
            context: PROCESSES.to_owned(),
            detail: "connection reset".to_owned(),
            timed_out,
        }
    }

    #[test]
    fn failures_are_described_like_go() {
        let body_timeout = SeiError::Decode {
            route: PROCESSES.to_owned(),
            detail: "operation timed out".to_owned(),
            timed_out: true,
        };
        let cases = [
            (vec![body_timeout], "the call timed out"),
            (
                vec![api(ErrorKind::Server, 500), transport(true)],
                "the call timed out",
            ),
            (
                vec![api(ErrorKind::Unauthorized, 403)],
                "SEI rejected the session token",
            ),
            (
                vec![transport(false), api(ErrorKind::Server, 502)],
                "SEI answered with an internal error",
            ),
            (
                vec![api(ErrorKind::UnexpectedResponse, 404)],
                "SEI answered outside the API format",
            ),
            (
                vec![api(ErrorKind::Redirect, 302)],
                "SEI answered with a redirect",
            ),
            (
                vec![
                    api(ErrorKind::Refused, 200),
                    api(ErrorKind::InvalidCredentials, 401),
                ],
                "SEI refused the call (HTTP 200)",
            ),
            (vec![transport(false)], "network error"),
            (vec![SeiError::NoToken], "the call failed"),
        ];
        for (failures, described) in cases {
            assert_eq!(describe_failure(&failures), described, "{failures:?}");
        }
    }
}
