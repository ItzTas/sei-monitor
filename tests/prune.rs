mod support;

use std::time::Duration;

use chrono::{DateTime, Utc};

use sei_monitor::event::{Event, Kind};
use sei_monitor::monitor::Clock;
use sei_monitor::store::Trigger;
use support::fake_sei::{FakeProcess, ROUTE_PROCESSES, UNIT_NUPROC};
use support::harness::{
    FIFTEEN_MINUTES, Harness, activity, bounded, database_store, populate, process, start, store_as,
};
use support::logs::{captured, logged};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn days(n: u32) -> Duration {
    DAY * n
}

fn highest_id(events: &[Event]) -> i64 {
    events.iter().map(|event| event.id).max().expect("events")
}

fn ids(events: &[Event]) -> Vec<i64> {
    events.iter().map(|event| event.id).collect()
}

fn due_soon(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| event.kind == Kind::ReturnDueSoon)
        .collect()
}

async fn procedure_ids(h: &Harness) -> Vec<i64> {
    h.store
        .processes(h.account_id())
        .await
        .expect("processes")
        .iter()
        .map(|row| row.procedure_id)
        .collect()
}

async fn after_a_change(h: &Harness) -> Vec<Event> {
    populate(&h.fake);
    h.prepare().await;
    h.run_once().await;
    h.clock.advance(FIFTEEN_MINUTES);
    h.fake.remove_process(UNIT_NUPROC, 9002);
    h.fake.add_activities(
        9001,
        vec![activity(
            50004,
            start() + chrono::Duration::minutes(10),
            "Documento assinado",
        )],
    );
    h.run_once().await;
    h.events().await
}

fn cutoff(h: &Harness, retention_days: i64) -> DateTime<Utc> {
    h.clock.now() - chrono::Duration::days(retention_days)
}

#[tokio::test]
async fn old_records_are_removed_without_repeating_events() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.retention_days = 30);
    let before = after_a_change(&h).await;
    assert!(!before.is_empty());
    h.monitor.acknowledge(highest_id(&before));

    h.clock.advance(days(31));
    h.run_once().await;

    let events = h.events().await;
    let limit = cutoff(&h, 30);
    assert!(
        events
            .iter()
            .all(|event| event.detected_at.is_some_and(|at| at >= limit)),
        "{events:?}"
    );
    assert!(
        before.iter().all(|old| !ids(&events).contains(&old.id)),
        "{events:?}"
    );
    assert_eq!(h.activity_refs().await, Vec::<String>::new());
    assert_eq!(procedure_ids(&h).await, [9001]);
    let documents = store
        .document_ids(h.account_id(), 9001)
        .await
        .expect("documents");
    assert_eq!(documents.len(), 3);
}

#[tokio::test]
async fn unread_events_are_kept_past_the_retention() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.retention_days = 30);
    let before = after_a_change(&h).await;
    assert!(!before.is_empty());

    h.clock.advance(days(31));
    h.run_once().await;

    let events = h.events().await;
    for old in &before {
        assert!(ids(&events).contains(&old.id), "{old:?} in {events:?}");
    }
    assert_eq!(procedure_ids(&h).await, [9001]);
}

#[tokio::test]
async fn acknowledged_due_soon_inside_retention_is_neither_deleted_nor_recreated() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| {
        config.retention_days = 4;
        config.return_warning_days = 2;
    });
    h.fake.set_processes(
        UNIT_NUPROC,
        vec![FakeProcess {
            return_date: "06/11/2026".to_owned(),
            ..process(9001, 1, "Contrato")
        }],
    );
    h.fake.add_activities(
        9001,
        vec![activity(
            50000,
            start() - chrono::Duration::hours(24),
            "Processo gerado",
        )],
    );
    h.prepare().await;
    h.run_once().await;

    h.clock.advance(Duration::from_secs(60 * 60));
    h.run_once().await;
    let events = h.events().await;
    let first = due_soon(&events);
    assert_eq!(first.len(), 1, "{events:?}");
    let id = first[0].id;
    h.monitor.acknowledge(id);

    h.clock.advance(days(1));
    h.run_once().await;
    let events = h.events().await;
    let kept = due_soon(&events);
    assert_eq!(kept.len(), 1, "{events:?}");
    assert_eq!(kept[0].id, id);

    h.clock.advance(days(5));
    h.run_once().await;
    h.clock.advance(FIFTEEN_MINUTES);
    h.run_once().await;
    let events = h.events().await;
    assert_eq!(due_soon(&events).len(), 0, "{events:?}");
}

#[tokio::test]
async fn retention_zero_keeps_everything() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.retention_days = 0);
    let before = after_a_change(&h).await;
    assert!(!before.is_empty());
    h.monitor.acknowledge(i64::MAX);

    h.clock.advance(days(400));
    h.run_once().await;

    let events = h.events().await;
    for old in &before {
        assert!(ids(&events).contains(&old.id), "{old:?} in {events:?}");
    }
    assert_eq!(procedure_ids(&h).await, [9001, 9002]);
}

#[tokio::test]
async fn acknowledge_never_moves_backwards() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |_| {});
    assert_eq!(h.monitor.acked_seq(), 0);

    h.monitor.acknowledge(5);
    h.monitor.acknowledge(3);
    h.monitor.acknowledge(-2);

    assert_eq!(h.monitor.acked_seq(), 5);
}

#[tokio::test]
async fn a_failed_cycle_removes_nothing() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.retention_days = 30);
    let before = after_a_change(&h).await;
    assert!(!before.is_empty());
    h.monitor.acknowledge(highest_id(&before));
    h.clock.advance(days(31));
    h.fake.fail_next(ROUTE_PROCESSES, 502, 1);

    bounded(h.monitor.run_once(Trigger::Scheduled))
        .await
        .expect_err("a cycle against a failing listing");

    let events = h.events().await;
    assert_eq!(ids(&events), ids(&before));
    assert_eq!(procedure_ids(&h).await, [9001, 9002]);
}

#[tokio::test]
async fn acknowledged_events_stay_until_the_cutoff_passes_them() {
    let Some((_database, store)) = database_store().await else {
        return;
    };
    let h = Harness::new(&store, |config| config.retention_days = 30);
    let before = after_a_change(&h).await;
    assert!(!before.is_empty());
    h.monitor.acknowledge(highest_id(&before));

    h.clock.advance(days(29) + Duration::from_secs(60 * 60));
    h.run_once().await;
    assert_eq!(ids(&h.events().await), ids(&before));
    assert_eq!(procedure_ids(&h).await, [9001, 9002]);

    h.clock.advance(days(1));
    h.run_once().await;
    assert_eq!(h.events().await, []);
    assert_eq!(procedure_ids(&h).await, [9001]);
}

#[tokio::test]
async fn a_failed_prune_leaves_the_cycle_successful() {
    let Some((database, store)) = database_store().await else {
        return;
    };
    let denied = store_as(&database, "sei_prune_denied", "DELETE ON events").await;
    let h = Harness::new(&denied, |config| config.retention_days = 30);
    populate(&h.fake);
    h.prepare().await;

    let (result, logs) = captured(bounded(h.monitor.run_once(Trigger::Scheduled))).await;

    assert_eq!(result.expect("a cycle whose prune fails").processes_seen, 2);
    let run = store
        .last_cycle_run(h.account_id())
        .await
        .expect("last run")
        .expect("a run");
    assert_eq!(run.error, "");
    assert_eq!(run.processes_seen, 2);
    assert!(run.finished_at.is_some());
    let failure = logged(&[
        ("error", "db error"),
        ("level", "WARN"),
        ("message", "could not remove old records"),
    ]);
    let message = (
        "message".to_owned(),
        "could not remove old records".to_owned(),
    );
    let prune_lines: Vec<_> = logs
        .iter()
        .filter(|event| event.contains(&message))
        .collect();
    assert_eq!(prune_lines, [&failure]);
}
