use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Serialize, Serializer};

use super::Kind;

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

fn kind_text<S: Serializer>(kind: &Kind, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(kind.as_str())
}

fn rfc3339_nano(at: DateTime<Utc>) -> String {
    let base = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    let nanos = at.nanosecond() % 1_000_000_000;
    if nanos == 0 {
        return format!("{base}Z");
    }
    let fraction = format!("{nanos:09}");
    format!("{base}.{}Z", fraction.trim_end_matches('0'))
}

fn instant_text<S: Serializer>(
    at: &Option<DateTime<Utc>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match at {
        Some(at) => serializer.serialize_str(&rfc3339_nano(*at)),
        None => serializer.serialize_str("0001-01-01T00:00:00Z"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct View {
    pub id: i64,
    #[serde(serialize_with = "kind_text")]
    pub kind: Kind,
    #[serde(skip_serializing_if = "is_zero")]
    pub procedure_id: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub process_number: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unit: String,
    pub title: String,
    #[serde(serialize_with = "instant_text")]
    pub occurred_at: Option<DateTime<Utc>>,
    #[serde(serialize_with = "instant_text")]
    pub detected_at: Option<DateTime<Utc>>,
    pub detail: ViewDetail,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ViewDetail {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub process_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub activity_text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub activity_user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub activity_unit: String,
    #[serde(skip_serializing_if = "is_false")]
    pub activity_by_me: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub document_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub document_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub document_protocol: String,
    #[serde(skip_serializing_if = "is_false")]
    pub assigned_to_me: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub assigned_user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub assigned_user_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub previous_assigned_user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub return_date: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::{Value, json};

    use super::*;
    use crate::event::{Event, Formatter, Payload};

    fn sample_event() -> Event {
        Event {
            id: 42,
            account_id: 0,
            kind: Kind::ActivityAdded,
            procedure_id: 9001,
            reference: String::new(),
            occurred_at: Some(Utc.with_ymd_and_hms(2026, 11, 4, 21, 13, 5).unwrap()),
            detected_at: Some(Utc.with_ymd_and_hms(2026, 11, 4, 18, 0, 0).unwrap()),
            payload: Payload {
                process_number: "00000.000001/2026-00".into(),
                unit: "SEDEMO/NUPROC".into(),
                process_type: "Contrato".into(),
                description: "Compra de cadeiras".into(),
                activity_text: "Processo recebido na unidade".into(),
                activity_user: "beltrano.silva".into(),
                activity_unit: "SEDEMO/PROTOCOLO".into(),
                ..Payload::default()
            },
        }
    }

    fn formatter(full: bool, own: &str) -> Formatter {
        Formatter {
            zone: chrono_tz::Tz::UTC,
            full,
            selves: vec![own.to_string()],
        }
    }

    fn encode(view: &View) -> Value {
        serde_json::to_value(view).unwrap()
    }

    #[test]
    fn minimal_view_leaves_the_details_out() {
        let view = encode(&formatter(false, "").view(&sample_event()));

        assert_eq!(view["id"], 42);
        assert_eq!(view["kind"], "activity_added");
        assert_eq!(view["process_number"], "00000.000001/2026-00");
        assert_eq!(view["unit"], "SEDEMO/NUPROC");
        assert_eq!(view["occurred_at"], "2026-11-04T21:13:05Z");
        assert_eq!(
            view["title"],
            "New activity on process 00000.000001/2026-00 (2026-11-04 21:13)"
        );
        assert_eq!(view["detail"], json!({}));
    }

    #[test]
    fn full_view_shows_the_details_but_never_the_user_own_acronym() {
        let formatter = formatter(true, "Fulano.Tal");
        let mut event = sample_event();

        let other = encode(&formatter.view(&event))["detail"].clone();
        event.payload.activity_user = "fulano.tal".into();
        let own = encode(&formatter.view(&event))["detail"].clone();

        assert_eq!(other["activity_text"], "Processo recebido na unidade");
        assert_eq!(other["activity_user"], "beltrano.silva");
        assert_eq!(other["process_type"], "Contrato");
        assert_eq!(other["description"], "Compra de cadeiras");
        assert!(own.get("activity_user").is_none());
        assert_eq!(own["activity_by_me"], true);
    }

    #[test]
    fn assignment_view_hides_the_user_behind_assigned_to_me() {
        let event = Event {
            kind: Kind::AssignmentChanged,
            payload: Payload {
                process_number: "00000.000001/2026-00".into(),
                assigned_user: "fulano.tal".into(),
                assigned_user_name: "Fulano de Tal".into(),
                assigned_to_self: true,
                previous_assignee: "beltrano.silva".into(),
                ..Payload::default()
            },
            ..sample_event()
        };

        let text = serde_json::to_string(&formatter(true, "fulano.tal").view(&event)).unwrap();

        assert!(!text.contains("fulano.tal"));
        assert!(!text.contains("Fulano de Tal"));
        assert!(text.contains(r#""assigned_to_me":true"#));
        assert!(text.contains(r#""previous_assigned_user":"beltrano.silva""#));
    }

    #[test]
    fn a_missing_time_is_written_like_a_go_zero_time() {
        let event = Event {
            occurred_at: None,
            ..sample_event()
        };

        let view = encode(&formatter(false, "").view(&event));

        assert_eq!(view["occurred_at"], "0001-01-01T00:00:00Z");
    }

    #[test]
    fn times_are_written_like_go_marshals_them() {
        let cases = [
            (123_456_789, "2026-11-04T17:13:05.123456789Z"),
            (500_000_000, "2026-11-04T17:13:05.5Z"),
            (120_000_000, "2026-11-04T17:13:05.12Z"),
            (1, "2026-11-04T17:13:05.000000001Z"),
            (1_000, "2026-11-04T17:13:05.000001Z"),
            (0, "2026-11-04T17:13:05Z"),
        ];
        for (nanos, want) in cases {
            let at = Utc
                .with_ymd_and_hms(2026, 11, 4, 17, 13, 5)
                .unwrap()
                .with_nanosecond(nanos)
                .unwrap();
            let event = Event {
                occurred_at: Some(at),
                ..sample_event()
            };

            let view = encode(&formatter(false, "").view(&event));

            assert_eq!(view["occurred_at"], want, "{nanos} nanoseconds");
        }
    }
}
