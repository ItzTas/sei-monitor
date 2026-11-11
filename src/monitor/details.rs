use std::collections::HashSet;
use std::error::Error;
use std::future::Future;

use chrono::{DateTime, Utc};
use futures_util::stream::{self, StreamExt, TryStreamExt};
use tracing::warn;

use super::MonitorError;
use super::diff::{
    Observation, Presence, ProcessChanges, activity_event, document_event, format_date,
    fresh_activities, fresh_documents,
};
use crate::sei::{Activity, Client, Document, ErrorKind, SeiError, Unit};
use crate::store::{CycleChanges, DocumentRow, ProcessRow, Store};

const ACTIVITY_PAGE_SIZE: usize = 20;
const DOCUMENT_PAGE_SIZE: usize = 50;
const MAX_ACTIVITY_PAGES: usize = 10;
const MAX_DOCUMENT_PAGES: usize = 200;

#[derive(Default)]
pub(super) struct Detail {
    skipped: bool,
    last_activity_id: i64,
    document_total: i64,
    activities: Vec<Activity>,
    documents: Vec<Document>,
    announce_documents: bool,
}

enum DocumentRead {
    Read {
        documents: Vec<Document>,
        total: i64,
    },
    Emptied {
        total: i64,
    },
}

pub(super) async fn observe_units(
    client: &Client,
    units: &[Unit],
    concurrency: usize,
) -> Result<Vec<Observation>, SeiError> {
    let listings: Vec<_> = units.iter().map(|unit| client.processes(unit.id)).collect();
    let lists = in_order(listings, concurrency).await?;
    Ok(units
        .iter()
        .zip(lists)
        .flat_map(|(unit, processes)| {
            processes.into_iter().map(|process| Observation {
                process,
                unit: unit.clone(),
            })
        })
        .collect())
}

pub(super) async fn fetch_details(
    store: &Store,
    client: &Client,
    account_id: i64,
    present: &[Presence],
    baseline: bool,
    concurrency: usize,
) -> Result<Vec<Detail>, MonitorError> {
    let reads: Vec<_> = present
        .iter()
        .map(|presence| detail_or_skip(store, client, account_id, presence, baseline))
        .collect();
    in_order(reads, concurrency).await
}

pub(super) fn build_changes(
    account_id: i64,
    run_id: i64,
    now: DateTime<Utc>,
    baseline: bool,
    diff: ProcessChanges,
    details: Vec<Detail>,
) -> CycleChanges {
    let mut changes = CycleChanges {
        account_id,
        run_id: Some(run_id),
        now,
        finished_at: now,
        baseline,
        left: diff.left.iter().map(|row| row.procedure_id).collect(),
        events: diff.events,
        ..CycleChanges::default()
    };
    for (presence, detail) in diff.present.iter().zip(details) {
        let detail = keep_previous_when_skipped(presence, detail);
        changes.processes.push(process_row(presence, &detail));
        changes.documents.extend(
            detail
                .documents
                .iter()
                .map(|document| document_row(presence, document)),
        );
        changes.events.extend(
            detail
                .activities
                .iter()
                .rev()
                .map(|activity| activity_event(presence, activity)),
        );
        if detail.announce_documents {
            changes.events.extend(
                detail
                    .documents
                    .iter()
                    .map(|document| document_event(presence, document)),
            );
        }
    }
    changes
}

async fn in_order<T, E, F>(calls: Vec<F>, concurrency: usize) -> Result<Vec<T>, E>
where
    F: Future<Output = Result<T, E>>,
{
    let indexed: Vec<_> = calls
        .into_iter()
        .enumerate()
        .map(|(index, call)| tagged(index, call))
        .collect();
    let mut finished: Vec<(usize, T)> = stream::iter(indexed)
        .buffer_unordered(concurrency)
        .try_collect()
        .await?;
    finished.sort_unstable_by_key(|(index, _)| *index);
    Ok(finished.into_iter().map(|(_, value)| value).collect())
}

async fn tagged<T, E>(
    index: usize,
    call: impl Future<Output = Result<T, E>>,
) -> Result<(usize, T), E> {
    call.await.map(|value| (index, value))
}

async fn detail_or_skip(
    store: &Store,
    client: &Client,
    account_id: i64,
    presence: &Presence,
    baseline: bool,
) -> Result<Detail, MonitorError> {
    match fetch_detail(store, client, account_id, presence, baseline).await {
        Err(MonitorError::Sei(error)) if error.kind() == Some(ErrorKind::Refused) => {
            warn!(
                process = %presence.observation.process.number,
                error = &error as &dyn Error,
                "SEI refused the details of a process; it keeps its previous state in this cycle"
            );
            Ok(Detail {
                skipped: true,
                ..Detail::default()
            })
        }
        read => read,
    }
}

async fn fetch_detail(
    store: &Store,
    client: &Client,
    account_id: i64,
    presence: &Presence,
    baseline: bool,
) -> Result<Detail, MonitorError> {
    let quiet = presence.quiet(baseline);
    let (last_seen, saved_total) = presence
        .previous
        .as_ref()
        .map_or((0, 0), |row| (row.last_activity_id, row.document_total));
    let (activities, last_activity_id) =
        read_activities(client, presence, last_seen, quiet).await?;
    if !quiet && activities.is_empty() {
        return Ok(Detail {
            last_activity_id,
            document_total: saved_total,
            ..Detail::default()
        });
    }
    let (known, (documents, document_total)) =
        tokio::try_join!(known_documents(store, account_id, presence), async {
            Ok(read_documents(client, presence, saved_total).await?)
        },)?;
    Ok(Detail {
        skipped: false,
        last_activity_id,
        document_total,
        activities,
        documents: fresh_documents(&known, &documents),
        announce_documents: !quiet,
    })
}

async fn known_documents(
    store: &Store,
    account_id: i64,
    presence: &Presence,
) -> Result<HashSet<i64>, MonitorError> {
    if presence.previous.is_none() {
        return Ok(HashSet::new());
    }
    let procedure_id = presence.observation.process.procedure_id;
    Ok(store.document_ids(account_id, procedure_id).await?)
}

async fn read_activities(
    client: &Client,
    presence: &Presence,
    last_seen: i64,
    quiet: bool,
) -> Result<(Vec<Activity>, i64), SeiError> {
    let (unit_id, procedure_id) = place(presence);
    if quiet {
        let first = client
            .activity_page(unit_id, procedure_id, 0, ACTIVITY_PAGE_SIZE)
            .await?;
        return Ok((Vec::new(), last_seen.max(highest_activity(&first.items))));
    }
    let mut collected = Vec::new();
    for page in 0..MAX_ACTIVITY_PAGES {
        let result = client
            .activity_page(unit_id, procedure_id, page, ACTIVITY_PAGE_SIZE)
            .await?;
        let (fresh, reached) = fresh_activities(last_seen, &result.items);
        collected.extend(fresh);
        if reached || result.items.len() < ACTIVITY_PAGE_SIZE {
            break;
        }
    }
    let last_id = last_seen.max(highest_activity(&collected));
    Ok((collected, last_id))
}

async fn read_documents(
    client: &Client,
    presence: &Presence,
    saved_total: i64,
) -> Result<(Vec<Document>, i64), SeiError> {
    let mut start = first_page(saved_total);
    loop {
        match read_document_pages(client, presence, start).await? {
            DocumentRead::Read { documents, total } => return Ok((documents, total)),
            DocumentRead::Emptied { total } => start = restart_page(start, total),
        }
    }
}

async fn read_document_pages(
    client: &Client,
    presence: &Presence,
    start: usize,
) -> Result<DocumentRead, SeiError> {
    let (unit_id, procedure_id) = place(presence);
    let mut documents = Vec::new();
    let mut total = 0;
    for page in start..start.saturating_add(MAX_DOCUMENT_PAGES) {
        let result = client
            .document_page(unit_id, procedure_id, page, DOCUMENT_PAGE_SIZE)
            .await?;
        if page == start && start > 0 && result.items.is_empty() {
            return Ok(DocumentRead::Emptied {
                total: result.total,
            });
        }
        let listed = result.items.len();
        documents.extend(result.items);
        total = result
            .total
            .max(count(page.saturating_mul(DOCUMENT_PAGE_SIZE) + listed));
        let next = count(page.saturating_add(1).saturating_mul(DOCUMENT_PAGE_SIZE));
        if listed < DOCUMENT_PAGE_SIZE || next >= result.total {
            break;
        }
    }
    Ok(DocumentRead::Read { documents, total })
}

fn first_page(total: i64) -> usize {
    if total <= 0 {
        return 0;
    }
    usize::try_from((total - 1) / count(DOCUMENT_PAGE_SIZE)).unwrap_or(usize::MAX)
}

fn restart_page(emptied: usize, total: i64) -> usize {
    let page = first_page(total);
    if page < emptied {
        return page;
    }
    0
}

fn place(presence: &Presence) -> (i64, i64) {
    (
        presence.observation.unit.id,
        presence.observation.process.procedure_id,
    )
}

fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn highest_activity(activities: &[Activity]) -> i64 {
    activities
        .iter()
        .map(|activity| activity.id)
        .fold(0, i64::max)
}

fn keep_previous_when_skipped(presence: &Presence, detail: Detail) -> Detail {
    let Some(previous) = presence.previous.as_ref().filter(|_| detail.skipped) else {
        return detail;
    };
    Detail {
        last_activity_id: previous.last_activity_id,
        document_total: previous.document_total,
        ..detail
    }
}

fn process_row(presence: &Presence, detail: &Detail) -> ProcessRow {
    let process = &presence.observation.process;
    let unit = &presence.observation.unit;
    let (assigned_user, assigned_user_id) = process
        .assignee
        .as_ref()
        .map_or((String::new(), 0), |user| (user.acronym.clone(), user.id));
    ProcessRow {
        procedure_id: process.procedure_id,
        number: process.number.clone(),
        process_type: process.process_type.clone(),
        description: process.description.clone(),
        unit_id: unit.id,
        unit_acronym: unit.acronym.clone(),
        assigned_user,
        assigned_user_id,
        return_date: format_date(process.return_date),
        return_overdue: process.return_overdue,
        last_activity_id: detail.last_activity_id,
        document_total: detail.document_total,
        first_seen_at: presence.previous.as_ref().and_then(|row| row.first_seen_at),
        last_seen_at: None,
        left_at: None,
    }
}

fn document_row(presence: &Presence, document: &Document) -> DocumentRow {
    DocumentRow {
        document_id: document.id,
        procedure_id: presence.observation.process.procedure_id,
        protocol: document.protocol.clone(),
        document_type: document.document_type.clone(),
        name: document.name.clone(),
    }
}
