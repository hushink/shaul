use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shaul_core::{Error, Result};
use tokio_util::sync::CancellationToken;

/// Aggregates reservations from all transfers sharing this limiter.
pub(crate) struct RateLimiter {
    bytes_per_second: u64,
    next: Mutex<Instant>,
}

impl RateLimiter {
    pub(crate) fn new(bytes_per_second: u64) -> Self {
        Self {
            bytes_per_second: bytes_per_second.max(1),
            next: Mutex::new(Instant::now()),
        }
    }

    /// Reserves `bytes` and returns how long the caller should wait before
    /// consuming them. Reservations are serialized, so concurrent callers share
    /// one aggregate rate.
    pub(crate) fn reserve(&self, bytes: u64) -> Duration {
        if bytes == 0 {
            return Duration::ZERO;
        }
        let now = Instant::now();
        let mut next = self
            .next
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let start = (*next).max(now);
        let seconds = bytes as f64 / self.bytes_per_second as f64;
        let nanos = (seconds * 1_000_000_000.0).min(u64::MAX as f64) as u64;
        let duration = Duration::from_nanos(nanos);
        *next = start.checked_add(duration).unwrap_or(start);
        start.saturating_duration_since(now)
    }
}

/// Global, per-host and per-task limiters applied to one transfer.
#[derive(Clone, Default)]
pub(crate) struct RateLimiters {
    global: Option<Arc<RateLimiter>>,
    host: Option<Arc<RateLimiter>>,
    task: Option<Arc<RateLimiter>>,
}

impl RateLimiters {
    pub(crate) fn new(
        global: Option<Arc<RateLimiter>>,
        host: Option<Arc<RateLimiter>>,
        task: Option<Arc<RateLimiter>>,
    ) -> Self {
        Self { global, host, task }
    }

    /// Reserves `bytes` on every configured limiter and returns the longest
    /// wait. The effective rate is therefore the lower of the limits.
    pub(crate) fn reserve(&self, bytes: u64) -> Duration {
        let mut wait = Duration::ZERO;
        for limiter in [&self.global, &self.host, &self.task].into_iter().flatten() {
            wait = wait.max(limiter.reserve(bytes));
        }
        wait
    }

    pub(crate) async fn throttle(&self, bytes: u64, cancel: &CancellationToken) -> Result<()> {
        let wait = self.reserve(bytes);
        if wait.is_zero() {
            return Ok(());
        }
        tokio::select! {
            _ = cancel.cancelled() => Err(Error::Cancelled),
            _ = tokio::time::sleep(wait) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_share_one_virtual_schedule() {
        let limiter = RateLimiter::new(1_000_000);
        assert_eq!(limiter.reserve(100_000), Duration::ZERO);

        let wait = limiter.reserve(100_000);
        assert!(
            wait >= Duration::from_millis(95) && wait <= Duration::from_millis(105),
            "unexpected wait: {wait:?}"
        );
    }

    #[test]
    fn the_slowest_limiter_wins() {
        let global = Arc::new(RateLimiter::new(1_000));
        let task = Arc::new(RateLimiter::new(2_000));
        let limiters = RateLimiters::new(Some(global), None, Some(task));
        assert_eq!(limiters.reserve(100), Duration::ZERO);

        let wait = limiters.reserve(100);
        assert!(
            wait >= Duration::from_millis(95) && wait <= Duration::from_millis(105),
            "unexpected wait: {wait:?}"
        );
    }

    #[test]
    fn host_limiter_is_aggregated_with_the_others() {
        let host = Arc::new(RateLimiter::new(1_000));
        let limiters = RateLimiters::new(None, Some(host), None);
        assert_eq!(limiters.reserve(100), Duration::ZERO);
        let wait = limiters.reserve(100);
        assert!(
            wait >= Duration::from_millis(95) && wait <= Duration::from_millis(105),
            "unexpected wait: {wait:?}"
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_reservation() {
        let limiters = RateLimiters::new(Some(Arc::new(RateLimiter::new(1))), None, None);
        let cancel = CancellationToken::new();
        assert!(limiters.throttle(1, &cancel).await.is_ok());

        cancel.cancel();
        assert!(matches!(
            limiters.throttle(1, &cancel).await,
            Err(Error::Cancelled)
        ));
    }
}
