use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, EntityTrait, QueryFilter,
    QueryOrder, Select, Statement, TransactionTrait,
};

use super::entity::cycle_runs;
use super::records::{CycleRun, Trigger};
use super::{Store, StoreError};

const KEPT_RUNS_PER_ACCOUNT: i64 = 1000;

// The scheduler tests find this insert in pg_stat_activity by its text.
const START_CYCLE_RUN: &str = r#"INSERT INTO cycle_runs (account_id, "trigger", started_at)
VALUES ($1, $2, $3)
RETURNING id"#;

const PRUNE_CYCLE_RUNS: &str = "DELETE FROM cycle_runs AS runs
WHERE runs.account_id = $1 AND runs.id <= (
    SELECT kept.id FROM cycle_runs AS kept
    WHERE kept.account_id = $1
    ORDER BY kept.id DESC
    LIMIT 1 OFFSET $2::bigint
)";

impl Store {
    pub async fn start_cycle_run(
        &self,
        account_id: i64,
        trigger: Trigger,
        started_at: DateTime<Utc>,
    ) -> Result<i64, StoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            START_CYCLE_RUN,
            [
                account_id.into(),
                trigger.as_str().into(),
                started_at.into(),
            ],
        );
        let row = self
            .connection()
            .query_one_raw(statement)
            .await?
            .ok_or(DbErr::RecordNotInserted)?;
        Ok(row.try_get("", "id")?)
    }

    pub async fn finish_cycle_run(
        &self,
        run_id: i64,
        account_id: i64,
        finished_at: DateTime<Utc>,
        error: &str,
    ) -> Result<(), StoreError> {
        let transaction = self.connection().begin().await?;
        finish_run(&transaction, run_id, account_id, finished_at, error, 0, 0).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn last_cycle_run(&self, account_id: i64) -> Result<Option<CycleRun>, StoreError> {
        self.one_run(cycle_runs::Entity::find(), account_id).await
    }

    pub async fn last_successful_cycle_run(
        &self,
        account_id: i64,
    ) -> Result<Option<CycleRun>, StoreError> {
        let successful = cycle_runs::Entity::find()
            .filter(cycle_runs::Column::Error.eq(""))
            .filter(cycle_runs::Column::FinishedAt.is_not_null());
        self.one_run(successful, account_id).await
    }

    async fn one_run(
        &self,
        runs: Select<cycle_runs::Entity>,
        account_id: i64,
    ) -> Result<Option<CycleRun>, StoreError> {
        let run = runs
            .filter(cycle_runs::Column::AccountId.eq(account_id))
            .order_by_desc(cycle_runs::Column::Id)
            .one(self.connection())
            .await?;
        Ok(run.map(CycleRun::from))
    }
}

pub(super) async fn finish_run(
    transaction: &DatabaseTransaction,
    run_id: i64,
    account_id: i64,
    finished_at: DateTime<Utc>,
    error: &str,
    processes_seen: i64,
    events_created: i64,
) -> Result<(), StoreError> {
    cycle_runs::Entity::update_many()
        .col_expr(cycle_runs::Column::FinishedAt, Expr::value(finished_at))
        .col_expr(cycle_runs::Column::Error, Expr::value(error))
        .col_expr(
            cycle_runs::Column::ProcessesSeen,
            Expr::value(processes_seen),
        )
        .col_expr(
            cycle_runs::Column::EventsCreated,
            Expr::value(events_created),
        )
        .filter(cycle_runs::Column::Id.eq(run_id))
        .filter(cycle_runs::Column::AccountId.eq(account_id))
        .exec(transaction)
        .await?;
    let prune = Statement::from_sql_and_values(
        DbBackend::Postgres,
        PRUNE_CYCLE_RUNS,
        [account_id.into(), KEPT_RUNS_PER_ACCOUNT.into()],
    );
    transaction.execute_raw(prune).await?;
    Ok(())
}
