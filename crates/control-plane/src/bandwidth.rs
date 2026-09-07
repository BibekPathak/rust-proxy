//! Bandwidth allocation and rate limiting.
//!
//! Two cooperating pieces:
//!
//! * [`BandwidthAllocator`] reserves *rate capacity* (bytes/sec) up to a
//!   configured ceiling before a connection is admitted, preventing
//!   over-commitment. A [`BandwidthGuard`] holds a reservation and releases
//!   it on drop.
//! * [`RateLimiter`] is a leaky-bucket limiter that actually throttles bytes
//!   transferred through a live connection to the reserved rate.
//!
//! The allocator shares a single `Arc<AtomicU64>` counter between the
//! allocator and every outstanding guard, so reservations are updated
//! atomically without a global mutex.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Reserves a share of a node's bandwidth budget.
///
/// The sum of all outstanding reservations never exceeds the configured
/// ceiling, guarding a node against over-commitment.
#[derive(Debug, Clone)]
pub struct BandwidthAllocator {
    limit: u64,
    reserved: Arc<AtomicU64>,
}

impl BandwidthAllocator {
    pub fn new(limit_bytes_per_sec: u64) -> Self {
        Self {
            limit: limit_bytes_per_sec,
            reserved: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Attempt to reserve `rate` bytes/sec.
    ///
    /// Returns a guard on success, or `None` when it would exceed the ceiling.
    pub fn try_reserve(&self, rate: u64) -> Option<BandwidthGuard> {
        loop {
            let current = self.reserved.load(Ordering::Relaxed);
            let next = current.checked_add(rate)?;
            if next > self.limit {
                return None;
            }
            if self
                .reserved
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Some(BandwidthGuard {
                    reserved: Arc::clone(&self.reserved),
                    rate,
                });
            }
        }
    }

    /// Available (unreserved) bandwidth in bytes/sec.
    pub fn available(&self) -> u64 {
        self.limit
            .saturating_sub(self.reserved.load(Ordering::Relaxed))
    }

    /// Reserved bandwidth in bytes/sec.
    pub fn reserved(&self) -> u64 {
        self.reserved.load(Ordering::Relaxed)
    }

    /// Configured ceiling in bytes/sec.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Utilization in `[0, 1]`.
    pub fn utilization(&self) -> f64 {
        if self.limit == 0 {
            return 1.0;
        }
        (self.reserved() as f64 / self.limit as f64).clamp(0.0, 1.0)
    }
}

/// A borrowed reservation returned by [`BandwidthAllocator::try_reserve`].
///
/// Releasing happens automatically when the guard is dropped; it decrements
/// the shared counter atomically.
#[derive(Debug)]
pub struct BandwidthGuard {
    reserved: Arc<AtomicU64>,
    rate: u64,
}

impl BandwidthGuard {
    /// The rate reserved by this guard.
    pub fn rate(&self) -> u64 {
        self.rate
    }
}

impl Drop for BandwidthGuard {
    fn drop(&mut self) {
        // A reservation is always <= the counter, so a plain subtract (not
        // saturating) is correct; use saturating_sub defensively anyway.
        self.reserved.fetch_sub(self.rate, Ordering::AcqRel);
    }
}

/// A leaky-bucket rate limiter.
///
/// A fixed supply of tokens refills linearly over time at `rate`
/// tokens/second up to `capacity`. Consumers call [`RateLimiter::acquire`]
/// before emitting bytes; it sleeps until enough tokens have accumulated.
/// Bursts up to `capacity` are allowed, but the long-term average is `rate`.
#[derive(Debug)]
pub struct RateLimiter {
    rate: f64,
    capacity: f64,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(rate_bytes_per_sec: u64, capacity_bytes: u64) -> Self {
        Self {
            rate: rate_bytes_per_sec as f64,
            capacity: capacity_bytes as f64,
            tokens: capacity_bytes as f64,
            last: Instant::now(),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        let added = elapsed * self.rate;
        self.tokens = (self.tokens + added).min(self.capacity);
    }

    /// Number of tokens currently available.
    pub fn available(&self) -> f64 {
        self.tokens
    }

    /// Whether `n` tokens can be taken immediately without waiting.
    pub fn try_take(&mut self, n: u64) -> bool {
        self.refill();
        let n = n as f64;
        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }

    /// Block (async) until `n` tokens are available, then consume them.
    pub async fn acquire(&mut self, n: u64) -> Duration {
        loop {
            if self.try_take(n) {
                return Duration::ZERO;
            }
            let missing = n as f64 - self.tokens;
            let wait = missing / self.rate;
            tokio::time::sleep(Duration::from_secs_f64(wait.max(0.001))).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_respects_ceiling() {
        let a = BandwidthAllocator::new(1000);
        let g1 = a.try_reserve(400).unwrap();
        let g2 = a.try_reserve(400).unwrap();
        assert_eq!(a.reserved(), 800);
        // Over the ceiling -> rejected.
        assert!(a.try_reserve(300).is_none());
        let g3 = a.try_reserve(200).unwrap();
        assert_eq!(a.reserved(), 1000);
        drop(g1);
        drop(g2);
        drop(g3);
        assert_eq!(a.reserved(), 0);
    }

    #[test]
    fn frozen_limit_reserves_nothing() {
        let a = BandwidthAllocator::new(1000);
        let guards = vec![a.try_reserve(600).unwrap(), a.try_reserve(400).unwrap()];
        assert_eq!(a.reserved(), 1000);
        // Nothing left.
        assert!(a.try_reserve(1).is_none());
        drop(guards);
    }

    #[test]
    fn rate_limiter_allows_burst_then_throttles() {
        let mut limiter = RateLimiter::new(100, 100);
        // Capacity 100 -> a full burst is allowed immediately.
        assert!(limiter.try_take(100));
        // Now drained; cannot take more instantly.
        assert!(!limiter.try_take(1));
    }

    #[tokio::test]
    async fn rate_limiter_refills_over_time() {
        let mut limiter = RateLimiter::new(1000, 1000);
        assert!(limiter.try_take(1000));
        assert!(!limiter.try_take(100));
        // After ~150ms at 1000/s, ~150 tokens accumulate.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(limiter.try_take(100));
    }
}
