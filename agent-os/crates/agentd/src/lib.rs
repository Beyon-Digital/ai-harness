//! `agentd` — the Agent OS microkernel daemon library surface.
//!
//! [`bootstrap::boot`] composes the daemon (singleton lock, durable store,
//! fencing, recovery, command coordinator, event pipeline, background
//! workers, and the Unix-socket Control/Event APIs) so both the binary and
//! integration tests run the real composition.
#![forbid(unsafe_code)]

pub mod adapters;
pub mod api;
pub mod bootstrap;
pub mod lock;
pub mod recovery;
pub mod workers;
