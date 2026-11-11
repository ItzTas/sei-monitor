use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};

use super::records::{Account, AccountInfo, AccountKey};
use super::{Store, StoreError};

const ENSURE_ACCOUNT: &str = "INSERT INTO accounts (sei_base_url, username, org_id, created_at)
VALUES ($1, $2, $3, $4)
ON CONFLICT (sei_base_url, username, org_id) DO UPDATE SET username = excluded.username
RETURNING *";

const SET_PAUSED_REASON: &str = "UPDATE accounts SET paused_reason = $1 WHERE id = $2";

const LOCK_ACCOUNT: &str = "SELECT id FROM accounts WHERE id = $1 FOR NO KEY UPDATE";

const UPDATE_ACCOUNT_INFO: &str = "UPDATE accounts
SET org_acronym = $1, sei_version = $2, wssei_version = $3
WHERE id = $4";

impl Store {
    pub async fn ensure_account(
        &self,
        key: &AccountKey,
        now: DateTime<Utc>,
    ) -> Result<Account, StoreError> {
        let row = self
            .client()
            .await?
            .query_one(
                ENSURE_ACCOUNT,
                &[&key.sei_base_url, &key.username, &key.org_id, &now],
            )
            .await?;
        Ok(account(&row)?)
    }

    pub async fn set_paused_reason(&self, account_id: i64, reason: &str) -> Result<(), StoreError> {
        let updated = self
            .client()
            .await?
            .execute(SET_PAUSED_REASON, &[&reason, &account_id])
            .await?;
        if updated == 0 {
            return Err(StoreError::AccountNotFound);
        }
        Ok(())
    }

    pub async fn update_account_info(
        &self,
        account_id: i64,
        info: &AccountInfo,
    ) -> Result<(), StoreError> {
        self.client()
            .await?
            .execute(
                UPDATE_ACCOUNT_INFO,
                &[
                    &info.org_acronym,
                    &info.sei_version,
                    &info.wssei_version,
                    &account_id,
                ],
            )
            .await?;
        Ok(())
    }
}

pub(super) async fn lock_account(
    transaction: &Transaction<'_>,
    account_id: i64,
) -> Result<(), StoreError> {
    transaction.execute(LOCK_ACCOUNT, &[&account_id]).await?;
    Ok(())
}

fn account(row: &Row) -> Result<Account, tokio_postgres::Error> {
    Ok(Account {
        id: row.try_get("id")?,
        key: AccountKey {
            sei_base_url: row.try_get("sei_base_url")?,
            username: row.try_get("username")?,
            org_id: row.try_get("org_id")?,
        },
        created_at: row.try_get("created_at")?,
        baseline_at: row.try_get("baseline_at")?,
        paused_reason: row.try_get("paused_reason")?,
        info: AccountInfo {
            org_acronym: row.try_get("org_acronym")?,
            sei_version: row.try_get("sei_version")?,
            wssei_version: row.try_get("wssei_version")?,
        },
    })
}
