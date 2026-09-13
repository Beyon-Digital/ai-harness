//! Durable cursors: `v1:{stream_key}:{sequence}`, opaque at boundaries.
//!
//! [`EventCursor`] itself lives in `domain`; this module re-exports it and
//! anchors the canonical constructor on validated [`StreamKey`] instances.

pub use domain::ids::EventCursor;

use crate::stream::StreamKey;

/// Constructs cursors for canonical streams.
///
/// `EventCursor` is a domain type, so the pairing constructor is provided as a
/// trait; with this trait in scope it is callable as
/// `EventCursor::for_event(&key, sequence)`.
pub trait EventCursorExt {
    /// Builds the canonical `v1:{stream_key}:{sequence}` cursor for `key`.
    fn for_event(key: &StreamKey, sequence: u64) -> Self;
}

impl EventCursorExt for EventCursor {
    fn for_event(key: &StreamKey, sequence: u64) -> Self {
        EventCursor::new(key.event_stream_key().clone(), sequence)
    }
}
