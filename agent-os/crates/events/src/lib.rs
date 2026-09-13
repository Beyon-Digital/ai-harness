//! Event envelope, sequence and cursor helpers, and the live event bus.
#![forbid(unsafe_code)]

pub mod cursor;
pub mod dispatcher;
pub mod envelope;
pub mod journal;
pub mod live_bus;
pub mod outbox;
pub mod stream;

pub use cursor::{EventCursor, EventCursorExt};
pub use envelope::{
    CatalogClassificationPolicy, ClassificationPolicy, EventBuilder, EventEnvelope,
};
pub use journal::{AppendResult, EventJournalPort, ReadResult};
pub use live_bus::LiveItem;
pub use stream::{StreamKey, StreamKind};
