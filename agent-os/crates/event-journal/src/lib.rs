//! Event journal port.
//!
//! The port types live in `events::journal`, the lower-vocabulary crate that
//! the SQLite journal already consumes, and are re-exported here so existing
//! `event_journal::{...}` imports keep compiling.

#![forbid(unsafe_code)]

pub use events::journal::{AppendResult, EventJournalPort, ReadResult};
