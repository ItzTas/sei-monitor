use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use chrono_tz::Tz;

use crate::event::{Event, Kind, Payload};
use crate::gotext::equal_fold;
use crate::sei::{Activity, Document, Process, Unit};
use crate::store::ProcessRow;
use crate::timezone::days_until;

#[derive(Clone, Debug)]
pub struct Observation {
    pub process: Process,
    pub unit: Unit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CurrentUser {
    pub user_id: i64,
    pub acronym: String,
}

pub struct Rules {
    pub now: DateTime<Utc>,
    pub zone: Tz,
    pub return_warning_days: i64,
    pub user: CurrentUser,
    pub baseline: bool,
}

#[derive(Clone, Debug)]
pub struct Presence {
    pub observation: Observation,
    pub previous: Option<ProcessRow>,
    pub entered: bool,
}

impl Presence {
    pub fn quiet(&self, baseline: bool) -> bool {
        baseline
            || self.entered
            || self
                .previous
                .as_ref()
                .is_none_or(|previous| previous.last_activity_id == 0)
    }
}

pub struct ProcessChanges {
    pub present: Vec<Presence>,
    pub left: Vec<ProcessRow>,
    pub events: Vec<Event>,
}

pub fn diff_processes(
    previous: &[ProcessRow],
    current: &[Observation],
    rules: &Rules,
) -> ProcessChanges {
    let known: HashMap<i64, &ProcessRow> =
        previous.iter().map(|row| (row.procedure_id, row)).collect();
    let mut changes = ProcessChanges {
        present: Vec::new(),
        left: Vec::new(),
        events: Vec::new(),
    };
    let mut seen = HashSet::new();
    for observation in current {
        let id = observation.process.procedure_id;
        if !seen.insert(id) {
            continue;
        }
        let previous = known.get(&id).map(|row| (*row).clone());
        let entered = previous.as_ref().is_none_or(|row| row.left_at.is_some());
        let presence = Presence {
            observation: observation.clone(),
            previous,
            entered,
        };
        if !rules.baseline {
            changes.events.extend(process_events(&presence, rules));
        }
        changes.present.push(presence);
    }
    for row in previous {
        if seen.contains(&row.procedure_id) || row.left_at.is_some() {
            continue;
        }
        changes.left.push(row.clone());
        if !rules.baseline {
            changes.events.push(left_event(row, rules));
        }
    }
    changes
}

pub fn fresh_activities(last_seen_id: i64, newest_first: &[Activity]) -> (Vec<Activity>, bool) {
    let fresh: Vec<Activity> = newest_first
        .iter()
        .take_while(|activity| activity.id > last_seen_id)
        .cloned()
        .collect();
    let reached = fresh.len() < newest_first.len();
    (fresh, reached)
}

pub fn fresh_documents(known: &HashSet<i64>, documents: &[Document]) -> Vec<Document> {
    documents
        .iter()
        .filter(|document| !known.contains(&document.id))
        .cloned()
        .collect()
}

pub fn is_self(id: i64, acronym: &str, user: &CurrentUser) -> bool {
    if id != 0 && user.user_id != 0 {
        return id == user.user_id;
    }
    !acronym.is_empty() && equal_fold(acronym, &user.acronym)
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the monitoring cycle of Task 13 is the first caller"
    )
)]
pub(crate) fn activity_event(presence: &Presence, activity: &Activity) -> Event {
    let mut payload = base_payload(presence);
    payload.activity_text.clone_from(&activity.text);
    payload.activity_user.clone_from(&activity.user);
    payload.activity_unit.clone_from(&activity.unit);
    let mut event = new_event(
        Kind::ActivityAdded,
        presence.observation.process.procedure_id,
        activity.id.to_string(),
        payload,
    );
    event.occurred_at = activity.at;
    event
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the monitoring cycle of Task 13 is the first caller"
    )
)]
pub(crate) fn document_event(presence: &Presence, document: &Document) -> Event {
    let mut payload = base_payload(presence);
    payload.document_name.clone_from(&document.name);
    payload.document_type.clone_from(&document.document_type);
    payload.document_protocol.clone_from(&document.protocol);
    new_event(
        Kind::DocumentAdded,
        presence.observation.process.procedure_id,
        document.id.to_string(),
        payload,
    )
}

pub(crate) fn format_date(date: Option<NaiveDate>) -> String {
    date.map_or_else(String::new, |date| date.format("%Y-%m-%d").to_string())
}

fn process_events(presence: &Presence, rules: &Rules) -> Vec<Event> {
    if presence.entered {
        return vec![entered_event(presence, rules)];
    }
    [assignment_event, overdue_event, due_soon_event]
        .into_iter()
        .filter_map(|check| check(presence, rules))
        .collect()
}

fn entered_event(presence: &Presence, rules: &Rules) -> Event {
    let process = &presence.observation.process;
    let mut payload = base_payload(presence);
    if let Some(assignee) = &process.assignee {
        payload.assigned_user.clone_from(&assignee.acronym);
        payload.assigned_user_name.clone_from(&assignee.name);
        payload.assigned_to_self = is_self(assignee.id, &assignee.acronym, &rules.user);
    }
    new_event(
        Kind::ProcessEntered,
        process.procedure_id,
        cycle_ref(process.procedure_id, rules.now),
        payload,
    )
}

fn left_event(row: &ProcessRow, rules: &Rules) -> Event {
    let payload = Payload {
        process_number: row.number.clone(),
        unit: row.unit_acronym.clone(),
        process_type: row.process_type.clone(),
        description: row.description.clone(),
        ..Payload::default()
    };
    new_event(
        Kind::ProcessLeft,
        row.procedure_id,
        cycle_ref(row.procedure_id, rules.now),
        payload,
    )
}

fn assignment_event(presence: &Presence, rules: &Rules) -> Option<Event> {
    let process = &presence.observation.process;
    let previous = presence.previous.as_ref()?;
    let (id, acronym, name) = process
        .assignee
        .as_ref()
        .map_or((0, "", ""), |user| (user.id, &user.acronym, &user.name));
    if same_assignee(
        previous.assigned_user_id,
        &previous.assigned_user,
        id,
        acronym,
    ) {
        return None;
    }
    let mut payload = base_payload(presence);
    payload.assigned_user = acronym.to_owned();
    payload.assigned_user_name = name.to_owned();
    payload
        .previous_assignee
        .clone_from(&previous.assigned_user);
    payload.assigned_to_self = !acronym.is_empty() && is_self(id, acronym, &rules.user);
    Some(new_event(
        Kind::AssignmentChanged,
        process.procedure_id,
        cycle_ref(process.procedure_id, rules.now),
        payload,
    ))
}

fn overdue_event(presence: &Presence, rules: &Rules) -> Option<Event> {
    let process = &presence.observation.process;
    let previous = presence.previous.as_ref()?;
    if previous.return_overdue || !process.return_overdue {
        return None;
    }
    let mut payload = base_payload(presence);
    payload.return_date = format_date(process.return_date);
    let reference = if payload.return_date.is_empty() {
        cycle_ref(process.procedure_id, rules.now)
    } else {
        format!("{}:{}", process.procedure_id, payload.return_date)
    };
    Some(new_event(
        Kind::ReturnOverdue,
        process.procedure_id,
        reference,
        payload,
    ))
}

fn due_soon_event(presence: &Presence, rules: &Rules) -> Option<Event> {
    let process = &presence.observation.process;
    let date = process.return_date?;
    if process.return_overdue {
        return None;
    }
    let days = days_until(rules.now, date, rules.zone);
    if !(0..=rules.return_warning_days).contains(&days) {
        return None;
    }
    let mut payload = base_payload(presence);
    payload.return_date = format_date(Some(date));
    let reference = format!("{}:{}", process.procedure_id, payload.return_date);
    Some(new_event(
        Kind::ReturnDueSoon,
        process.procedure_id,
        reference,
        payload,
    ))
}

fn new_event(kind: Kind, procedure_id: i64, reference: String, payload: Payload) -> Event {
    Event {
        id: 0,
        account_id: 0,
        kind,
        procedure_id,
        reference,
        occurred_at: None,
        detected_at: None,
        payload,
    }
}

fn base_payload(presence: &Presence) -> Payload {
    let observation = &presence.observation;
    Payload {
        process_number: observation.process.number.clone(),
        unit: observation.unit.acronym.clone(),
        process_type: observation.process.process_type.clone(),
        description: observation.process.description.clone(),
        ..Payload::default()
    }
}

fn cycle_ref(procedure_id: i64, now: DateTime<Utc>) -> String {
    format!(
        "{procedure_id}:{}",
        now.to_rfc3339_opts(SecondsFormat::Secs, true)
    )
}

fn same_assignee(
    previous_id: i64,
    previous_acronym: &str,
    current_id: i64,
    current_acronym: &str,
) -> bool {
    if previous_id != 0 && current_id != 0 {
        return previous_id == current_id;
    }
    equal_fold(previous_acronym, current_acronym)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::golden;
    use crate::sei::User;
    use crate::timezone::load_zone;

    #[derive(Deserialize)]
    struct StoredCase {
        procedure_id: i64,
        number: String,
        #[serde(rename = "type")]
        process_type: String,
        description: String,
        unit_id: i64,
        unit_acronym: String,
        assigned_user: String,
        assigned_user_id: i64,
        return_date: String,
        return_overdue: bool,
        last_activity_id: i64,
        document_total: i64,
        first_seen_at: String,
        last_seen_at: String,
        left_at: Option<String>,
    }

    #[derive(Deserialize)]
    struct UserCase {
        id: i64,
        acronym: String,
        name: String,
    }

    #[derive(Deserialize)]
    struct ProcessCase {
        procedure_id: i64,
        number: String,
        #[serde(rename = "type")]
        process_type: String,
        description: String,
        assignee: Option<UserCase>,
        return_date: String,
        return_overdue: bool,
        origin: String,
    }

    #[derive(Deserialize)]
    struct UnitCase {
        id: i64,
        acronym: String,
    }

    #[derive(Deserialize)]
    struct ObservationCase {
        process: ProcessCase,
        unit: UnitCase,
    }

    #[derive(Deserialize)]
    struct SelfCase {
        user_id: i64,
        acronym: String,
    }

    #[derive(Deserialize)]
    struct RulesCase {
        now: String,
        zone: String,
        return_warning_days: i64,
        #[serde(rename = "self")]
        user: SelfCase,
        baseline: bool,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct PresenceCase {
        procedure_id: i64,
        entered: bool,
        quiet: bool,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct EventCase {
        kind: String,
        procedure_id: i64,
        #[serde(rename = "ref")]
        reference: String,
        occurred_at: String,
        payload: Payload,
    }

    #[derive(Deserialize)]
    struct ResultCase {
        present: Vec<PresenceCase>,
        left: Vec<i64>,
        events: Vec<EventCase>,
    }

    #[derive(Deserialize)]
    struct DiffCase {
        name: String,
        previous: Vec<StoredCase>,
        current: Vec<ObservationCase>,
        rules: RulesCase,
        result: ResultCase,
    }

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap_or_else(|err| panic!("bad instant {text:?}: {err}"))
            .with_timezone(&Utc)
    }

    fn date(text: &str) -> Option<NaiveDate> {
        (!text.is_empty()).then(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .unwrap_or_else(|err| panic!("bad date {text:?}: {err}"))
        })
    }

    fn stored(case: StoredCase) -> ProcessRow {
        ProcessRow {
            procedure_id: case.procedure_id,
            number: case.number,
            process_type: case.process_type,
            description: case.description,
            unit_id: case.unit_id,
            unit_acronym: case.unit_acronym,
            assigned_user: case.assigned_user,
            assigned_user_id: case.assigned_user_id,
            return_date: case.return_date,
            return_overdue: case.return_overdue,
            last_activity_id: case.last_activity_id,
            document_total: case.document_total,
            first_seen_at: Some(instant(&case.first_seen_at)),
            last_seen_at: Some(instant(&case.last_seen_at)),
            left_at: case.left_at.as_deref().map(instant),
        }
    }

    fn observation(case: ObservationCase) -> Observation {
        let process = case.process;
        Observation {
            process: Process {
                procedure_id: process.procedure_id,
                number: process.number,
                process_type: process.process_type,
                description: process.description,
                assignee: process.assignee.map(|user| User {
                    id: user.id,
                    acronym: user.acronym,
                    name: user.name,
                }),
                return_date: date(&process.return_date),
                return_overdue: process.return_overdue,
                origin: process.origin,
            },
            unit: Unit {
                id: case.unit.id,
                acronym: case.unit.acronym,
                description: String::new(),
            },
        }
    }

    fn occurred(event: &Event) -> String {
        event.occurred_at.map_or_else(
            || "0001-01-01T00:00:00Z".to_owned(),
            |at| at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        )
    }

    #[test]
    fn diff_matches_go() {
        for case in golden::cases::<DiffCase>("diff", "") {
            let previous: Vec<ProcessRow> = case.previous.into_iter().map(stored).collect();
            let current: Vec<Observation> = case.current.into_iter().map(observation).collect();
            let rules = Rules {
                now: instant(&case.rules.now),
                zone: load_zone(&case.rules.zone).expect("vector zone exists"),
                return_warning_days: case.rules.return_warning_days,
                user: CurrentUser {
                    user_id: case.rules.user.user_id,
                    acronym: case.rules.user.acronym,
                },
                baseline: case.rules.baseline,
            };

            let changes = diff_processes(&previous, &current, &rules);

            let present: Vec<PresenceCase> = changes
                .present
                .iter()
                .map(|presence| PresenceCase {
                    procedure_id: presence.observation.process.procedure_id,
                    entered: presence.entered,
                    quiet: presence.quiet(rules.baseline),
                })
                .collect();
            let left: Vec<i64> = changes.left.iter().map(|row| row.procedure_id).collect();
            let events: Vec<EventCase> = changes
                .events
                .iter()
                .map(|event| EventCase {
                    kind: event.kind.as_str().to_owned(),
                    procedure_id: event.procedure_id,
                    reference: event.reference.clone(),
                    occurred_at: occurred(event),
                    payload: event.payload.clone(),
                })
                .collect();
            assert_eq!(present, case.result.present, "{}: present", case.name);
            assert_eq!(left, case.result.left, "{}: left", case.name);
            assert_eq!(events, case.result.events, "{}: events", case.name);
        }
    }

    fn activity(id: i64) -> Activity {
        Activity {
            id,
            procedure_id: 0,
            user: String::new(),
            unit: String::new(),
            text: String::new(),
            at: None,
        }
    }

    #[test]
    fn fresh_activities_stop_at_the_last_seen() {
        let page = [activity(507), activity(506), activity(505), activity(503)];

        let (fresh, reached) = fresh_activities(505, &page);
        assert!(reached);
        assert_eq!(fresh, [activity(507), activity(506)]);

        let (fresh, reached) = fresh_activities(400, &page);
        assert!(!reached);
        assert_eq!(fresh.len(), 4);
    }

    #[test]
    fn fresh_documents_are_the_unknown_ones() {
        let documents: Vec<Document> = [801, 802, 803]
            .into_iter()
            .map(|id| Document {
                id,
                procedure_id: 0,
                protocol: String::new(),
                document_type: String::new(),
                name: String::new(),
            })
            .collect();

        let fresh = fresh_documents(&HashSet::from([801, 803]), &documents);

        assert_eq!(fresh, [documents[1].clone()]);
    }

    fn presence() -> Presence {
        Presence {
            observation: Observation {
                process: Process {
                    procedure_id: 7,
                    number: "00000.000007/2026-00".to_owned(),
                    process_type: "Contrato".to_owned(),
                    description: "Compra de mesas".to_owned(),
                    assignee: None,
                    return_date: None,
                    return_overdue: false,
                    origin: String::new(),
                },
                unit: Unit {
                    id: 1001,
                    acronym: "SEDEMO/NUPROC".to_owned(),
                    description: String::new(),
                },
            },
            previous: None,
            entered: false,
        }
    }

    #[test]
    fn activity_event_carries_the_activity_and_its_time() {
        let at = instant("2026-11-04T17:30:00Z");
        let event = activity_event(
            &presence(),
            &Activity {
                id: 612,
                procedure_id: 7,
                user: "beltrano.silva".to_owned(),
                unit: "SEDEMO/GAB".to_owned(),
                text: "Processo remetido".to_owned(),
                at: Some(at),
            },
        );

        assert_eq!(event.kind, Kind::ActivityAdded);
        assert_eq!(event.procedure_id, 7);
        assert_eq!(event.reference, "612");
        assert_eq!(event.occurred_at, Some(at));
        assert_eq!(
            event.payload,
            Payload {
                process_number: "00000.000007/2026-00".to_owned(),
                unit: "SEDEMO/NUPROC".to_owned(),
                process_type: "Contrato".to_owned(),
                description: "Compra de mesas".to_owned(),
                activity_text: "Processo remetido".to_owned(),
                activity_user: "beltrano.silva".to_owned(),
                activity_unit: "SEDEMO/GAB".to_owned(),
                ..Payload::default()
            }
        );
    }

    #[test]
    fn document_event_carries_the_document() {
        let event = document_event(
            &presence(),
            &Document {
                id: 802,
                procedure_id: 7,
                protocol: "0001234".to_owned(),
                document_type: "Parecer".to_owned(),
                name: "Parecer 12".to_owned(),
            },
        );

        assert_eq!(event.kind, Kind::DocumentAdded);
        assert_eq!(event.reference, "802");
        assert_eq!(event.occurred_at, None);
        assert_eq!(event.payload.document_name, "Parecer 12");
        assert_eq!(event.payload.document_type, "Parecer");
        assert_eq!(event.payload.document_protocol, "0001234");
        assert_eq!(event.payload.unit, "SEDEMO/NUPROC");
    }
}
