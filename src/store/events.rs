use std::collections::HashSet;
use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement, TransactionTrait,
};
use serde_json::error::Category;
use tracing::error;

use super::entity::events::{Column, Entity as StoredEvents, Model as StoredEvent};
use super::entity::processes;
use super::records::Pruned;
use super::{Store, StoreError, count};
use crate::event::{Event, Kind, Payload};

const LAST_ISSUED_SEQ: &str = "SELECT GREATEST(
    (SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM events_id_seq),
    (SELECT COALESCE(max(id), 0) FROM events)
)";

const NEWEST_EVENT_ID: &str = "SELECT COALESCE(max(id), 0) FROM events";

const REPORTED_LIMIT: usize = 1024;
const BATCH: i64 = 1000;
const WALK_LIMIT: u64 = 100_000;

#[derive(Default)]
struct Reported {
    ids: HashSet<i64>,
    full: bool,
}

#[derive(Clone, Default)]
pub(super) struct Skipped(Arc<Mutex<Reported>>);

enum Report {
    OneByOne,
    Full,
    Quiet,
}

impl Skipped {
    fn report(&self, event_id: i64) -> Report {
        let mut reported = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if reported.full || reported.ids.contains(&event_id) {
            return Report::Quiet;
        }
        if reported.ids.len() < REPORTED_LIMIT {
            reported.ids.insert(event_id);
            return Report::OneByOne;
        }
        reported.full = true;
        Report::Full
    }
}

#[derive(Debug, thiserror::Error)]
enum Unreadable {
    #[error("its kind is not one sei-monitor knows")]
    Kind,
    #[error("its payload is not an event payload: {problem} at line {line}, column {column}")]
    Payload {
        problem: &'static str,
        line: usize,
        column: usize,
    },
}

impl Store {
    pub async fn events_after(
        &self,
        account_id: i64,
        after: i64,
        limit: i64,
    ) -> Result<Vec<Event>, StoreError> {
        let mut events = Vec::new();
        let mut cursor = after;
        let mut walked: u64 = 0;
        loop {
            let wanted = (limit - count(events.len())).max(BATCH);
            let rows = StoredEvents::find()
                .filter(Column::AccountId.eq(account_id))
                .filter(Column::Id.gt(cursor))
                .order_by_asc(Column::Id)
                .limit(wanted.unsigned_abs())
                .all(self.connection())
                .await?;
            let fetched = count(rows.len());
            for row in rows {
                if count(events.len()) >= limit {
                    return Ok(events);
                }
                walked += 1;
                cursor = row.id;
                let Some(event) = self.readable(row) else {
                    continue;
                };
                events.push(event);
            }
            if fetched < wanted || count(events.len()) >= limit {
                return Ok(events);
            }
            if walked >= WALK_LIMIT {
                return Err(StoreError::TooManyUnreadableEvents(walked));
            }
        }
    }

    pub async fn last_issued_seq(&self) -> Result<i64, StoreError> {
        self.one_number(LAST_ISSUED_SEQ).await
    }

    pub async fn newest_event_id(&self) -> Result<i64, StoreError> {
        self.one_number(NEWEST_EVENT_ID).await
    }

    pub async fn prune_before(
        &self,
        account_id: i64,
        cutoff: DateTime<Utc>,
        acked_seq: i64,
    ) -> Result<Pruned, StoreError> {
        let transaction = self.connection().begin().await?;
        let events = StoredEvents::delete_many()
            .filter(Column::AccountId.eq(account_id))
            .filter(Column::DetectedAt.lt(cutoff))
            .filter(Column::Id.lte(acked_seq))
            .exec(&transaction)
            .await?
            .rows_affected;
        let processes = processes::Entity::delete_many()
            .filter(processes::Column::AccountId.eq(account_id))
            .filter(processes::Column::LeftAt.lt(cutoff))
            .exec(&transaction)
            .await?
            .rows_affected;
        transaction.commit().await?;
        Ok(Pruned { events, processes })
    }

    async fn one_number(&self, query: &str) -> Result<i64, StoreError> {
        let row = self
            .connection()
            .query_one_raw(Statement::from_string(DbBackend::Postgres, query))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound(query.to_owned()))?;
        Ok(row.try_get_by_index(0)?)
    }

    fn readable(&self, row: StoredEvent) -> Option<Event> {
        let (kind, payload) = match decode(&row.kind, &row.payload) {
            Ok(decoded) => decoded,
            Err(unreadable) => {
                self.skip(row.id, &unreadable);
                return None;
            }
        };
        Some(Event {
            id: row.id,
            account_id: row.account_id,
            kind,
            procedure_id: row.procedure_id,
            reference: row.reference,
            occurred_at: Some(row.occurred_at),
            detected_at: Some(row.detected_at),
            payload,
        })
    }

    fn skip(&self, event_id: i64, unreadable: &Unreadable) {
        match self.skipped.report(event_id) {
            Report::OneByOne => error!(
                event_id,
                error = unreadable as &dyn Error,
                "skipping a stored event that cannot be read"
            ),
            Report::Full => error!(
                limit = REPORTED_LIMIT,
                "more stored events cannot be read than are reported one by one"
            ),
            Report::Quiet => {}
        }
    }
}

fn decode(kind: &str, payload: &str) -> Result<(Kind, Payload), Unreadable> {
    let kind = Kind::parse(kind).ok_or(Unreadable::Kind)?;
    let payload = serde_json::from_str(payload).map_err(|error| payload_problem(&error))?;
    Ok((kind, payload))
}

fn payload_problem(error: &serde_json::Error) -> Unreadable {
    let problems = [
        (Category::Syntax, "it is not valid JSON"),
        (Category::Eof, "the JSON ends early"),
        (Category::Data, "a field has the wrong type"),
        (Category::Io, "it could not be read"),
    ];
    let (_, problem) = problems
        .into_iter()
        .find(|(category, _)| *category == error.classify())
        .expect("every JSON error category has a text");
    Unreadable::Payload {
        problem,
        line: error.line(),
        column: error.column(),
    }
}
