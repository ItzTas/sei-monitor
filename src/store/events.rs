use chrono::{DateTime, Utc};
use tokio_postgres::Row;

use super::records::Pruned;
use super::{Store, StoreError};
use crate::event::{Event, Kind};

const EVENTS_AFTER: &str =
    "SELECT id, account_id, kind, procedure_id, ref, occurred_at, detected_at, payload
FROM events WHERE account_id = $1 AND id > $2 ORDER BY id LIMIT $3";

const PRUNE_EVENTS_BEFORE: &str =
    "DELETE FROM events WHERE account_id = $1 AND detected_at < $2 AND id <= $3";

const PRUNE_PROCESSES_LEFT_BEFORE: &str =
    "DELETE FROM processes WHERE account_id = $1 AND left_at < $2";

impl Store {
    pub async fn events_after(
        &self,
        account_id: i64,
        after: i64,
        limit: i64,
    ) -> Result<Vec<Event>, StoreError> {
        let rows = self
            .client()
            .await?
            .query(EVENTS_AFTER, &[&account_id, &after, &limit])
            .await?;
        rows.iter().map(stored_event).collect()
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
}

fn stored_event(row: &Row) -> Result<Event, StoreError> {
    let id: i64 = row.try_get("id")?;
    let kind: &str = row.try_get("kind")?;
    let kind = Kind::parse(kind).ok_or_else(|| unreadable(id, format!("unknown kind {kind:?}")))?;
    let payload: &str = row.try_get("payload")?;
    let payload =
        serde_json::from_str(payload).map_err(|error| unreadable(id, error.to_string()))?;
    Ok(Event {
        id,
        account_id: row.try_get("account_id")?,
        kind,
        procedure_id: row.try_get("procedure_id")?,
        reference: row.try_get("ref")?,
        occurred_at: Some(row.try_get("occurred_at")?),
        detected_at: Some(row.try_get("detected_at")?),
        payload,
    })
}

fn unreadable(event_id: i64, detail: String) -> StoreError {
    StoreError::Payload { event_id, detail }
}
