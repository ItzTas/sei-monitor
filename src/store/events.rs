use std::collections::HashSet;
use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use serde_json::error::Category;
use tokio_postgres::Row;
use tracing::error;

use super::records::Pruned;
use super::{Store, StoreError, count};
use crate::event::{Event, Kind, Payload};

const EVENTS_AFTER: &str =
    "SELECT id, account_id, kind, procedure_id, ref, occurred_at, detected_at, payload
FROM events WHERE account_id = $1 AND id > $2 ORDER BY id LIMIT $3";

const LAST_ISSUED_SEQ: &str = "SELECT GREATEST(
    (SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM events_id_seq),
    (SELECT COALESCE(max(id), 0) FROM events)
)";

const NEWEST_EVENT_ID: &str = "SELECT COALESCE(max(id), 0) FROM events";

const PRUNE_EVENTS_BEFORE: &str =
    "DELETE FROM events WHERE account_id = $1 AND detected_at < $2 AND id <= $3";

const PRUNE_PROCESSES_LEFT_BEFORE: &str =
    "DELETE FROM processes WHERE account_id = $1 AND left_at < $2";

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
        let client = self.client().await?;
        let mut events = Vec::new();
        let mut cursor = after;
        let mut walked: u64 = 0;
        loop {
            let wanted = (limit - count(events.len())).max(BATCH);
            let rows = client
                .query(EVENTS_AFTER, &[&account_id, &cursor, &wanted])
                .await?;
            for row in &rows {
                if count(events.len()) >= limit {
                    return Ok(events);
                }
                walked += 1;
                cursor = row.try_get("id")?;
                let Some(event) = self.readable(row, cursor)? else {
                    continue;
                };
                events.push(event);
            }
            if count(rows.len()) < wanted || count(events.len()) >= limit {
                return Ok(events);
            }
            if walked >= WALK_LIMIT {
                return Err(StoreError::TooManyUnreadableEvents(walked));
            }
        }
    }

    pub async fn last_issued_seq(&self) -> Result<i64, StoreError> {
        let row = self.client().await?.query_one(LAST_ISSUED_SEQ, &[]).await?;
        Ok(row.try_get(0)?)
    }

    pub async fn newest_event_id(&self) -> Result<i64, StoreError> {
        let row = self.client().await?.query_one(NEWEST_EVENT_ID, &[]).await?;
        Ok(row.try_get(0)?)
    }

    pub async fn prune_before(
        &self,
        account_id: i64,
        cutoff: DateTime<Utc>,
        acked_seq: i64,
    ) -> Result<Pruned, StoreError> {
        let mut client = self.client().await?;
        let transaction = client.transaction().await?;
        let events = transaction
            .execute(PRUNE_EVENTS_BEFORE, &[&account_id, &cutoff, &acked_seq])
            .await?;
        let processes = transaction
            .execute(PRUNE_PROCESSES_LEFT_BEFORE, &[&account_id, &cutoff])
            .await?;
        transaction.commit().await?;
        Ok(Pruned { events, processes })
    }

    fn readable(&self, row: &Row, id: i64) -> Result<Option<Event>, StoreError> {
        let (kind, payload) = match decode(row.try_get("kind")?, row.try_get("payload")?) {
            Ok(decoded) => decoded,
            Err(unreadable) => {
                self.skip(id, &unreadable);
                return Ok(None);
            }
        };
        Ok(Some(Event {
            id,
            account_id: row.try_get("account_id")?,
            kind,
            procedure_id: row.try_get("procedure_id")?,
            reference: row.try_get("ref")?,
            occurred_at: Some(row.try_get("occurred_at")?),
            detected_at: Some(row.try_get("detected_at")?),
            payload,
        }))
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
