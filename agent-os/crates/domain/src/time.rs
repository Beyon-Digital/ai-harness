//! `Clock` trait and the system implementation.

/// Source of wall-clock time as UTC Unix milliseconds.
pub trait Clock: Send + Sync + 'static {
    /// Returns the current UTC Unix time in milliseconds.
    fn now_unix_ms(&self) -> i64;
}

/// Production [`Clock`] backed by `std::time::SystemTime`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
            Err(_) => 0,
        }
    }
}
