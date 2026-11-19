use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseTransaction, DbBackend, EntityTrait, FromQueryResult,
    QueryFilter, Statement,
};

use super::entity::accounts;
use super::records::{Account, AccountInfo, AccountKey};
use super::{Store, StoreError};

// DO UPDATE instead of DO NOTHING so RETURNING also gives back an account that already exists.
const ENSURE_ACCOUNT: &str = "INSERT INTO accounts (sei_base_url, username, org_id, created_at)
VALUES ($1, $2, $3, $4)
ON CONFLICT (sei_base_url, username, org_id) DO UPDATE SET username = excluded.username
RETURNING *";

// The scheduler tests find the pause write in pg_stat_activity by this text.
const SET_PAUSED_REASON: &str = "UPDATE accounts SET paused_reason = $1 WHERE id = $2";

// Every cycle of an account takes this row lock first, so events get their ids in commit order.
const LOCK_ACCOUNT: &str = "SELECT id FROM accounts WHERE id = $1 FOR NO KEY UPDATE";

impl Store {
    pub async fn ensure_account(
        &self,
        key: &AccountKey,
        now: DateTime<Utc>,
    ) -> Result<Account, StoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            ENSURE_ACCOUNT,
            [
                key.sei_base_url.as_str().into(),
                key.username.as_str().into(),
                key.org_id.into(),
                now.into(),
            ],
        );
        let account = accounts::Model::find_by_statement(statement)
            .one(self.connection())
            .await?
            .ok_or(StoreError::AccountNotFound)?;
        Ok(account.into())
    }

    pub async fn set_paused_reason(&self, account_id: i64, reason: &str) -> Result<(), StoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            SET_PAUSED_REASON,
            [reason.into(), account_id.into()],
        );
        let updated = self.connection().execute_raw(statement).await?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::AccountNotFound);
        }
        Ok(())
    }

    pub async fn update_account_info(
        &self,
        account_id: i64,
        info: &AccountInfo,
    ) -> Result<(), StoreError> {
        accounts::Entity::update_many()
            .col_expr(
                accounts::Column::OrgAcronym,
                Expr::value(info.org_acronym.as_str()),
            )
            .col_expr(
                accounts::Column::SeiVersion,
                Expr::value(info.sei_version.as_str()),
            )
            .col_expr(
                accounts::Column::WsseiVersion,
                Expr::value(info.wssei_version.as_str()),
            )
            .filter(accounts::Column::Id.eq(account_id))
            .exec(self.connection())
            .await?;
        Ok(())
    }
}

pub(super) async fn lock_account(
    transaction: &DatabaseTransaction,
    account_id: i64,
) -> Result<(), StoreError> {
    let statement =
        Statement::from_sql_and_values(DbBackend::Postgres, LOCK_ACCOUNT, [account_id.into()]);
    transaction.execute_raw(statement).await?;
    Ok(())
}
