use chrono::{DateTime, Utc};
use tokio_postgres::Row;

use super::records::{CycleRun, Trigger};
use super::{Store, StoreError};

const KEPT_RUNS_PER_ACCOUNT: i64 = 1000;

const START_CYCLE_RUN: &str = r#"INSERT INTO cycle_runs (account_id, "trigger", started_at)
VALUES ($1, $2, $3)
RETURNING id"#;

const FINISH_CYCLE_RUN: &str = "UPDATE cycle_runs
SET finished_at = $1, error = $2, processes_seen = $3, events_created = $4
WHERE id = $5 AND account_id = $6";

const PRUNE_CYCLE_RUNS: &str = "DELETE FROM cycle_runs AS runs
WHERE runs.account_id = $1 AND runs.id <= (
    SELECT kept.id FROM cycle_runs AS kept
    WHERE kept.account_id = $1
    ORDER BY kept.id DESC
    LIMIT 1 OFFSET $2::bigint
)";

const GET_LAST_CYCLE_RUN: &str =
    "SELECT * FROM cycle_runs WHERE account_id = $1 ORDER BY id DESC LIMIT 1";

impl Store {
    pub async fn start_cycle_run(
        &self,
        account_id: i64,
        trigger: Trigger,
        started_at: DateTime<Utc>,
    ) -> Result<i64, StoreError> {
        let row = self
            .client()
            .await?
            .query_one(
                START_CYCLE_RUN,
                &[&account_id, &trigger.as_str(), &started_at],
            )
            .await?;
        Ok(row.try_get("id")?)
    }

    pub async fn finish_cycle_run(
        &self,
        run_id: i64,
        account_id: i64,
        finished_at: DateTime<Utc>,
        error: &str,
    ) -> Result<(), StoreError> {
        let mut client = self.client().await?;
        let transaction = client.transaction().await?;
        transaction
            .execute(
                FINISH_CYCLE_RUN,
                &[&finished_at, &error, &0_i64, &0_i64, &run_id, &account_id],
            )
            .await?;
        transaction
            .execute(PRUNE_CYCLE_RUNS, &[&account_id, &KEPT_RUNS_PER_ACCOUNT])
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn last_cycle_run(&self, account_id: i64) -> Result<Option<CycleRun>, StoreError> {
        let row = self
            .client()
            .await?
            .query_opt(GET_LAST_CYCLE_RUN, &[&account_id])
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(cycle_run(&row)?))
    }
}

fn cycle_run(row: &Row) -> Result<CycleRun, tokio_postgres::Error> {
    Ok(CycleRun {
        id: row.try_get("id")?,
        account_id: row.try_get("account_id")?,
        trigger: row.try_get("trigger")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        error: row.try_get("error")?,
        processes_seen: row.try_get("processes_seen")?,
        events_created: row.try_get("events_created")?,
    })
}
