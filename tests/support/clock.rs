use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sei_monitor::monitor::Clock;
use tokio::sync::{mpsc, oneshot};

const SLEEP_WAIT: Duration = Duration::from_secs(10);

pub struct PendingSleep {
    pub duration: Duration,
    pub fire: oneshot::Sender<()>,
}

pub struct FakeClock {
    now: Mutex<DateTime<Utc>>,
    sleeps: mpsc::UnboundedSender<PendingSleep>,
    pending: tokio::sync::Mutex<mpsc::UnboundedReceiver<PendingSleep>>,
}

impl FakeClock {
    pub fn new(start: DateTime<Utc>) -> Arc<FakeClock> {
        let (sleeps, pending) = mpsc::unbounded_channel();
        Arc::new(FakeClock {
            now: Mutex::new(start),
            sleeps,
            pending: tokio::sync::Mutex::new(pending),
        })
    }

    pub fn advance(&self, by: Duration) {
        let by = chrono::Duration::from_std(by).expect("an advance the clock can hold");
        *self.now.lock().unwrap_or_else(PoisonError::into_inner) += by;
    }

    pub async fn next_sleep(&self) -> PendingSleep {
        let mut pending = self.pending.lock().await;
        tokio::time::timeout(SLEEP_WAIT, pending.recv())
            .await
            .expect("the monitor never started waiting")
            .expect("the clock keeps its own sender")
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let (fire, fired) = oneshot::channel();
        self.sleeps
            .send(PendingSleep { duration, fire })
            .expect("the clock keeps its own receiver");
        Box::pin(async move {
            if fired.await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}
