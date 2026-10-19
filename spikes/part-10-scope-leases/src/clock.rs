//! Time source for lease expiry.
//!
//! Expiry is measured on a monotonic clock, never wall-clock time, so changing the system date
//! cannot extend or shorten a lease. The clock is a trait so tests can move time by hand.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub trait Clock: Send + Sync {
    /// Time since an arbitrary fixed origin. Must never go backwards.
    fn now(&self) -> Duration;
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now(&self) -> Duration {
        (**self).now()
    }
}

/// `std::time::Instant` relative to the moment the clock was created.
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock that only moves when told to. For deterministic expiry tests.
#[derive(Debug, Default)]
pub struct ManualClock {
    nanos: AtomicU64,
}

impl ManualClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves time forward, saturating at the largest representable time.
    pub fn advance(&self, by: Duration) {
        let by = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        // A plain compare-exchange loop: `fetch_update` is deprecated on newer toolchains and its
        // replacement `try_update` is missing on older ones.
        let mut current = self.nanos.load(Ordering::SeqCst);
        while let Err(seen) = self.nanos.compare_exchange_weak(
            current,
            current.saturating_add(by),
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            current = seen;
        }
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
    }
}
