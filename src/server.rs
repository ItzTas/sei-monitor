use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Utc};
use monitor_hub_plugin::proto::plugin_server::Plugin;
use monitor_hub_plugin::proto::{
    CheckRequest, CheckResponse, DescribeRequest, DescribeResponse, Event, EventKind,
    EventsRequest, EventsResponse, Severity, State, StatusRequest, StatusResponse,
};
use prost_types::value::Kind as ValueKind;
use prost_types::{ListValue, Struct, Timestamp, Value as ProtoValue};
use serde_json::{Map, Value};
use tonic::{Request, Response, Status};
use tracing::{error, warn};

use crate::event::{Event as StoredEvent, Formatter, Kind};
use crate::gotext::capped;
use crate::monitor::{Monitor, MonitorError, State as MonitorState, TriggerError};
use crate::store::StoreError;
use crate::version::VERSION;

const PLUGIN_NAME: &str = "sei";
const DISPLAY_NAME: &str = "SEI";
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;
const STATUS_QUERY_LIMIT: Duration = Duration::from_secs(10);
const DATABASE_UNAVAILABLE: &str = "the database is unavailable";
const HUB_AHEAD: &str =
    "the hub is ahead of this database: reset the hub's position for the sei plugin";
const HELD_INSTRUCTION: &str =
    "nothing is logging in: request a check once the configuration is fixed";
const PAUSED_INSTRUCTION: &str =
    "restart the plugin with the configuration fixed, then request a check";
const CHECK_AFTER_RESTART: &str = ", then request a check";
const FIRST_YEAR: i32 = 1;
const LAST_YEAR: i32 = 9999;

pub struct SeiPlugin {
    monitor: Arc<Monitor>,
    full_detail: bool,
    first_read_boundary: Option<i64>,
    first_read_over: AtomicBool,
    hub_ahead: AtomicBool,
}

impl SeiPlugin {
    pub fn new(monitor: Arc<Monitor>, full_detail: bool) -> SeiPlugin {
        SeiPlugin {
            monitor,
            full_detail,
            first_read_boundary: None,
            first_read_over: AtomicBool::new(false),
            hub_ahead: AtomicBool::new(false),
        }
    }

    pub fn with_first_read_boundary(mut self, boundary: i64) -> SeiPlugin {
        self.first_read_boundary = Some(boundary);
        self
    }

    async fn read_events(&self, after: i64, limit: i64) -> Result<Vec<Event>, Status> {
        let store = self.monitor.store();
        let last_issued = store.last_issued_seq().await.map_err(unavailable)?;
        if self.hub_is_ahead(after, last_issued) {
            return Ok(Vec::new());
        }
        self.monitor.acknowledge(after);
        let state = self.monitor.state();
        if state.account_id == 0 {
            return Err(Status::unavailable(MonitorError::NotReady.to_string()));
        }
        let stored = store
            .events_after(state.account_id, after, limit)
            .await
            .map_err(unavailable)?;
        let formatter = self.formatter(&state);
        let boundary = self.active_boundary();
        Ok(stored
            .into_iter()
            .take_while(|stored| boundary.is_none_or(|boundary| stored.id <= boundary))
            .map(|stored| self.event(&formatter, &stored))
            .collect())
    }

    fn hub_is_ahead(&self, after: i64, last_issued: i64) -> bool {
        let ahead = after > last_issued;
        let was_ahead = self.hub_ahead.swap(ahead, Ordering::SeqCst);
        if ahead && !was_ahead {
            error!(after, last_issued, "{HUB_AHEAD}");
        }
        ahead
    }

    fn active_boundary(&self) -> Option<i64> {
        if self.first_read_over.load(Ordering::SeqCst) {
            return None;
        }
        self.first_read_boundary
    }

    fn formatter(&self, state: &MonitorState) -> Formatter {
        let config = self.monitor.config();
        Formatter {
            zone: config.zone,
            full: self.full_detail,
            selves: vec![state.user.acronym.clone(), config.target.username.clone()],
        }
    }

    fn event(&self, formatter: &Formatter, stored: &StoredEvent) -> Event {
        Event {
            seq: stored.id,
            id: stored.id.to_string(),
            kind: stored.kind.as_str().to_owned(),
            severity: severity(stored.kind).into(),
            occurred_at: [stored.occurred_at, stored.detected_at]
                .into_iter()
                .flatten()
                .find(|at| (FIRST_YEAR..=LAST_YEAR).contains(&at.year()))
                .map(timestamp),
            title: formatter.title(stored),
            body: formatter.body(stored),
            url: process_link(&self.monitor.config().target.sei_url, stored.procedure_id),
            payload: proto_struct(formatter.payload(stored)),
        }
    }

    async fn last_success(&self, account_id: i64) -> Option<Option<DateTime<Utc>>> {
        if account_id == 0 {
            return Some(None);
        }
        let store = self.monitor.store();
        let read = tokio::time::timeout(
            STATUS_QUERY_LIMIT,
            store.last_successful_cycle_run(account_id),
        )
        .await;
        let Ok(read) = read else {
            warn!("reading the last successful cycle took longer than {STATUS_QUERY_LIMIT:?}");
            return None;
        };
        read.map(|run| run.and_then(|run| run.finished_at))
            .inspect_err(|failure| {
                warn!(
                    error = failure as &dyn Error,
                    "could not read the last successful cycle"
                );
            })
            .ok()
    }
}

#[tonic::async_trait]
impl Plugin for SeiPlugin {
    async fn describe(
        &self,
        _: Request<DescribeRequest>,
    ) -> Result<Response<DescribeResponse>, Status> {
        Ok(Response::new(DescribeResponse {
            name: PLUGIN_NAME.to_owned(),
            display_name: DISPLAY_NAME.to_owned(),
            version: VERSION.to_owned(),
            kinds: Kind::ALL
                .iter()
                .map(|kind| EventKind {
                    id: kind.as_str().to_owned(),
                    description: kind.description().to_owned(),
                })
                .collect(),
        }))
    }

    async fn status(&self, _: Request<StatusRequest>) -> Result<Response<StatusResponse>, Status> {
        let state = self.monitor.state();
        let last_success = self.last_success(state.account_id).await;
        let hub_ahead = self.hub_ahead.load(Ordering::SeqCst);
        let (current, message) = state_and_message(&state, last_success.is_some(), hub_ahead);
        Ok(Response::new(StatusResponse {
            state: current.into(),
            message: capped(&message).to_owned(),
            last_check_started_at: state.last_started_at.map(timestamp),
            last_success_at: last_success.flatten().map(timestamp),
            next_check_at: next_check(&state).map(timestamp),
        }))
    }

    async fn check(&self, _: Request<CheckRequest>) -> Result<Response<CheckResponse>, Status> {
        let Err(refusal) = self.monitor.trigger_cycle() else {
            return Ok(Response::new(CheckResponse {
                accepted: true,
                message: String::new(),
            }));
        };
        Ok(Response::new(CheckResponse {
            accepted: false,
            message: refusal_message(&refusal),
        }))
    }

    async fn events(
        &self,
        request: Request<EventsRequest>,
    ) -> Result<Response<EventsResponse>, Status> {
        let EventsRequest { after, limit } = request.into_inner();
        let limit = effective_limit(limit);
        let events = self.read_events(after, limit).await?;
        if i64::try_from(events.len()).unwrap_or(i64::MAX) < limit {
            self.first_read_over.store(true, Ordering::SeqCst);
        }
        Ok(Response::new(EventsResponse { events }))
    }
}

pub fn process_link(sei_url: &str, procedure_id: i64) -> String {
    if procedure_id == 0 {
        return String::new();
    }
    format!(
        "{sei_url}/sei/controlador.php?acao=procedimento_trabalhar&id_procedimento={procedure_id}"
    )
}

fn effective_limit(requested: i32) -> i64 {
    if requested <= 0 {
        return DEFAULT_LIMIT;
    }
    i64::from(requested).min(MAX_LIMIT)
}

fn unavailable(failure: StoreError) -> Status {
    warn!(error = &failure as &dyn Error, "could not read the events");
    Status::unavailable(DATABASE_UNAVAILABLE)
}

fn severity(kind: Kind) -> Severity {
    if kind.urgent() {
        return Severity::Urgent;
    }
    Severity::Info
}

fn refusal_message(refusal: &TriggerError) -> String {
    if *refusal == TriggerError::Paused {
        return format!("{refusal}{CHECK_AFTER_RESTART}");
    }
    refusal.to_string()
}

fn state_and_message(
    state: &MonitorState,
    database_answered: bool,
    hub_ahead: bool,
) -> (State, String) {
    if hub_ahead {
        return (State::Failing, HUB_AHEAD.to_owned());
    }
    if let Some(fatal) = &state.fatal {
        return (State::Failing, fatal.clone());
    }
    if state.running {
        return (State::Checking, String::new());
    }
    if state.paused {
        return (State::Paused, paused_message(state));
    }
    if state.account_id == 0 {
        return (State::Failing, not_reached(state));
    }
    if !database_answered {
        return (State::Failing, DATABASE_UNAVAILABLE.to_owned());
    }
    (State::Idle, String::new())
}

fn paused_message(state: &MonitorState) -> String {
    if state.held {
        return format!("{}; {HELD_INSTRUCTION}", state.paused_reason);
    }
    format!("{}; {PAUSED_INSTRUCTION}", state.paused_reason)
}

fn not_reached(state: &MonitorState) -> String {
    let not_ready = MonitorError::NotReady.to_string();
    let Some(problem) = &state.prepare_error else {
        return not_ready;
    };
    format!("{not_ready}: {problem}")
}

fn next_check(state: &MonitorState) -> Option<DateTime<Utc>> {
    if state.running {
        return None;
    }
    state.next_cycle_at
}

fn timestamp(at: DateTime<Utc>) -> Timestamp {
    Timestamp::from(SystemTime::from(at))
}

fn proto_struct(value: Value) -> Option<Struct> {
    let Value::Object(fields) = value else {
        return None;
    };
    Some(object_struct(fields))
}

fn object_struct(fields: Map<String, Value>) -> Struct {
    Struct {
        fields: fields
            .into_iter()
            .map(|(name, value)| (name, proto_value(value)))
            .collect(),
    }
}

fn proto_value(value: Value) -> ProtoValue {
    let kind = match value {
        Value::Null => ValueKind::NullValue(0),
        Value::Bool(flag) => ValueKind::BoolValue(flag),
        Value::Number(number) => ValueKind::NumberValue(number.as_f64().unwrap_or_default()),
        Value::String(text) => ValueKind::StringValue(text),
        Value::Array(items) => ValueKind::ListValue(ListValue {
            values: items.into_iter().map(proto_value).collect(),
        }),
        Value::Object(fields) => ValueKind::StructValue(object_struct(fields)),
    };
    ProtoValue { kind: Some(kind) }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::process_link;

    #[test]
    fn plugin_json_matches_the_contract() {
        let manifest: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/plugin.json"
        )))
        .expect("plugin.json is JSON");

        assert_eq!(
            manifest,
            json!({
                "protocol_version": 1,
                "name": "sei",
                "command": ["sei-monitor", "plugin"],
                "apk": [],
            })
        );
    }

    #[test]
    fn process_link_points_at_the_process_or_nowhere() {
        assert_eq!(
            process_link("https://sei.rr.gov.br", 9001),
            "https://sei.rr.gov.br/sei/controlador.php?acao=procedimento_trabalhar&id_procedimento=9001"
        );
        assert_eq!(process_link("https://sei.rr.gov.br", 0), "");
    }
}
