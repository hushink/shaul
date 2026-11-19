use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use shaul_core::{Error, Result};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

/// Shared active-request budget for the HTTP and Hub file-transfer paths.
pub(crate) struct ConnectionBudget {
    global: Arc<Semaphore>,
    per_host: usize,
    hosts: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl ConnectionBudget {
    pub(crate) fn new(global: usize, per_host: usize) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global.max(1))),
            per_host: per_host.max(1),
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Acquires one host permit and one global permit. The host is acquired
    /// first so a busy host cannot consume the global budget while waiting for
    /// its own per-host slot.
    pub(crate) async fn acquire(
        &self,
        host: &str,
        cancel: &CancellationToken,
    ) -> Result<ConnectionPermit> {
        let host_semaphore = {
            let mut hosts = self
                .hosts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            hosts
                .entry(host.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_host)))
                .clone()
        };
        let host_permit = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            permit = host_semaphore.acquire_owned() => permit
                .map_err(|_| Error::Other("host connection budget closed".to_string()))?,
        };
        let global_permit = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            permit = self.global.clone().acquire_owned() => permit
                .map_err(|_| Error::Other("global connection budget closed".to_string()))?,
        };
        Ok(ConnectionPermit {
            _host: host_permit,
            _global: global_permit,
        })
    }
}

/// Holds active-request permits until the response body is done.
pub(crate) struct ConnectionPermit {
    _host: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn enforces_the_global_budget() {
        let budget = ConnectionBudget::new(2, 10);
        let cancel = CancellationToken::new();
        let _first = budget.acquire("a", &cancel).await.unwrap();
        let _second = budget.acquire("a", &cancel).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), budget.acquire("a", &cancel))
                .await
                .is_err(),
            "a third request exceeded the global budget"
        );
    }

    #[tokio::test]
    async fn enforces_the_per_host_budget() {
        let budget = ConnectionBudget::new(10, 1);
        let cancel = CancellationToken::new();
        let _first = budget.acquire("a", &cancel).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), budget.acquire("a", &cancel))
                .await
                .is_err(),
            "a second request to the same host exceeded the per-host budget"
        );
        let _other_host = budget.acquire("b", &cancel).await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_interrupts_acquisition() {
        let budget = ConnectionBudget::new(1, 1);
        let cancel = CancellationToken::new();
        let _first = budget.acquire("a", &cancel).await.unwrap();
        cancel.cancel();
        assert!(matches!(
            budget.acquire("a", &cancel).await,
            Err(Error::Cancelled)
        ));
    }
}
