//! Deterministic `Clock` implementation for tests.

use std::sync::{Arc, Mutex, MutexGuard};

use domain::time::Clock;

/// Deterministic [`Clock`] whose time only moves through explicit calls.
#[derive(Clone, Debug)]
pub struct TestClock {
    unix_ms: Arc<Mutex<i64>>,
}

impl TestClock {
    /// Creates a clock pinned at `start_unix_ms`.
    pub fn new(start_unix_ms: i64) -> Self {
        Self {
            unix_ms: Arc::new(Mutex::new(start_unix_ms)),
        }
    }

    /// Advances the clock by `delta_ms`, which may be negative.
    pub fn advance(&self, delta_ms: i64) {
        let mut now = self.lock();
        *now = now.saturating_add(delta_ms);
    }

    /// Sets the clock to `unix_ms`.
    pub fn set(&self, unix_ms: i64) {
        *self.lock() = unix_ms;
    }

    fn lock(&self) -> MutexGuard<'_, i64> {
        match self.unix_ms.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Clock for TestClock {
    fn now_unix_ms(&self) -> i64 {
        *self.lock()
    }
}

#[cfg(test)]
mod tests {
    use domain::time::Clock;

    use super::TestClock;

    #[test]
    fn test_clock_advances() {
        let clock = TestClock::new(1_000);
        assert_eq!(clock.now_unix_ms(), 1_000);
        clock.advance(250);
        assert_eq!(clock.now_unix_ms(), 1_250);
        clock.set(42);
        assert_eq!(clock.now_unix_ms(), 42);
        clock.advance(-50);
        assert_eq!(clock.now_unix_ms(), -8);
    }

    #[test]
    fn cloned_clocks_share_their_time_source() {
        let clock = TestClock::new(5);
        let clone = clock.clone();
        clone.advance(10);
        assert_eq!(clock.now_unix_ms(), 15);
    }
}
