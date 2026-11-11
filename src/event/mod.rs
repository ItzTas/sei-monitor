mod format;
mod view;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use format::Formatter;
pub use view::{View, ViewDetail};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    ProcessEntered,
    ProcessLeft,
    ActivityAdded,
    DocumentAdded,
    AssignmentChanged,
    ReturnDueSoon,
    ReturnOverdue,
    AccountLoginFailed,
}

const KINDS: [(Kind, &str, &str); 8] = [
    (
        Kind::ProcessEntered,
        "process_entered",
        "A process appeared in a watched unit",
    ),
    (
        Kind::ProcessLeft,
        "process_left",
        "A process is no longer listed in the watched units",
    ),
    (
        Kind::ActivityAdded,
        "activity_added",
        "A new activity was recorded on a process",
    ),
    (
        Kind::DocumentAdded,
        "document_added",
        "A new document was added to a process",
    ),
    (
        Kind::AssignmentChanged,
        "assignment_changed",
        "A process was assigned to someone else or to nobody",
    ),
    (
        Kind::ReturnDueSoon,
        "return_due_soon",
        "A scheduled return is due within RETURN_WARNING_DAYS days",
    ),
    (
        Kind::ReturnOverdue,
        "return_overdue",
        "A scheduled return is overdue",
    ),
    (
        Kind::AccountLoginFailed,
        "account_login_failed",
        "SEI rejected the login and monitoring is paused",
    ),
];

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::ProcessEntered,
        Kind::ProcessLeft,
        Kind::ActivityAdded,
        Kind::DocumentAdded,
        Kind::AssignmentChanged,
        Kind::ReturnDueSoon,
        Kind::ReturnOverdue,
        Kind::AccountLoginFailed,
    ];

    fn row(self) -> &'static (Kind, &'static str, &'static str) {
        KINDS
            .iter()
            .find(|(kind, _, _)| *kind == self)
            .expect("every kind has a row")
    }

    pub fn as_str(self) -> &'static str {
        self.row().1
    }

    pub fn parse(text: &str) -> Option<Kind> {
        KINDS
            .iter()
            .find(|(_, name, _)| *name == text)
            .map(|(kind, _, _)| *kind)
    }

    pub fn urgent(self) -> bool {
        matches!(self, Kind::AccountLoginFailed | Kind::ReturnOverdue)
    }

    pub fn description(self) -> &'static str {
        self.row().2
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Payload {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub process_number: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unit: String,
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
    #[serde(skip_serializing_if = "String::is_empty")]
    pub document_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub document_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub document_protocol: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub assigned_user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub assigned_user_name: String,
    #[serde(
        rename = "previous_assigned_user",
        skip_serializing_if = "String::is_empty"
    )]
    pub previous_assignee: String,
    #[serde(skip_serializing_if = "is_false")]
    pub assigned_to_self: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub return_date: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: i64,
    pub account_id: i64,
    pub kind: Kind,
    pub procedure_id: i64,
    pub reference: String,
    pub occurred_at: Option<DateTime<Utc>>,
    pub detected_at: Option<DateTime<Utc>>,
    pub payload: Payload,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_go_names() {
        let names = [
            "process_entered",
            "process_left",
            "activity_added",
            "document_added",
            "assignment_changed",
            "return_due_soon",
            "return_overdue",
            "account_login_failed",
        ];
        for (kind, name) in Kind::ALL.into_iter().zip(names) {
            assert_eq!(kind.as_str(), name);
            assert_eq!(Kind::parse(name), Some(kind));
        }
        assert_eq!(Kind::parse("nonsense"), None);
        assert_eq!(Kind::parse(""), None);
    }

    #[test]
    fn only_login_failures_and_overdue_returns_are_urgent() {
        let urgent: Vec<Kind> = Kind::ALL.into_iter().filter(|kind| kind.urgent()).collect();
        assert_eq!(urgent, [Kind::ReturnOverdue, Kind::AccountLoginFailed]);
    }

    #[test]
    fn every_kind_has_its_description() {
        let descriptions = [
            "A process appeared in a watched unit",
            "A process is no longer listed in the watched units",
            "A new activity was recorded on a process",
            "A new document was added to a process",
            "A process was assigned to someone else or to nobody",
            "A scheduled return is due within RETURN_WARNING_DAYS days",
            "A scheduled return is overdue",
            "SEI rejected the login and monitoring is paused",
        ];
        for (kind, text) in Kind::ALL.into_iter().zip(descriptions) {
            assert_eq!(kind.description(), text);
        }
    }

    #[test]
    fn go_written_payloads_are_read() {
        let payload: Payload =
            serde_json::from_str(r#"{"process_number":"a<b","unknown":1}"#).unwrap();
        assert_eq!(
            payload,
            Payload {
                process_number: "a<b".into(),
                ..Payload::default()
            }
        );
    }

    #[test]
    fn payloads_are_written_with_go_names_and_omissions() {
        let payload = Payload {
            unit: "U".into(),
            previous_assignee: "x".into(),
            assigned_to_self: true,
            ..Payload::default()
        };
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            r#"{"unit":"U","previous_assigned_user":"x","assigned_to_self":true}"#
        );
        assert_eq!(serde_json::to_string(&Payload::default()).unwrap(), "{}");
    }
}
