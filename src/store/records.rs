use chrono::{DateTime, Utc};
use sea_orm::{DeriveActiveEnum, EnumIter};

use super::entity::{accounts, cycle_runs, processes};
use crate::event::Event;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountKey {
    pub sei_base_url: String,
    pub username: String,
    pub org_id: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountInfo {
    pub org_acronym: String,
    pub sei_version: String,
    pub wssei_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: i64,
    pub key: AccountKey,
    pub created_at: DateTime<Utc>,
    pub baseline_at: Option<DateTime<Utc>>,
    pub paused_reason: String,
    pub info: AccountInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum Trigger {
    #[sea_orm(string_value = "scheduled")]
    Scheduled,
    #[sea_orm(string_value = "manual")]
    Manual,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Scheduled => "scheduled",
            Trigger::Manual => "manual",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CycleRun {
    pub id: i64,
    pub account_id: i64,
    pub trigger: Trigger,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: String,
    pub processes_seen: i64,
    pub events_created: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessRow {
    pub procedure_id: i64,
    pub number: String,
    pub process_type: String,
    pub description: String,
    pub unit_id: i64,
    pub unit_acronym: String,
    pub assigned_user: String,
    pub assigned_user_id: i64,
    pub return_date: String,
    pub return_overdue: bool,
    pub last_activity_id: i64,
    pub document_total: i64,
    pub first_seen_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub left_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentRow {
    pub document_id: i64,
    pub procedure_id: i64,
    pub protocol: String,
    pub document_type: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct CycleChanges {
    pub account_id: i64,
    pub run_id: Option<i64>,
    pub now: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub baseline: bool,
    pub processes: Vec<ProcessRow>,
    pub left: Vec<i64>,
    pub documents: Vec<DocumentRow>,
    pub events: Vec<Event>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub events: u64,
    pub processes: u64,
}

impl From<accounts::Model> for Account {
    fn from(row: accounts::Model) -> Account {
        Account {
            id: row.id,
            key: AccountKey {
                sei_base_url: row.sei_base_url,
                username: row.username,
                org_id: row.org_id,
            },
            created_at: row.created_at,
            baseline_at: row.baseline_at,
            paused_reason: row.paused_reason,
            info: AccountInfo {
                org_acronym: row.org_acronym,
                sei_version: row.sei_version,
                wssei_version: row.wssei_version,
            },
        }
    }
}

impl From<processes::Model> for ProcessRow {
    fn from(row: processes::Model) -> ProcessRow {
        ProcessRow {
            procedure_id: row.procedure_id,
            number: row.number,
            process_type: row.process_type,
            description: row.description,
            unit_id: row.unit_id,
            unit_acronym: row.unit_acronym,
            assigned_user: row.assigned_user,
            assigned_user_id: row.assigned_user_id,
            return_date: row.return_date,
            return_overdue: row.return_overdue,
            last_activity_id: row.last_activity_id,
            document_total: row.document_total,
            first_seen_at: Some(row.first_seen_at),
            last_seen_at: Some(row.last_seen_at),
            left_at: row.left_at,
        }
    }
}

impl From<cycle_runs::Model> for CycleRun {
    fn from(row: cycle_runs::Model) -> CycleRun {
        CycleRun {
            id: row.id,
            account_id: row.account_id,
            trigger: row.trigger,
            started_at: row.started_at,
            finished_at: row.finished_at,
            error: row.error,
            processes_seen: row.processes_seen,
            events_created: row.events_created,
        }
    }
}
