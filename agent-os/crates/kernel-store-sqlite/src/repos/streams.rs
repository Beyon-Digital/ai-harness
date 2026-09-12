//! Stream and outbox repository: structural sequence allocation, immutable
//! outbox insertion, ordered unpublished scans, and publication metadata
//! updates (R6.1-R6.5).
//!
//! Allocation is a single conditional statement per event (D10): the UPSERT
//! either creates the head at `1` or increments it, and `RETURNING` yields the
//! allocated value, so a gap or duplicate is impossible even under concurrent
//! writers serialized by `BEGIN IMMEDIATE`.

use async_trait::async_trait;
use domain::ids::{EventId, EventStreamKey};
use domain::security::{RetentionClass, SensitivityClass};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewOutboxEvent, OutboxEventRow};
use kernel_store::repositories::{PublishKind, StreamRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over `event_stream_heads` and `outbox_events`.
pub(crate) struct SqliteStreamRepo {
    conn: SharedConn,
}

impl SqliteStreamRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_event(row: &SqliteRow) -> errors::Result<OutboxEventRow> {
    let event_version = mapping::int(row, "event_version")?;
    Ok(OutboxEventRow {
        event_id: mapping::decode_id("outbox_events.event_id", &mapping::text(row, "event_id")?)?,
        event_type: mapping::text(row, "event_type")?,
        event_version: u32::try_from(event_version).map_err(|_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "outbox_events.event_version is out of range",
            )
        })?,
        stream_key: mapping::decode_id(
            "outbox_events.stream_key",
            &mapping::text(row, "stream_key")?,
        )?,
        sequence: mapping::decode_u64("outbox_events.sequence", mapping::int(row, "sequence")?)?,
        occurred_at_ms: mapping::int(row, "occurred_at_ms")?,
        run_id: mapping::decode_opt_id("outbox_events.run_id", mapping::opt_text(row, "run_id")?)?,
        task_id: mapping::decode_opt_id(
            "outbox_events.task_id",
            mapping::opt_text(row, "task_id")?,
        )?,
        session_id: mapping::decode_opt_id(
            "outbox_events.session_id",
            mapping::opt_text(row, "session_id")?,
        )?,
        effect_id: mapping::decode_opt_id(
            "outbox_events.effect_id",
            mapping::opt_text(row, "effect_id")?,
        )?,
        causation_id: mapping::decode_opt_id(
            "outbox_events.causation_id",
            mapping::opt_text(row, "causation_id")?,
        )?,
        correlation_id: mapping::opt_text(row, "correlation_id")?,
        sensitivity: mapping::decode_wire(
            "outbox_events.sensitivity",
            mapping::int(row, "sensitivity")?,
            SensitivityClass::from_wire,
        )?,
        retention: mapping::decode_wire(
            "outbox_events.retention",
            mapping::int(row, "retention")?,
            RetentionClass::from_wire,
        )?,
        payload: mapping::blob(row, "payload")?,
        journal_published_at_ms: mapping::opt_int(row, "journal_published_at_ms")?,
        live_published_at_ms: mapping::opt_int(row, "live_published_at_ms")?,
    })
}

#[async_trait]
impl StreamRepo for SqliteStreamRepo {
    async fn allocate(&mut self, stream_key: EventStreamKey) -> errors::Result<u64> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        let row = sqlx::query(
            "INSERT INTO event_stream_heads (stream_key, last_sequence) VALUES (?1, 1) \
             ON CONFLICT(stream_key) DO UPDATE SET last_sequence = last_sequence + 1 \
             RETURNING last_sequence",
        )
        .bind(stream_key.as_str())
        .fetch_one(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        mapping::decode_u64(
            "event_stream_heads.last_sequence",
            mapping::int(&row, "last_sequence")?,
        )
    }

    async fn insert_outbox(&mut self, event: NewOutboxEvent) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        sqlx::query(
            "INSERT INTO outbox_events (event_id, event_type, event_version, stream_key, \
             sequence, occurred_at_ms, run_id, task_id, session_id, effect_id, causation_id, \
             correlation_id, sensitivity, retention, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )
        .bind(event.event_id.to_string())
        .bind(event.event_type)
        .bind(i64::from(event.event_version))
        .bind(event.stream_key.as_str())
        .bind(mapping::encode_u64(
            "outbox_events.sequence",
            event.sequence,
        )?)
        .bind(event.occurred_at_ms)
        .bind(event.run_id.map(|id| id.to_string()))
        .bind(event.task_id.map(|id| id.to_string()))
        .bind(event.session_id.map(|id| id.to_string()))
        .bind(event.effect_id.map(|id| id.to_string()))
        .bind(event.causation_id.map(|id| id.to_string()))
        .bind(event.correlation_id)
        .bind(i64::from(event.sensitivity.to_wire()))
        .bind(i64::from(event.retention.to_wire()))
        .bind(event.payload)
        .execute(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn scan_unpublished(&mut self, limit: u32) -> errors::Result<Vec<OutboxEventRow>> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        let rows = sqlx::query(
            "SELECT event_id, event_type, event_version, stream_key, sequence, \
             occurred_at_ms, run_id, task_id, session_id, effect_id, causation_id, \
             correlation_id, sensitivity, retention, payload, journal_published_at_ms, \
             live_published_at_ms FROM outbox_events WHERE journal_published_at_ms IS NULL \
             ORDER BY stream_key, sequence LIMIT ?1",
        )
        .bind(i64::from(limit))
        .fetch_all(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_event).collect()
    }

    async fn mark_published(&mut self, event_id: EventId, kind: PublishKind) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        let published_at_ms = mapping::unix_ms_now()?;
        let statement = match kind {
            PublishKind::Journal => {
                "UPDATE outbox_events SET journal_published_at_ms = ?1 WHERE event_id = ?2"
            }
            PublishKind::Live => {
                "UPDATE outbox_events SET live_published_at_ms = ?1 WHERE event_id = ?2"
            }
        };
        let result = sqlx::query(statement)
            .bind(published_at_ms)
            .bind(event_id.to_string())
            .execute(conn)
            .await
            .map_err(mapping::from_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                "outbox event not found",
            ));
        }
        Ok(())
    }
}
