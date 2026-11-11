use std::collections::HashSet;

use tokio_postgres::{Row, Transaction};

use super::accounts::lock_account;
use super::records::{CycleChanges, ProcessRow};
use super::runs::finish_run;
use super::{Store, StoreError};
use crate::event::{Event, Payload};

const LIST_ACCOUNT_PROCESSES: &str =
    "SELECT * FROM processes WHERE account_id = $1 ORDER BY procedure_id";

const UPSERT_PROCESS: &str = "INSERT INTO processes (account_id, procedure_id, number, process_type, description, unit_id, unit_acronym,
                       assigned_user, assigned_user_id, return_date, return_overdue, last_activity_id,
                       document_total, first_seen_at, last_seen_at)
VALUES ($1, $2, $3, $4, $5, $6, $7,
        $8, $9, $10, $11, $12,
        $13, $14, $15)
ON CONFLICT (account_id, procedure_id) DO UPDATE SET
    number = excluded.number,
    process_type = excluded.process_type,
    description = excluded.description,
    unit_id = excluded.unit_id,
    unit_acronym = excluded.unit_acronym,
    assigned_user = excluded.assigned_user,
    assigned_user_id = excluded.assigned_user_id,
    return_date = excluded.return_date,
    return_overdue = excluded.return_overdue,
    last_activity_id = excluded.last_activity_id,
    document_total = excluded.document_total,
    last_seen_at = excluded.last_seen_at,
    left_at = NULL";

const MARK_PROCESS_LEFT: &str = "UPDATE processes SET left_at = $1
WHERE account_id = $2 AND procedure_id = $3 AND left_at IS NULL";

const LIST_DOCUMENT_IDS: &str =
    "SELECT document_id FROM documents WHERE account_id = $1 AND procedure_id = $2";

const INSERT_DOCUMENT: &str = "INSERT INTO documents (account_id, document_id, procedure_id, formatted_protocol, document_type, name, first_seen_at)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (account_id, document_id) DO NOTHING";

const MARK_BASELINE: &str =
    "UPDATE accounts SET baseline_at = $1 WHERE id = $2 AND baseline_at IS NULL";

const INSERT_EVENT: &str =
    "INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (account_id, kind, ref) DO NOTHING
RETURNING id";

impl Store {
    pub async fn processes(&self, account_id: i64) -> Result<Vec<ProcessRow>, StoreError> {
        let rows = self
            .client()
            .await?
            .query(LIST_ACCOUNT_PROCESSES, &[&account_id])
            .await?;
        Ok(rows.iter().map(process_row).collect::<Result<_, _>>()?)
    }

    pub async fn document_ids(
        &self,
        account_id: i64,
        procedure_id: i64,
    ) -> Result<HashSet<i64>, StoreError> {
        let rows = self
            .client()
            .await?
            .query(LIST_DOCUMENT_IDS, &[&account_id, &procedure_id])
            .await?;
        Ok(rows
            .iter()
            .map(|row| row.try_get("document_id"))
            .collect::<Result<_, _>>()?)
    }

    pub async fn apply_cycle(&self, changes: &CycleChanges) -> Result<Vec<Event>, StoreError> {
        let mut client = self.client().await?;
        let transaction = client.transaction().await?;
        lock_account(&transaction, changes.account_id).await?;
        upsert_processes(&transaction, changes).await?;
        mark_left(&transaction, changes).await?;
        insert_documents(&transaction, changes).await?;
        mark_baseline(&transaction, changes).await?;
        let inserted = insert_events(&transaction, changes).await?;
        if let Some(run_id) = changes.run_id {
            finish_run(
                &transaction,
                run_id,
                changes.account_id,
                changes.finished_at,
                "",
                count(changes.processes.len()),
                count(inserted.len()),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(inserted)
    }
}

async fn upsert_processes(
    transaction: &Transaction<'_>,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    for process in &changes.processes {
        let first_seen_at = process.first_seen_at.unwrap_or(changes.now);
        transaction
            .execute(
                UPSERT_PROCESS,
                &[
                    &changes.account_id,
                    &process.procedure_id,
                    &process.number,
                    &process.process_type,
                    &process.description,
                    &process.unit_id,
                    &process.unit_acronym,
                    &process.assigned_user,
                    &process.assigned_user_id,
                    &process.return_date,
                    &process.return_overdue,
                    &process.last_activity_id,
                    &process.document_total,
                    &first_seen_at,
                    &changes.now,
                ],
            )
            .await?;
    }
    Ok(())
}

async fn mark_left(
    transaction: &Transaction<'_>,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    for procedure_id in &changes.left {
        transaction
            .execute(
                MARK_PROCESS_LEFT,
                &[&changes.now, &changes.account_id, procedure_id],
            )
            .await?;
    }
    Ok(())
}

async fn insert_documents(
    transaction: &Transaction<'_>,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    for document in &changes.documents {
        transaction
            .execute(
                INSERT_DOCUMENT,
                &[
                    &changes.account_id,
                    &document.document_id,
                    &document.procedure_id,
                    &document.protocol,
                    &document.document_type,
                    &document.name,
                    &changes.now,
                ],
            )
            .await?;
    }
    Ok(())
}

async fn mark_baseline(
    transaction: &Transaction<'_>,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    if !changes.baseline {
        return Ok(());
    }
    transaction
        .execute(MARK_BASELINE, &[&changes.now, &changes.account_id])
        .await?;
    Ok(())
}

async fn insert_events(
    transaction: &Transaction<'_>,
    changes: &CycleChanges,
) -> Result<Vec<Event>, StoreError> {
    let mut inserted = Vec::new();
    for event in &changes.events {
        let Some(saved) = insert_event(transaction, changes, event).await? else {
            continue;
        };
        inserted.push(saved);
    }
    Ok(inserted)
}

async fn insert_event(
    transaction: &Transaction<'_>,
    changes: &CycleChanges,
    event: &Event,
) -> Result<Option<Event>, StoreError> {
    let detected_at = event.detected_at.unwrap_or(changes.now);
    let occurred_at = event.occurred_at.unwrap_or(detected_at);
    let row = transaction
        .query_opt(
            INSERT_EVENT,
            &[
                &changes.account_id,
                &event.kind.as_str(),
                &event.procedure_id,
                &event.reference,
                &occurred_at,
                &detected_at,
                &go_json(&event.payload),
            ],
        )
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(Event {
        id: row.try_get("id")?,
        account_id: changes.account_id,
        occurred_at: Some(occurred_at),
        detected_at: Some(detected_at),
        ..event.clone()
    }))
}

fn go_json(payload: &Payload) -> String {
    let text = serde_json::to_string(payload).expect("a payload holds only text and a flag");
    let escapes = [
        ('<', "\\u003c"),
        ('>', "\\u003e"),
        ('&', "\\u0026"),
        ('\u{2028}', "\\u2028"),
        ('\u{2029}', "\\u2029"),
    ];
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        let Some((_, code)) = escapes.iter().find(|(raw, _)| *raw == character) else {
            escaped.push(character);
            continue;
        };
        escaped.push_str(code);
    }
    escaped
}

fn count(total: usize) -> i64 {
    i64::try_from(total).unwrap_or(i64::MAX)
}

fn process_row(row: &Row) -> Result<ProcessRow, tokio_postgres::Error> {
    Ok(ProcessRow {
        procedure_id: row.try_get("procedure_id")?,
        number: row.try_get("number")?,
        process_type: row.try_get("process_type")?,
        description: row.try_get("description")?,
        unit_id: row.try_get("unit_id")?,
        unit_acronym: row.try_get("unit_acronym")?,
        assigned_user: row.try_get("assigned_user")?,
        assigned_user_id: row.try_get("assigned_user_id")?,
        return_date: row.try_get("return_date")?,
        return_overdue: row.try_get("return_overdue")?,
        last_activity_id: row.try_get("last_activity_id")?,
        document_total: row.try_get("document_total")?,
        first_seen_at: row.try_get("first_seen_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        left_at: row.try_get("left_at")?,
    })
}
