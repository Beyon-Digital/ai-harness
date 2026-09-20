//! Catalogued event staging for resource streams.
//!
//! Mirrors the scheduler/effects staging helpers: sensitivity and retention
//! floors come from the embedded event catalog so a staged event can never
//! under-classify its payload.

use domain::ids::{EventId, EventStreamKey, RunId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::outbox::{DraftEvent, stage};
use events::stream::StreamKey;
use events::{CatalogClassificationPolicy, ClassificationPolicy};
use kernel_store::KernelTxn;

/// Stages a catalogued resource event on the owning run's stream.
pub(crate) async fn stage_resource_event(
    txn: &mut dyn KernelTxn,
    event_id: EventId,
    event_type: &str,
    run_id: RunId,
    payload: Vec<u8>,
    correlation_id: Option<String>,
    causation_id: Option<EventId>,
) -> errors::Result<u64> {
    let policy = CatalogClassificationPolicy::embedded()?;
    let sensitivity = policy
        .minimum(event_type)
        .ok_or_else(|| uncatalogued(event_type))?;
    let retention = policy
        .default_retention(event_type)
        .ok_or_else(|| uncatalogued(event_type))?;
    let stream_key = EventStreamKey::new(StreamKey::run(run_id).as_str().to_owned()).map_err(
        |_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "catalogued resource stream key is not canonical",
            )
        },
    )?;
    stage(
        txn,
        DraftEvent {
            event_id,
            stream_key,
            event_type: event_type.to_owned(),
            payload,
            sensitivity,
            retention,
            correlation_id,
            causation_id,
        },
    )
    .await
}

fn uncatalogued(event_type: &str) -> KernelError {
    KernelError::new(
        ErrorCode::Internal,
        RetryClass::Never,
        format!("event type {event_type} is not in the embedded catalog"),
    )
}
