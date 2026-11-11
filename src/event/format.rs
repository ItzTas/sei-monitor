use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value};

use super::view::{View, ViewDetail};
use super::{Event, Kind, Payload};
use crate::gotext::{equal_fold, fields, trim};
use crate::timezone::{days_until, format_local};

const LABEL_LIMIT: usize = 40;
const SUMMARY_LIMIT: usize = 80;
const PAYLOAD_KEYS: [&str; 4] = ["procedure_id", "process_number", "unit", "detail"];

#[derive(Clone)]
pub struct Formatter {
    pub zone: Tz,
    pub full: bool,
    pub selves: Vec<String>,
}

impl Formatter {
    pub fn title(&self, event: &Event) -> String {
        let p = &event.payload;
        match event.kind {
            Kind::ProcessEntered => entered_title(p),
            Kind::ProcessLeft => format!("Process {} left {}", p.process_number, p.unit),
            Kind::ActivityAdded => self.activity_title(event),
            Kind::DocumentAdded => format!("New document on process {}", p.process_number),
            Kind::AssignmentChanged => self.assignment_title(p),
            Kind::ReturnDueSoon => self.return_due_title(event),
            Kind::ReturnOverdue => {
                format!(
                    "Scheduled return of process {} is overdue",
                    p.process_number
                )
            }
            Kind::AccountLoginFailed => "sei-monitor could not log in to SEI".to_string(),
        }
    }

    pub fn details(&self, event: &Event) -> Vec<String> {
        let p = &event.payload;
        if event.kind == Kind::AccountLoginFailed {
            return vec![
                format!("Reason: {}", p.reason),
                "Monitoring is paused until sei-monitor restarts with the configuration fixed."
                    .to_string(),
            ];
        }
        let mut lines = vec![format!("When: {}", self.time(event.occurred_at))];
        if !p.unit.is_empty() && !matches!(event.kind, Kind::ProcessEntered | Kind::ProcessLeft) {
            lines.push(format!("Unit: {}", p.unit));
        }
        if !self.full {
            return lines;
        }
        let optional = [
            ("Type", p.process_type.clone()),
            ("Description", p.description.clone()),
            (
                "Activity",
                join_non_empty(&[
                    &p.activity_text,
                    &parenthesized(&[&self.person(&p.activity_user), &p.activity_unit]),
                ]),
            ),
            ("Document", p.document_name.clone()),
            ("Return date", valid_date(&p.return_date)),
            ("Assigned to", self.assignee(p)),
        ];
        lines.extend(
            optional
                .into_iter()
                .filter(|(_, value)| !value.is_empty())
                .map(|(label, value)| format!("{label}: {value}")),
        );
        lines
    }

    pub fn body(&self, event: &Event) -> String {
        if !self.full && event.kind != Kind::AccountLoginFailed {
            return String::new();
        }
        self.details(event).join("\n")
    }

    pub fn is_self(&self, acronym: &str) -> bool {
        !acronym.is_empty()
            && self
                .selves
                .iter()
                .any(|own| !own.is_empty() && equal_fold(acronym, own))
    }

    pub fn view(&self, event: &Event) -> View {
        let p = &event.payload;
        let mut detail = ViewDetail {
            document_protocol: p.document_protocol.clone(),
            assigned_to_me: self.assigned_to_self(p),
            activity_by_me: self.is_self(&p.activity_user),
            return_date: p.return_date.clone(),
            reason: p.reason.clone(),
            ..ViewDetail::default()
        };
        if self.full {
            self.fill_full_detail(&mut detail, p);
        }
        View {
            id: event.id,
            kind: event.kind,
            procedure_id: event.procedure_id,
            process_number: p.process_number.clone(),
            unit: p.unit.clone(),
            title: self.title(event),
            occurred_at: event.occurred_at,
            detected_at: event.detected_at,
            detail,
        }
    }

    pub fn payload(&self, event: &Event) -> Value {
        let Ok(Value::Object(view)) = serde_json::to_value(self.view(event)) else {
            return Value::Object(Map::new());
        };
        Value::Object(
            view.into_iter()
                .filter(|(key, _)| PAYLOAD_KEYS.contains(&key.as_str()))
                .collect(),
        )
    }

    fn fill_full_detail(&self, detail: &mut ViewDetail, p: &Payload) {
        detail.process_type = p.process_type.clone();
        detail.description = p.description.clone();
        detail.activity_text = p.activity_text.clone();
        detail.activity_user = self.other(&p.activity_user);
        detail.activity_unit = p.activity_unit.clone();
        detail.document_name = p.document_name.clone();
        detail.document_type = p.document_type.clone();
        detail.previous_assigned_user = self.other(&p.previous_assignee);
        if !detail.assigned_to_me {
            detail.assigned_user = self.other(&p.assigned_user);
            detail.assigned_user_name = p.assigned_user_name.clone();
        }
    }

    fn other(&self, acronym: &str) -> String {
        if self.is_self(acronym) {
            return String::new();
        }
        acronym.to_string()
    }

    fn person(&self, acronym: &str) -> String {
        if self.is_self(acronym) {
            return "you".to_string();
        }
        acronym.to_string()
    }

    fn assigned_to_self(&self, p: &Payload) -> bool {
        p.assigned_to_self || self.is_self(&p.assigned_user)
    }

    fn assignee(&self, p: &Payload) -> String {
        if self.assigned_to_self(p) {
            return "you".to_string();
        }
        join_non_empty(&[&p.assigned_user, &parenthesized(&[&p.assigned_user_name])])
    }

    fn assignment_title(&self, p: &Payload) -> String {
        let number = &p.process_number;
        if self.assigned_to_self(p) {
            return format!("Process {number} was assigned to you");
        }
        if p.assigned_user.is_empty() {
            return format!("Process {number} is no longer assigned to anyone");
        }
        if self.full {
            return format!("Process {number} was assigned to {}", p.assigned_user);
        }
        format!("Process {number} was assigned to someone else")
    }

    fn activity_title(&self, event: &Event) -> String {
        let p = &event.payload;
        if !self.full || trim(&p.activity_text).is_empty() {
            return format!(
                "New activity on process {} ({})",
                p.process_number,
                self.time(event.occurred_at)
            );
        }
        let described = trim(&p.description);
        let source = if described.is_empty() {
            &p.process_type
        } else {
            described
        };
        let label = shorten(source, LABEL_LIMIT);
        format!(
            "{}: {}",
            join_non_empty(&[&p.process_number, &parenthesized(&[&label])]),
            shorten(&p.activity_text, SUMMARY_LIMIT)
        )
    }

    fn return_due_title(&self, event: &Event) -> String {
        let p = &event.payload;
        let Some(date) = parse_date(&p.return_date) else {
            return format!(
                "Scheduled return of process {} is due soon",
                p.process_number
            );
        };
        let detected = event.detected_at.unwrap_or_else(zero_instant);
        let when = match days_until(detected, date, self.zone) {
            0 => "today".to_string(),
            1 => "tomorrow".to_string(),
            days => format!("in {days} days"),
        };
        format!(
            "Scheduled return of process {} is due {when} ({})",
            p.process_number, p.return_date
        )
    }

    fn time(&self, at: Option<DateTime<Utc>>) -> String {
        match at {
            Some(at) => format_local(at, self.zone),
            None => "unknown time".to_string(),
        }
    }
}

fn entered_title(p: &Payload) -> String {
    let suffix = if p.assigned_to_self {
        " (assigned to you)"
    } else {
        ""
    };
    format!("New process in {}: {}{suffix}", p.unit, p.process_number)
}

fn zero_instant() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0)
        .single()
        .expect("year 1 is a valid instant")
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
    if !shaped {
        return None;
    }
    NaiveDate::from_ymd_opt(
        text[0..4].parse().ok()?,
        text[5..7].parse().ok()?,
        text[8..10].parse().ok()?,
    )
}

fn valid_date(text: &str) -> String {
    match parse_date(text) {
        Some(_) => text.to_string(),
        None => String::new(),
    }
}

pub(crate) fn shorten(text: &str, limit: usize) -> String {
    let collapsed = fields(text).join(" ");
    let runes: Vec<char> = collapsed.chars().collect();
    if runes.len() <= limit {
        return collapsed;
    }
    let kept: String = runes[..limit.saturating_sub(1)].iter().collect();
    format!("{}…", trim(&kept))
}

fn non_empty<'a>(parts: &[&'a str]) -> Vec<&'a str> {
    parts
        .iter()
        .map(|part| trim(part))
        .filter(|part| !part.is_empty())
        .collect()
}

fn parenthesized(parts: &[&str]) -> String {
    let kept = non_empty(parts);
    if kept.is_empty() {
        return String::new();
    }
    format!("({})", kept.join(", "))
}

fn join_non_empty(parts: &[&str]) -> String {
    non_empty(parts).join(" ")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde::Deserialize;
    use serde_json::Value;

    use super::*;
    use crate::golden;
    use crate::gotext::to_lower;
    use crate::timezone::load_zone;

    const NUMBER: &str = "00000.000001/2026-00";
    const UNIT: &str = "SEDEMO/NUPROC";

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn at(day: u32, hour: u32, minute: u32, second: u32) -> Option<DateTime<Utc>> {
        Some(
            Utc.with_ymd_and_hms(2026, 11, day, hour, minute, second)
                .unwrap(),
        )
    }

    fn boa_vista(full: bool) -> Formatter {
        Formatter {
            zone: load_zone("America/Boa_Vista").unwrap(),
            full,
            selves: Vec::new(),
        }
    }

    fn utc(full: bool, own: &str) -> Formatter {
        Formatter {
            zone: Tz::UTC,
            full,
            selves: vec![own.to_string()],
        }
    }

    fn process() -> Payload {
        Payload {
            process_number: NUMBER.into(),
            unit: UNIT.into(),
            ..Payload::default()
        }
    }

    fn event(kind: Kind, payload: Payload) -> Event {
        Event {
            id: 0,
            account_id: 0,
            kind,
            procedure_id: 0,
            reference: String::new(),
            occurred_at: None,
            detected_at: None,
            payload,
        }
    }

    fn with(change: impl FnOnce(&mut Payload)) -> Payload {
        let mut payload = process();
        change(&mut payload);
        payload
    }

    #[test]
    fn titles_follow_the_kind() {
        let detected = at(4, 18, 0, 0);
        let described = |text: &str| {
            with(|p| {
                p.process_type = "Contrato".into();
                p.description = "Compra de cadeiras".into();
                p.activity_text = text.into();
            })
        };
        let due = |date: &str| Event {
            detected_at: detected,
            ..event(Kind::ReturnDueSoon, with(|p| p.return_date = date.into()))
        };
        let activity = |payload: Payload, when| Event {
            occurred_at: when,
            ..event(Kind::ActivityAdded, payload)
        };
        let activity_title = "New activity on process 00000.000001/2026-00 (2026-11-04 14:00)";
        let cases: Vec<(&str, Event, String, Option<String>)> = vec![
            (
                "entered",
                event(Kind::ProcessEntered, process()),
                format!("New process in {UNIT}: {NUMBER}"),
                None,
            ),
            (
                "entered and assigned to the user",
                event(Kind::ProcessEntered, with(|p| p.assigned_to_self = true)),
                format!("New process in {UNIT}: {NUMBER} (assigned to you)"),
                None,
            ),
            (
                "left",
                event(Kind::ProcessLeft, process()),
                format!("Process {NUMBER} left {UNIT}"),
                None,
            ),
            (
                "activity in the SEI time zone",
                activity(process(), at(4, 17, 13, 5)),
                "New activity on process 00000.000001/2026-00 (2026-11-04 13:13)".into(),
                None,
            ),
            (
                "activity with a description",
                activity(described("Processo recebido na unidade"), detected),
                activity_title.into(),
                Some(format!(
                    "{NUMBER} (Compra de cadeiras): Processo recebido na unidade"
                )),
            ),
            (
                "activity on a process without a description",
                activity(
                    with(|p| {
                        p.process_type = "Contrato".into();
                        p.activity_text = "Processo recebido na unidade".into();
                    }),
                    detected,
                ),
                activity_title.into(),
                Some(format!("{NUMBER} (Contrato): Processo recebido na unidade")),
            ),
            (
                "activity with long texts",
                activity(
                    with(|p| {
                        p.description =
                            "Aquisição de cadeiras ergonômicas para o setor de protocolo".into();
                        p.activity_text = "Processo remetido pela unidade SEDEMO/NUPROC\n  para a unidade SEDEMO/PROTOCOLO com retorno programado".into();
                    }),
                    detected,
                ),
                activity_title.into(),
                Some(format!(
                    "{NUMBER} (Aquisição de cadeiras ergonômicas para…): Processo remetido pela unidade SEDEMO/NUPROC para a unidade SEDEMO/PROTOCOLO co…"
                )),
            ),
            (
                "document",
                event(Kind::DocumentAdded, process()),
                format!("New document on process {NUMBER}"),
                None,
            ),
            (
                "assigned to the user",
                event(
                    Kind::AssignmentChanged,
                    with(|p| {
                        p.assigned_user = "fulano.tal".into();
                        p.assigned_to_self = true;
                    }),
                ),
                format!("Process {NUMBER} was assigned to you"),
                None,
            ),
            (
                "assigned to someone else",
                event(
                    Kind::AssignmentChanged,
                    with(|p| p.assigned_user = "beltrano.silva".into()),
                ),
                format!("Process {NUMBER} was assigned to someone else"),
                Some(format!("Process {NUMBER} was assigned to beltrano.silva")),
            ),
            (
                "assignment removed",
                event(Kind::AssignmentChanged, process()),
                format!("Process {NUMBER} is no longer assigned to anyone"),
                None,
            ),
            (
                "return in two days",
                due("2026-11-06"),
                format!("Scheduled return of process {NUMBER} is due in 2 days (2026-11-06)"),
                None,
            ),
            (
                "return tomorrow",
                due("2026-11-05"),
                format!("Scheduled return of process {NUMBER} is due tomorrow (2026-11-05)"),
                None,
            ),
            (
                "return today",
                due("2026-11-04"),
                format!("Scheduled return of process {NUMBER} is due today (2026-11-04)"),
                None,
            ),
            (
                "overdue return",
                event(Kind::ReturnOverdue, process()),
                format!("Scheduled return of process {NUMBER} is overdue"),
                None,
            ),
            (
                "login failure",
                event(
                    Kind::AccountLoginFailed,
                    Payload {
                        reason: "SEI rejected the login".into(),
                        ..Payload::default()
                    },
                ),
                "sei-monitor could not log in to SEI".into(),
                None,
            ),
        ];
        for (name, event, minimal, full) in cases {
            let want_full = full.unwrap_or_else(|| minimal.clone());
            assert_eq!(boa_vista(false).title(&event), minimal, "{name} (minimal)");
            assert_eq!(boa_vista(true).title(&event), want_full, "{name} (full)");
        }
    }

    fn contract_activity() -> Event {
        Event {
            occurred_at: at(4, 17, 13, 5),
            ..event(
                Kind::ActivityAdded,
                with(|p| {
                    p.process_type = "Contrato".into();
                    p.description = "Compra de cadeiras".into();
                    p.activity_text = "Processo recebido na unidade".into();
                    p.activity_user = "beltrano.silva".into();
                    p.activity_unit = "SEDEMO/PROTOCOLO".into();
                }),
            )
        }
    }

    #[test]
    fn details_follow_the_detail_level() {
        let event = contract_activity();

        assert_eq!(
            boa_vista(false).details(&event),
            ["When: 2026-11-04 13:13", "Unit: SEDEMO/NUPROC"]
        );
        assert_eq!(
            boa_vista(true).details(&event),
            [
                "When: 2026-11-04 13:13",
                "Unit: SEDEMO/NUPROC",
                "Type: Contrato",
                "Description: Compra de cadeiras",
                "Activity: Processo recebido na unidade (beltrano.silva, SEDEMO/PROTOCOLO)",
            ]
        );
    }

    #[test]
    fn minimal_details_hide_other_people_and_content() {
        let event = event(
            Kind::AssignmentChanged,
            with(|p| {
                p.process_type = "Contrato".into();
                p.description = "Compra de cadeiras".into();
                p.assigned_user = "beltrano.silva".into();
                p.assigned_user_name = "Beltrano da Silva".into();
                p.document_name = "Ofício 12 (0012345)".into();
            }),
        );
        let formatter = utc(false, "");

        let text = format!(
            "{}\n{}",
            formatter.title(&event),
            formatter.details(&event).join("\n")
        );

        for hidden in ["beltrano", "Beltrano", "Contrato", "cadeiras", "Ofício"] {
            assert!(
                !text.contains(hidden),
                "minimal text shows {hidden:?}:\n{text}"
            );
        }
        let full = utc(true, "").details(&event);
        assert!(full.contains(&"Assigned to: beltrano.silva (Beltrano da Silva)".to_string()));
        assert!(full.contains(&"Document: Ofício 12 (0012345)".to_string()));
    }

    #[test]
    fn full_text_calls_the_user_you() {
        let formatter = utc(true, "12345678901");
        let activity = event(
            Kind::ActivityAdded,
            with(|p| {
                p.activity_text = "Processo recebido na unidade".into();
                p.activity_user = "12345678901".into();
                p.activity_unit = UNIT.into();
            }),
        );
        let assignment = event(
            Kind::AssignmentChanged,
            Payload {
                process_number: NUMBER.into(),
                assigned_user: "12345678901".into(),
                assigned_user_name: "Fulano de Tal".into(),
                ..Payload::default()
            },
        );

        let activity_details = formatter.details(&activity);
        let assignment_details = formatter.details(&assignment);

        assert!(
            activity_details.contains(
                &"Activity: Processo recebido na unidade (you, SEDEMO/NUPROC)".to_string()
            )
        );
        assert_eq!(
            formatter.title(&assignment),
            format!("Process {NUMBER} was assigned to you")
        );
        assert!(assignment_details.contains(&"Assigned to: you".to_string()));
        let text = [activity_details, assignment_details].concat().join("\n");
        assert!(
            !text.contains("12345678901") && !text.contains("Fulano"),
            "{text}"
        );
    }

    #[test]
    fn login_failure_details() {
        let failure = event(
            Kind::AccountLoginFailed,
            Payload {
                reason: "SEI rejected the login (Usuário ou Senha Inválida.)".into(),
                ..Payload::default()
            },
        );

        assert_eq!(
            utc(false, "").details(&failure),
            [
                "Reason: SEI rejected the login (Usuário ou Senha Inválida.)",
                "Monitoring is paused until sei-monitor restarts with the configuration fixed.",
            ]
        );
    }

    #[test]
    fn body_shows_details_only_in_full_mode_or_for_a_login_failure() {
        let document = event(Kind::DocumentAdded, process());
        let failure = event(
            Kind::AccountLoginFailed,
            Payload {
                reason: "SEI rejected the login (Usuário ou Senha Inválida.)".into(),
                ..Payload::default()
            },
        );

        assert_eq!(utc(false, "").body(&document), "");
        assert!(utc(true, "").body(&document).starts_with("When: "));
        assert_eq!(
            utc(false, "").body(&failure),
            "Reason: SEI rejected the login (Usuário ou Senha Inválida.)\nMonitoring is paused until sei-monitor restarts with the configuration fixed."
        );
    }

    #[test]
    fn either_self_value_hides_the_user() {
        let selves = vec!["fulano.tal".to_string(), "12345678900".to_string()];
        let formatter = Formatter {
            zone: Tz::UTC,
            full: true,
            selves,
        };
        let assignment = event(
            Kind::AssignmentChanged,
            with(|p| p.assigned_user = "12345678900".into()),
        );
        let activity = event(
            Kind::ActivityAdded,
            with(|p| {
                p.activity_text = "texto".into();
                p.activity_user = "FULANO.TAL".into();
            }),
        );

        let assigned = formatter.payload(&assignment);
        let recorded = formatter.payload(&activity);

        assert_eq!(
            formatter.title(&assignment),
            format!("Process {NUMBER} was assigned to you")
        );
        assert_eq!(assigned["detail"]["assigned_to_me"], true);
        assert!(!assigned.to_string().contains("12345678900"));
        assert_eq!(recorded["detail"]["activity_by_me"], true);
        assert!(!recorded.to_string().contains("FULANO.TAL"));
    }

    #[test]
    fn empty_selves_match_nothing() {
        let none = Formatter {
            zone: Tz::UTC,
            full: false,
            selves: vec![String::new()],
        };

        assert!(!none.is_self(""));
        assert!(!none.is_self("fulano.tal"));
        assert!(!utc(false, "").is_self("fulano.tal"));
    }

    #[test]
    fn payload_keeps_only_the_contract_fields() {
        let entered = Event {
            procedure_id: 9001,
            ..event(Kind::ProcessEntered, process())
        };
        let failure = event(
            Kind::AccountLoginFailed,
            Payload {
                reason: "SEI rejected the login".into(),
                ..Payload::default()
            },
        );

        let entered = utc(false, "").payload(&entered);
        let failure = utc(false, "").payload(&failure);

        let mut keys: Vec<&String> = entered.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["detail", "procedure_id", "process_number", "unit"]);
        assert_eq!(failure["detail"]["reason"], "SEI rejected the login");
        assert!(failure.get("procedure_id").is_none());
    }

    #[derive(Deserialize)]
    struct GoldenEvent {
        id: i64,
        kind: String,
        procedure_id: i64,
        r#ref: String,
        occurred_at: String,
        detected_at: String,
        payload: Payload,
    }

    #[derive(Deserialize)]
    struct EventTextCase {
        name: String,
        event: GoldenEvent,
        zone: String,
        #[serde(rename = "self")]
        own: String,
        full: bool,
        title: String,
        details: Vec<String>,
        view: Value,
        hidden: Vec<String>,
    }

    fn golden_instant(text: &str) -> Option<DateTime<Utc>> {
        let parsed = instant(text);
        (parsed != zero_instant()).then_some(parsed)
    }

    fn golden_event(source: &GoldenEvent) -> Event {
        Event {
            id: source.id,
            account_id: 0,
            kind: Kind::parse(&source.kind).unwrap_or_else(|| panic!("kind {}", source.kind)),
            procedure_id: source.procedure_id,
            reference: source.r#ref.clone(),
            occurred_at: golden_instant(&source.occurred_at),
            detected_at: golden_instant(&source.detected_at),
            payload: source.payload.clone(),
        }
    }

    fn restricted(view: &Value) -> Value {
        let kept = view
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, _)| PAYLOAD_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Value::Object(kept)
    }

    fn assert_hidden(case: &EventTextCase, shown: &[(&str, String)]) {
        for hidden in &case.hidden {
            let needle = to_lower(hidden);
            for (place, text) in shown {
                assert!(
                    !to_lower(text).contains(&needle),
                    "{}: {place} shows {hidden:?}:\n{text}",
                    case.name
                );
            }
        }
    }

    #[test]
    fn event_text_matches_go() {
        let cases = golden::cases::<EventTextCase>("event_text", "");
        let mut declared = 0;
        for case in cases {
            let formatter = Formatter {
                zone: load_zone(&case.zone).unwrap(),
                full: case.full,
                selves: vec![case.own.clone()],
            };
            let event = golden_event(&case.event);
            let view = formatter.view(&event);
            let view_json = serde_json::to_value(&view).unwrap();
            let payload = formatter.payload(&event);
            let details = formatter.details(&event);

            assert_eq!(formatter.title(&event), case.title, "{} title", case.name);
            assert_eq!(details, case.details, "{} details", case.name);
            assert_eq!(view_json, case.view, "{} view", case.name);
            assert_eq!(payload, restricted(&case.view), "{} payload", case.name);

            if case.hidden.is_empty() {
                continue;
            }
            declared += 1;
            assert_hidden(
                &case,
                &[
                    ("title", formatter.title(&event)),
                    ("details", details.join("\n")),
                    ("view", view_json.to_string()),
                    ("payload", payload.to_string()),
                ],
            );
        }
        assert!(
            declared >= 14,
            "only {declared} cases declare hidden strings"
        );
    }

    #[derive(Deserialize)]
    struct ShortenCase {
        text: String,
        limit: usize,
        output: String,
    }

    #[test]
    fn shorten_matches_go() {
        for case in golden::cases::<ShortenCase>("shorten", "") {
            assert_eq!(
                shorten(&case.text, case.limit),
                case.output,
                "{:?} within {}",
                case.text,
                case.limit
            );
        }
    }
}
