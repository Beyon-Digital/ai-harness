//! Event journal port.
//!
//! The journal is the durable, per-stream ordered history of accepted events.
//! It is a downstream projection with its own database file; callers append
//! batches under the exact expected stream head and read history forward from
//! a sequence. Implementations own all persistence details; the port fixes
//! only the append and read contracts.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use events::{EventEnvelope, StreamKey};

/// Outcome of a successful append.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendResult {
    /// Position of the last event in the appended batch.
    ///
    /// For an idempotent duplicate append this is the last position of the
    /// batch, which is already at or before the current stream head.
    pub final_sequence: u64,
}

/// A forward page of journal history for one stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadResult {
    /// Events with `sequence > from_sequence`, ascending, up to the limit.
    pub events: Vec<EventEnvelope>,
    /// Whether the journal dropped history before `from_sequence`.
    ///
    /// The inception journal never drops history, so implementations that do
    /// not retain gaps report `false`.
    pub retention_gap: bool,
}

/// Durable per-stream event history.
#[async_trait]
pub trait EventJournalPort: Send + Sync {
    /// Appends `batch` when the stream head is exactly `expected_sequence`.
    ///
    /// `batch` sequences must be contiguous from `expected_sequence + 1`, and
    /// every envelope must belong to `stream_key`.
    ///
    /// * `expected_sequence` not matching the head and no already-recorded
    ///   batch to replay is a failed precondition.
    /// * A batch already recorded exactly (same event id, position, and
    ///   bytes) is an idempotent success with no write.
    /// * A recorded position holding a different event id is a conflict.
    async fn append(
        &self,
        stream_key: &StreamKey,
        expected_sequence: u64,
        batch: &[EventEnvelope],
    ) -> errors::Result<AppendResult>;

    /// Reads events with `sequence > from_sequence` in ascending order.
    ///
    /// Fewer than `limit` events are returned when the stream ends or the
    /// stream has no events past `from_sequence`; reading past the head is an
    /// empty success, not an error.
    async fn read_stream(
        &self,
        stream_key: &StreamKey,
        from_sequence: u64,
        limit: u32,
    ) -> errors::Result<ReadResult>;
}
