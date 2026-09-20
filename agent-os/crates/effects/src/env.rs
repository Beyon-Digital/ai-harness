//! Shared environment for effect operations: the ID/clock sources plus the
//! correlation propagated onto every staged event.

use domain::ids::EventId;
use domain::provider::IdProvider;
use domain::time::Clock;

/// The ambient dependencies every effect operation needs.
#[derive(Clone)]
pub struct EffectEnv<'a> {
    /// Identifier source for staged outbox events.
    pub ids: &'a dyn IdProvider,
    /// Wall clock for lease expiry and timestamps.
    pub clock: &'a dyn Clock,
    /// Correlation identifier propagated to staged events.
    pub correlation_id: Option<String>,
    /// Directly causing event, when known.
    pub causation_id: Option<EventId>,
}
