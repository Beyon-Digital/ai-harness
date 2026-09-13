//! Transactional outbox staging: allocate a stream position and insert the
//! immutable outbox row inside the caller's write transaction.
//!
//! [`stage`] performs no I/O of its own: it uses the transaction's stream
//! repository, so the event becomes visible exactly when the causing
//! transaction commits and disappears with its rollback (R6.1, R6.5, D9).
//! Allocation is the single conditional statement of D10, which makes the
//! per-stream sequence structurally contiguous.

use domain::ids::{EventId, EventStreamKey};
use domain::security::{RetentionClass, SensitivityClass};
use domain::time::{Clock, SystemClock};
use kernel_store::KernelTxn;
use kernel_store::models::NewOutboxEvent;

/// Initial persisted version for events staged through [`stage`].
const EVENT_VERSION: u32 = 1;

/// Event contents staged into the transactional outbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftEvent {
    /// Stable identity of the event; unique across the outbox.
    pub event_id: EventId,
    /// Stream that receives the event.
    pub stream_key: EventStreamKey,
    /// Event type name from the contracts.
    pub event_type: String,
    /// Serialized event payload; never logged.
    pub payload: Vec<u8>,
    /// Sensitivity classification of the payload.
    pub sensitivity: SensitivityClass,
    /// Retention class of the payload.
    pub retention: RetentionClass,
    /// Optional correlation identifier linking related events.
    pub correlation_id: Option<String>,
    /// Optional event that directly caused this one.
    pub causation_id: Option<EventId>,
}

impl DraftEvent {
    /// Projects the draft onto the immutable outbox insert model.
    fn into_new(self, sequence: u64, occurred_at_ms: i64) -> NewOutboxEvent {
        NewOutboxEvent {
            event_id: self.event_id,
            event_type: self.event_type,
            event_version: EVENT_VERSION,
            stream_key: self.stream_key,
            sequence,
            occurred_at_ms,
            run_id: None,
            task_id: None,
            session_id: None,
            effect_id: None,
            causation_id: self.causation_id,
            correlation_id: self.correlation_id,
            sensitivity: self.sensitivity,
            retention: self.retention,
            payload: self.payload,
        }
    }
}

/// Allocates the next sequence for `draft`'s stream and inserts its immutable
/// outbox row in `txn`, returning the allocated sequence.
pub async fn stage(txn: &mut dyn KernelTxn, draft: DraftEvent) -> errors::Result<u64> {
    stage_at(txn, draft, SystemClock.now_unix_ms()).await
}

async fn stage_at(
    txn: &mut dyn KernelTxn,
    draft: DraftEvent,
    occurred_at_ms: i64,
) -> errors::Result<u64> {
    let streams = txn.streams();
    let sequence = streams.allocate(draft.stream_key.clone()).await?;
    streams
        .insert_outbox(draft.into_new(sequence, occurred_at_ms))
        .await?;
    Ok(sequence)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use domain::ids::{EventId, EventStreamKey};
    use domain::security::{RetentionClass, SensitivityClass};

    use super::DraftEvent;

    fn event_id(text: &str) -> EventId {
        match EventId::from_str(text) {
            Ok(id) => id,
            Err(error) => panic!("event id rejected: {error}"),
        }
    }

    fn stream_key(text: &str) -> EventStreamKey {
        match EventStreamKey::new(text) {
            Ok(key) => key,
            Err(error) => panic!("stream key rejected: {error}"),
        }
    }

    #[test]
    fn draft_projects_onto_the_immutable_outbox_row() {
        let draft = DraftEvent {
            event_id: event_id("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70"),
            stream_key: stream_key("run/018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70"),
            event_type: "run.created".to_owned(),
            payload: vec![0x07, 0x08],
            sensitivity: SensitivityClass::Confidential,
            retention: RetentionClass::Standard,
            correlation_id: Some("corr-1".to_owned()),
            causation_id: Some(event_id("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e71")),
        };

        let event = draft.into_new(7, 1_700_000_000_042);

        assert_eq!(
            event.event_id,
            event_id("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70")
        );
        assert_eq!(event.event_type, "run.created");
        assert_eq!(event.event_version, 1);
        assert_eq!(
            event.stream_key,
            stream_key("run/018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70")
        );
        assert_eq!(event.sequence, 7);
        assert_eq!(event.occurred_at_ms, 1_700_000_000_042);
        assert_eq!(event.sensitivity, SensitivityClass::Confidential);
        assert_eq!(event.retention, RetentionClass::Standard);
        assert_eq!(event.correlation_id.as_deref(), Some("corr-1"));
        assert_eq!(
            event.causation_id,
            Some(event_id("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e71"))
        );
        assert_eq!(event.payload, vec![0x07, 0x08]);
        assert!(event.run_id.is_none());
        assert!(event.task_id.is_none());
        assert!(event.session_id.is_none());
        assert!(event.effect_id.is_none());
    }
}
