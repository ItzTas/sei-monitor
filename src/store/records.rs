use std::error::Error;

use chrono::{DateTime, Utc};
use tokio_postgres::types::{FromSql, Type};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Scheduled,
    Manual,
}

impl Trigger {
    const ALL: [Trigger; 2] = [Trigger::Scheduled, Trigger::Manual];

    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Scheduled => "scheduled",
            Trigger::Manual => "manual",
        }
    }
}

impl<'a> FromSql<'a> for Trigger {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Trigger, Box<dyn Error + Sync + Send>> {
        let text = <&str as FromSql>::from_sql(ty, raw)?;
        Trigger::ALL
            .into_iter()
            .find(|trigger| trigger.as_str() == text)
            .ok_or_else(|| format!("unknown cycle trigger {text:?}").into())
    }

    fn accepts(ty: &Type) -> bool {
        <&str as FromSql>::accepts(ty)
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
