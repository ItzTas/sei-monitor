use std::collections::HashSet;

use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, DbBackend, EntityTrait,
    FromQueryResult, QueryFilter, QuerySelect, Statement, TransactionTrait, TryInsertResult, Value,
};

use super::accounts::lock_account;
use super::entity::{accounts, documents, events, processes};
use super::records::{CycleChanges, ProcessRow};
use super::runs::finish_run;
use super::{Store, StoreError, count};
use crate::event::{Event, Payload};

// The scheduler tests find this read in pg_stat_activity by its text.
const LIST_ACCOUNT_PROCESSES: &str =
    "SELECT * FROM processes WHERE account_id = $1 ORDER BY procedure_id";

// Kept as written: the update leaves first_seen_at alone and clears left_at.
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

impl Store {
    pub async fn processes(&self, account_id: i64) -> Result<Vec<ProcessRow>, StoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            LIST_ACCOUNT_PROCESSES,
            [account_id.into()],
        );
        let rows = processes::Model::find_by_statement(statement)
            .all(self.connection())
            .await?;
        Ok(rows.into_iter().map(ProcessRow::from).collect())
    }

    pub async fn document_ids(
        &self,
        account_id: i64,
        procedure_id: i64,
    ) -> Result<HashSet<i64>, StoreError> {
        let ids: Vec<i64> = documents::Entity::find()
            .select_only()
            .column(documents::Column::DocumentId)
            .filter(documents::Column::AccountId.eq(account_id))
            .filter(documents::Column::ProcedureId.eq(procedure_id))
            .into_tuple()
            .all(self.connection())
            .await?;
        Ok(ids.into_iter().collect())
    }

    pub async fn apply_cycle(&self, changes: &CycleChanges) -> Result<Vec<Event>, StoreError> {
        let transaction = self.connection().begin().await?;
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
        // Only after the commit: the hub answers a signal by reading Events, and the signal
        // carries a stored id, never a sequence value an insert lost to a conflict.
        if let Some(newest) = inserted.iter().map(|event| event.id).max() {
            self.notifier().notify(newest);
        }
        Ok(inserted)
    }
}

async fn upsert_processes(
    transaction: &DatabaseTransaction,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    for process in &changes.processes {
        let first_seen_at = process.first_seen_at.unwrap_or(changes.now);
        let values: [Value; 15] = [
            changes.account_id.into(),
            process.procedure_id.into(),
            process.number.as_str().into(),
            process.process_type.as_str().into(),
            process.description.as_str().into(),
            process.unit_id.into(),
            process.unit_acronym.as_str().into(),
            process.assigned_user.as_str().into(),
            process.assigned_user_id.into(),
            process.return_date.as_str().into(),
            process.return_overdue.into(),
            process.last_activity_id.into(),
            process.document_total.into(),
            first_seen_at.into(),
            changes.now.into(),
        ];
        let statement = Statement::from_sql_and_values(DbBackend::Postgres, UPSERT_PROCESS, values);
        transaction.execute_raw(statement).await?;
    }
    Ok(())
}

async fn mark_left(
    transaction: &DatabaseTransaction,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    for procedure_id in &changes.left {
        processes::Entity::update_many()
            .col_expr(processes::Column::LeftAt, Expr::value(changes.now))
            .filter(processes::Column::AccountId.eq(changes.account_id))
            .filter(processes::Column::ProcedureId.eq(*procedure_id))
            .filter(processes::Column::LeftAt.is_null())
            .exec(transaction)
            .await?;
    }
    Ok(())
}

async fn insert_documents(
    transaction: &DatabaseTransaction,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    for document in &changes.documents {
        let row = documents::ActiveModel {
            account_id: ActiveValue::Set(changes.account_id),
            document_id: ActiveValue::Set(document.document_id),
            procedure_id: ActiveValue::Set(document.procedure_id),
            formatted_protocol: ActiveValue::Set(document.protocol.clone()),
            document_type: ActiveValue::Set(document.document_type.clone()),
            name: ActiveValue::Set(document.name.clone()),
            first_seen_at: ActiveValue::Set(changes.now),
        };
        documents::Entity::insert(row)
            .on_conflict_do_nothing_on([
                documents::Column::AccountId,
                documents::Column::DocumentId,
            ])
            .exec_without_returning(transaction)
            .await?;
    }
    Ok(())
}

async fn mark_baseline(
    transaction: &DatabaseTransaction,
    changes: &CycleChanges,
) -> Result<(), StoreError> {
    if !changes.baseline {
        return Ok(());
    }
    accounts::Entity::update_many()
        .col_expr(accounts::Column::BaselineAt, Expr::value(changes.now))
        .filter(accounts::Column::Id.eq(changes.account_id))
        .filter(accounts::Column::BaselineAt.is_null())
        .exec(transaction)
        .await?;
    Ok(())
}

async fn insert_events(
    transaction: &DatabaseTransaction,
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
    transaction: &DatabaseTransaction,
    changes: &CycleChanges,
    event: &Event,
) -> Result<Option<Event>, StoreError> {
    let detected_at = event.detected_at.unwrap_or(changes.now);
    let occurred_at = event.occurred_at.unwrap_or(detected_at);
    let row = events::ActiveModel {
        id: ActiveValue::NotSet,
        account_id: ActiveValue::Set(changes.account_id),
        kind: ActiveValue::Set(event.kind.as_str().to_owned()),
        procedure_id: ActiveValue::Set(event.procedure_id),
        reference: ActiveValue::Set(event.reference.clone()),
        occurred_at: ActiveValue::Set(occurred_at),
        detected_at: ActiveValue::Set(detected_at),
        payload: ActiveValue::Set(go_json(&event.payload)),
    };
    let outcome = events::Entity::insert(row)
        .on_conflict_do_nothing_on([
            events::Column::AccountId,
            events::Column::Kind,
            events::Column::Reference,
        ])
        .exec(transaction)
        .await?;
    let TryInsertResult::Inserted(saved) = outcome else {
        return Ok(None);
    };
    Ok(Some(Event {
        id: saved.last_insert_id,
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
