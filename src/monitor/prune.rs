use std::error::Error;
use std::sync::atomic::Ordering;

use chrono::{DateTime, TimeDelta, Utc};
use tracing::{info, warn};

use super::Monitor;
use crate::store::Pruned;

impl Monitor {
    pub fn acknowledge(&self, after: i64) {
        self.acked.fetch_max(after, Ordering::SeqCst);
    }

    pub fn acked_seq(&self) -> i64 {
        self.acked.load(Ordering::SeqCst)
    }

    pub(super) async fn prune(&self, account_id: i64, started: DateTime<Utc>) {
        let retention_days = self.config.retention_days;
        if retention_days == 0 {
            return;
        }
        let Some(cutoff) = TimeDelta::try_days(retention_days)
            .and_then(|retention| started.checked_sub_signed(retention))
        else {
            return;
        };
        let pruned = match self
            .store
            .prune_before(account_id, cutoff, self.acked_seq())
            .await
        {
            Ok(pruned) => pruned,
            Err(error) => {
                warn!(error = &error as &dyn Error, "could not remove old records");
                return;
            }
        };
        if pruned == Pruned::default() {
            return;
        }
        info!(
            events = pruned.events,
            processes = pruned.processes,
            retention_days,
            "removed old records"
        );
    }
}
