//! Outbox dispatch worker.
//!
//! A thin interval loop around the journal-first dispatcher: one
//! [`EventDispatcher::dispatch_once`] per tick, with capped exponential
//! backoff while the journal reports `Unavailable` and prompt cancellation
//! through a `tokio::sync::watch` flag.
//!
//! The composition root wires this worker in a later task, so its items are
//! not yet reachable from `main`.

use std::sync::Arc;
use std::time::Duration;

use errors::codes::ErrorCode;
use events::dispatcher::EventDispatcher;
use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval, sleep};

/// Rows scanned per dispatch iteration.
const BATCH_LIMIT: u32 = 64;
/// First backoff after an unavailable iteration.
const INITIAL_BACKOFF: Duration = Duration::from_millis(50);
/// Upper bound on the dispatch backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(2);

/// Supplies the daemon fencing epoch the worker dispatches under.
pub trait EpochSource: Send + Sync {
    /// Returns the epoch currently held by the daemon.
    fn epoch(&self) -> u64;
}

/// Thin interval loop around the journal-first dispatcher.
pub struct OutboxWorker {
    dispatcher: Arc<EventDispatcher>,
    epoch: Arc<dyn EpochSource>,
    poll: Duration,
}

impl OutboxWorker {
    /// Creates a worker that polls the outbox every `poll`.
    #[allow(dead_code)]
    pub fn new(
        dispatcher: Arc<EventDispatcher>,
        epoch: Arc<dyn EpochSource>,
        poll: Duration,
    ) -> Self {
        Self {
            dispatcher,
            epoch,
            poll,
        }
    }

    /// Dispatches until `shutdown` requests a stop.
    ///
    /// Availability failures back off exponentially up to [`MAX_BACKOFF`];
    /// success and every other failure resume the normal `poll` cadence.
    #[allow(dead_code)]
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut ticker = interval(self.poll);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut backoff = INITIAL_BACKOFF;
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                    continue;
                }
            }
            match self
                .dispatcher
                .dispatch_once(BATCH_LIMIT, self.epoch.epoch())
                .await
            {
                Ok(_) => backoff = INITIAL_BACKOFF,
                Err(error) if error.code() == ErrorCode::Unavailable => {
                    tokio::select! {
                        _ = sleep(backoff) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                return;
                            }
                        }
                    }
                    backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
                }
                Err(_) => backoff = INITIAL_BACKOFF,
            }
        }
    }
}
