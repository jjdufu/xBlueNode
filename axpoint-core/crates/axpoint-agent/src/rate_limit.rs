//! Thread-safe token-bucket rate limiter keyed by arbitrary hashable subjects.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use axpoint_domain::AppError;

#[derive(Debug, Clone, Copy)]
struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

/// In-memory token-bucket rate limiter enforcing requests-per-minute (`rpm`) budgets.
#[derive(Debug)]
pub struct KeyedRateLimiter<K> {
    buckets: Mutex<HashMap<K, BucketState>>,
    max_entries: usize,
}

impl<K> Default for KeyedRateLimiter<K>
where
    K: Eq + Hash,
{
    fn default() -> Self {
        Self::new(16_384)
    }
}

impl<K> KeyedRateLimiter<K>
where
    K: Eq + Hash,
{
    /// Creates a new [`KeyedRateLimiter`] retaining at most `max_entries` active keys before pruning stale buckets.
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            max_entries: max_entries.max(64),
        }
    }

    /// Consumes `1.0` token from `key` under `rate_limit_rpm` (`1..=600_000` requests/min).
    ///
    /// # Errors
    /// Returns [`AppError::RateLimited`] (`429 RATE_LIMITED`) when no token is available.
    pub fn check_and_consume(&self, key: K, rate_limit_rpm: i32) -> Result<(), AppError> {
        self.check_and_consume_at(key, rate_limit_rpm, Instant::now())
    }

    /// Deterministic clock variant of [`Self::check_and_consume`] for unit testing.
    ///
    /// # Errors
    /// Returns [`AppError::RateLimited`] when the bucket is depleted at `now`.
    pub fn check_and_consume_at(
        &self,
        key: K,
        rate_limit_rpm: i32,
        now: Instant,
    ) -> Result<(), AppError> {
        if rate_limit_rpm <= 0 {
            return Err(AppError::RateLimited);
        }
        let capacity = f64::from(rate_limit_rpm);
        let refill_per_sec = capacity / 60.0_f64;

        let mut guard = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.len() >= self.max_entries && !guard.contains_key(&key) {
            guard.retain(|_, state| now.duration_since(state.last_refill).as_secs() < 120);
        }

        let entry = guard.entry(key).or_insert(BucketState {
            tokens: capacity,
            last_refill: now,
        });

        let elapsed = now
            .saturating_duration_since(entry.last_refill)
            .as_secs_f64();
        if elapsed > 0.0_f64 {
            entry.tokens = (entry.tokens + elapsed * refill_per_sec).min(capacity);
            entry.last_refill = now;
        }

        if entry.tokens >= 1.0_f64 {
            entry.tokens -= 1.0_f64;
            Ok(())
        } else {
            Err(AppError::RateLimited)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axpoint_domain::AppError;

    use super::KeyedRateLimiter;

    #[test]
    fn token_bucket_enforces_rpm_and_refills_over_time() -> Result<(), AppError> {
        let limiter: KeyedRateLimiter<&str> = KeyedRateLimiter::new(128);
        let start = Instant::now();

        limiter.check_and_consume_at("ep:tok1", 2, start)?;
        limiter.check_and_consume_at("ep:tok1", 2, start)?;
        let third = limiter.check_and_consume_at("ep:tok1", 2, start);
        assert!(matches!(third, Err(AppError::RateLimited)));

        // Independent key is unaffected.
        limiter.check_and_consume_at("ep:tok2", 2, start)?;

        // After 30 seconds at 2 RPM, 1.0 token refills.
        let after_30s = start + Duration::from_secs(30);
        limiter.check_and_consume_at("ep:tok1", 2, after_30s)?;
        assert!(matches!(
            limiter.check_and_consume_at("ep:tok1", 2, after_30s),
            Err(AppError::RateLimited)
        ));
        Ok(())
    }
}
