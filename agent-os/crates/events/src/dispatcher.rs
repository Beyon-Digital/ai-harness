//! Journal-first outbox dispatcher (EVT-003).
//!
//! One [`EventDispatcher::dispatch_once`] iteration scans the unpublished
//! outbox in `(stream_key, sequence)` order, appends each stream's contiguous
//! batch to the durable journal, marks journal publication in a short fenced
//! write transaction, and only then hands the events to the live sink. The
//! journal append is idempotent, so a crash between append and mark replays
//! safely on the next iteration with no duplicate rows (R3.1-R3.6, D1, D7).

use std::str::FromStr;
use std::sync::Arc;

use domain::faults::FaultInjector;
use domain::ids::{CommandId, PrincipalId};
use domain::provider::IdProvider;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::OutboxEventRow;
use kernel_store::repositories::PublishKind;
use kernel_store::{KernelStore, TxContext};

use crate::envelope::EventEnvelope;
use crate::journal::EventJournalPort;
use crate::stream::StreamKey;

/// Fault point consulted after a journal append and before publication marks.
pub const AFTER_JOURNAL_APPEND: &str = "outbox.after_journal_append";

/// One dispatch iteration's scan and publication counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchOutcome {
    /// Outbox rows returned by the ordered scan.
    pub scanned: usize,
    /// Rows journaled, marked, and handed to the live sink.
    pub published: usize,
    /// Rows scanned but left unpublished by this iteration.
    pub backlog: usize,
}

/// Synchronous hand-off seam to the live event bus (EVT-004).
pub trait LiveSink: Send + Sync {
    /// Hands an already-journaled event to live subscribers.
    fn publish(&self, event: &EventEnvelope);
}

/// Journal-first dispatcher over the kernel outbox.
pub struct EventDispatcher {
    store: Arc<dyn KernelStore>,
    journal: Arc<dyn EventJournalPort>,
    sink: Arc<dyn LiveSink>,
    faults: Arc<dyn FaultInjector>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdProvider>,
    system_principal: PrincipalId,
}

impl EventDispatcher {
    /// Wires the dispatcher to its persistence, journal, sink, and test seams.
    pub fn new(
        store: Arc<dyn KernelStore>,
        journal: Arc<dyn EventJournalPort>,
        sink: Arc<dyn LiveSink>,
        faults: Arc<dyn FaultInjector>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdProvider>,
        system_principal: PrincipalId,
    ) -> Self {
        Self {
            store,
            journal,
            sink,
            faults,
            clock,
            ids,
            system_principal,
        }
    }

    /// Runs one deterministic iteration: scan, append per stream, mark, publish.
    ///
    /// The expected sequence is the first scanned row's sequence minus one
    /// (D7). After every successful append the `outbox.after_journal_append`
    /// fault point is consulted; when it fires the iteration reports
    /// `Unavailable`/`Safe` before any mark is written, so recovery re-appends
    /// the same batch idempotently.
    pub async fn dispatch_once(
        &self,
        limit: u32,
        daemon_epoch: u64,
    ) -> errors::Result<DispatchOutcome> {
        let rows = self.scan(limit, daemon_epoch).await?;

        let mut groups: Vec<(StreamKey, Vec<EventEnvelope>)> = Vec::new();
        for row in &rows {
            let key = StreamKey::from_str(row.stream_key.as_str()).map_err(|_| {
                KernelError::new(
                    ErrorCode::Internal,
                    RetryClass::Never,
                    "outbox row stream key is not canonical",
                )
            })?;
            let envelope = envelope_from_row(row, key.clone());
            match groups.last_mut() {
                Some((existing, batch)) if *existing == key => batch.push(envelope),
                _ => groups.push((key, vec![envelope])),
            }
        }

        let mut envelopes: Vec<EventEnvelope> = Vec::with_capacity(rows.len());
        for (stream_key, batch) in &groups {
            let expected_sequence = batch[0].sequence.saturating_sub(1);
            self.journal
                .append(stream_key, expected_sequence, batch)
                .await?;
            if self.faults.inject(AFTER_JOURNAL_APPEND) {
                return Err(KernelError::new(
                    ErrorCode::Unavailable,
                    RetryClass::Safe,
                    "outbox.after_journal_append fault point fired after a journal append",
                ));
            }
            envelopes.extend(batch.iter().cloned());
        }

        if !rows.is_empty() {
            let mut txn = self
                .store
                .begin_write(self.write_context(daemon_epoch))
                .await?;
            for row in &rows {
                txn.streams()
                    .mark_published(row.event_id, PublishKind::Journal)
                    .await?;
            }
            txn.commit().await?;
        }

        for event in &envelopes {
            self.sink.publish(event);
        }

        Ok(DispatchOutcome {
            scanned: rows.len(),
            published: envelopes.len(),
            backlog: rows.len().saturating_sub(envelopes.len()),
        })
    }

    async fn scan(&self, limit: u32, daemon_epoch: u64) -> errors::Result<Vec<OutboxEventRow>> {
        let mut txn = self
            .store
            .begin_write(self.write_context(daemon_epoch))
            .await?;
        let rows = txn.streams().scan_unpublished(limit).await?;
        txn.rollback().await?;
        Ok(rows)
    }

    fn write_context(&self, daemon_epoch: u64) -> TxContext {
        TxContext {
            daemon_epoch,
            principal_id: self.system_principal,
            command_id: CommandId::new(&*self.ids),
            correlation_id: Some(format!("outbox.dispatch.{}", self.clock.now_unix_ms())),
        }
    }
}

fn envelope_from_row(row: &OutboxEventRow, stream_key: StreamKey) -> EventEnvelope {
    EventEnvelope {
        event_id: row.event_id,
        event_type: row.event_type.clone(),
        event_version: row.event_version,
        stream_key,
        sequence: row.sequence,
        occurred_unix_ms: row.occurred_at_ms,
        run_id: row.run_id,
        task_id: row.task_id,
        session_id: row.session_id,
        effect_id: row.effect_id,
        causation_id: row.causation_id.map(|id| id.to_hyphenated()),
        correlation_id: row.correlation_id.clone(),
        sensitivity: row.sensitivity,
        retention: row.retention,
        payload: row.payload.clone(),
    }
}
